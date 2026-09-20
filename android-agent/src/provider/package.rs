//! PackageProvider（AR8.3 起）：包相关的只读解析先落地，写操作（launch/force-stop/
//! uninstall/替换 SO）在同一 provider 上按 AR8.1/8.4 补齐，统一带 operation_id 与审计。
//!
//! 迁移前 Desktop 把 `dumpsys package <pkg>` 全文拉回宿主，再按字符串找
//! `legacyNativeLibraryDir=`：几十 KB 文本过一遍 IPC，且解析只认「第一个匹配」，
//! 对 32/64 位换算、多实例（Chrome 那种 data-app + system stub 两块）、
//! framework 包（`/system/lib64/framework-res` 根本不是 `<dir>/lib/<abi>` 形态）
//! 一律当异常报错。这里在设备端解析并把这些情况分开表达。

use std::process::Stdio;

use agent_protocol::method::{
    PACKAGE_NATIVE_LIB_DIR, PACKAGE_REPLACE_NATIVE_LIBRARY, PACKAGE_UNINSTALL,
};
use agent_protocol::{
    AgentError, ErrorCode, NativeLibDirSource, OperationStep, PackageNativeLibDirParams,
    PackageNativeLibDirResult, PackageUninstallParams, PackageUninstallResult, ProviderHealth,
    ProviderInfo, ReplaceNativeLibraryParams, ReplaceNativeLibraryResult, WriteOutcome,
};
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

use super::{Provider, ProviderFuture, RequestContext};

const PACKAGE_METHODS: &[&str] = &[
    PACKAGE_NATIVE_LIB_DIR,
    PACKAGE_UNINSTALL,
    PACKAGE_REPLACE_NATIVE_LIBRARY,
];
/// Desktop 用 ADB push 上来的暂存目录（AR8.4）；安装只接受这个目录里的文件。
/// 常量来自协议层，Desktop 拼路径与 Agent 校验路径必须同源。
const STAGED_DIR: &str = agent_protocol::SO_STAGED_ROOT;
/// 设备上算 sha256 的有界时间。
const SHA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PACKAGE_REPLACE_NAME: &str = "package.replace_native_library";
const PM: &str = "/system/bin/pm";
const PACKAGE_UNINSTALL_NAME: &str = "package.uninstall";
const DUMPSYS: &str = "/system/bin/dumpsys";
/// dumpsys 在大包上会慢，给足但仍有界（Desktop 侧超时更短，Agent 不能无限挂着）。
const DUMPSYS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Debug, Default)]
struct PackageDump {
    legacy_native_lib_dir: Option<String>,
    primary_cpu_abi: Option<String>,
    code_path: Option<String>,
    splits: Vec<String>,
    /// dumpsys 里出现了几块不同的包描述（data-app + stub 双块时 >1）
    distinct_native_lib_dirs: usize,
}

pub struct PackageProvider;

impl Provider for PackageProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "package".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: vec!["shell".into()],
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        PACKAGE_METHODS
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                PACKAGE_NATIVE_LIB_DIR => native_lib_dir(params).await,
                PACKAGE_UNINSTALL => uninstall(params).await,
                PACKAGE_REPLACE_NATIVE_LIBRARY => replace_native_library(params).await,
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported package method: {method}"),
                )),
            }
        })
    }
}

async fn native_lib_dir(params: Value) -> Result<Value, AgentError> {
    let params: PackageNativeLibDirParams = parse_params(params)?;
    let result =
        resolve_native_lib_dir(&params.package, params.abi.as_deref(), params.user).await?;
    serialize(result)
}

/// 解析包的 native lib 目录。AR8.4 的 SO 替换也走这里 —— **目标路径只能由包信息推导**，
/// 绝不让调用方传绝对路径，否则「系统包保护」「允许根」全部形同虚设。
async fn resolve_native_lib_dir(
    package: &str,
    abi: Option<&str>,
    user: Option<u32>,
) -> Result<PackageNativeLibDirResult, AgentError> {
    validate_package(package)?;
    let requested = match abi {
        None => None,
        Some(value @ ("arm64" | "arm")) => Some(value),
        Some(other) => {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                format!("ABI 只支持 arm64/arm，收到 {other}"),
            ));
        }
    };
    let text = dumpsys_package(package, user).await?;
    let dump = parse_package_dump(&text);
    if dump.legacy_native_lib_dir.is_none() && dump.code_path.is_none() {
        return Err(AgentError::new(
            ErrorCode::NotFound,
            format!("未找到 {package} 的安装信息（未安装或无权限）"),
        )
        .with_details(serde_json::json!({ "reason": "package_not_found" })));
    }
    let resolved_abi =
        requested.unwrap_or_else(|| abi_from_primary(dump.primary_cpu_abi.as_deref()));
    let (native_lib_dir, source, mut detail) = match dump.legacy_native_lib_dir.as_deref() {
        Some(legacy) => match substitute_abi(legacy, resolved_abi) {
            Substituted::Exact(path) => (path, NativeLibDirSource::FrameworkField, None),
            Substituted::Unsubstituted(path) => (
                path,
                NativeLibDirSource::FrameworkField,
                Some("native_lib_dir_not_substitutable".to_string()),
            ),
        },
        None => {
            let base = dump.code_path.clone().unwrap_or_default();
            (
                format!("{base}/lib/{resolved_abi}"),
                NativeLibDirSource::DerivedFromCodePath,
                Some("derived_from_code_path".to_string()),
            )
        }
    };
    if dump.distinct_native_lib_dirs > 1 {
        detail = Some(match detail {
            Some(existing) => format!(
                "{existing};multiple_package_blocks={}",
                dump.distinct_native_lib_dirs
            ),
            None => format!("multiple_package_blocks={}", dump.distinct_native_lib_dirs),
        });
    }
    Ok(PackageNativeLibDirResult {
        package: package.to_owned(),
        native_lib_dir,
        abi: resolved_abi.to_owned(),
        primary_cpu_abi: dump.primary_cpu_abi.clone(),
        code_path: dump.code_path.clone(),
        splits: dump.splits.clone(),
        user,
        source,
        detail,
    })
}

/// 卸载第三方应用（AR8.1 写操作，**不可逆**，因此结果带步骤链）。
///
/// 三道闸：① `operation_id` 必填且合法；② 目标必须是**已安装的第三方应用**
/// （未安装 → `not_found`，系统包 → `precondition_failed`，两种误导是相反的）；
/// ③ 执行后必须自己复核 `pm path` 已空——`pm` 口头说 Success 不算数。
/// 步骤链同时进结果与错误 details：卸载失败时用户要能看出卡在哪一步。
async fn uninstall(params: Value) -> Result<Value, AgentError> {
    let params: PackageUninstallParams = parse_params(params)?;
    super::activity::begin_write(
        PACKAGE_UNINSTALL_NAME,
        &params.package,
        &params.operation_id,
    )?;
    if let Some(cached) = super::operations::lookup(&params.operation_id) {
        return Ok(super::operations::mark_replayed(cached));
    }
    let mut steps = Vec::new();
    steps.push(step("guard_target", true, None));
    if let Err(error) = super::activity::guard_writable_package(&params.package).await {
        return Err(with_steps(error, &steps));
    }

    let mut args: Vec<String> = vec!["uninstall".to_string()];
    if params.keep_data {
        args.push("-k".to_string());
    }
    if let Some(user) = params.user {
        args.extend(["--user".to_string(), user.to_string()]);
    }
    args.push(params.package.clone());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = match super::activity::run(PM, &refs).await {
        Ok(output) => output,
        Err(error) => {
            steps.push(step("pm_uninstall", false, Some(error.message.clone())));
            return Err(with_steps(error, &steps));
        }
    };
    let rejected = output.contains("Failure") || output.contains("DELETE_FAILED");
    steps.push(step(
        "pm_uninstall",
        !rejected,
        Some(output.trim().chars().take(160).collect()),
    ));
    if rejected {
        // pm 明确拒绝：不复核、不谎报，原样把它的措辞带回给用户
        return Err(with_steps(
            AgentError::new(
                ErrorCode::PreconditionFailed,
                format!("pm 拒绝卸载 {}: {}", params.package, output.trim()),
            )
            .with_details(serde_json::json!({ "reason": "pm_rejected" })),
            &steps,
        ));
    }
    let gone = !super::activity::pm_path_present(&params.package).await;
    steps.push(step("verify_removed", gone, None));
    let detail = Some(if gone {
        "path_gone"
    } else {
        "path_still_present"
    });
    let value = serialize(PackageUninstallResult {
        package: params.package.clone(),
        operation_id: params.operation_id.clone(),
        outcome: WriteOutcome::Executed,
        verified: gone,
        keep_data: params.keep_data,
        steps: steps.clone(),
        detail: detail.map(str::to_owned),
    })?;
    super::activity::finish_write(
        PACKAGE_UNINSTALL_NAME,
        &params.operation_id,
        &params.package,
        &value,
    );
    if !gone {
        // 复核不过：返回结果但明确 verified=false，并在错误链路上也留步骤
        return Err(with_steps(
            AgentError::new(
                ErrorCode::Internal,
                format!("卸载后 {} 的 pm path 仍在", params.package),
            )
            .with_details(serde_json::json!({ "reason": "verify_failed" })),
            &steps,
        ));
    }
    Ok(value)
}

fn step(name: &str, ok: bool, detail: Option<String>) -> OperationStep {
    OperationStep {
        name: name.to_owned(),
        ok,
        detail,
    }
}

/// 把已走过的步骤挂到错误的 details 上，让「失败在哪一步」能穿过 typed 错误边界。
fn with_steps(mut error: AgentError, steps: &[OperationStep]) -> AgentError {
    let mut details = error.details.unwrap_or_else(|| serde_json::json!({}));
    if let Some(object) = details.as_object_mut() {
        if let Ok(value) = serde_json::to_value(steps) {
            object.insert("steps".to_owned(), value);
        }
    }
    error.details = Some(details);
    error
}

/// `dumpsys package <pkg> [--user N]`：参数数组执行，包名不进 shell。
async fn dumpsys_package(package: &str, user: Option<u32>) -> Result<String, AgentError> {
    let mut args: Vec<String> = vec!["package".to_string(), package.to_string()];
    if let Some(user) = user {
        args.extend(["--user".to_string(), user.to_string()]);
    }
    let output = tokio::time::timeout(
        DUMPSYS_TIMEOUT,
        Command::new(DUMPSYS)
            .args(&args)
            .stdin(Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| {
        AgentError::new(
            ErrorCode::DeadlineExceeded,
            format!("dumpsys package {package} 超时"),
        )
    })?
    .map_err(|error| AgentError::new(ErrorCode::Internal, format!("dumpsys 执行失败: {error}")))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() && !text.contains("Packages") {
        return Err(AgentError::new(
            ErrorCode::Internal,
            format!(
                "dumpsys package 退出码 {:?}",
                output.status.code().unwrap_or_default()
            ),
        ));
    }
    Ok(text)
}

/// 解析包描述块。字段取第一次出现（与 Legacy `parse_legacy_native_lib` 同规则），
/// 但会数一下有几个不同的 native lib 目录，双块情况必须能被上层看到。
fn parse_package_dump(text: &str) -> PackageDump {
    let mut dump = PackageDump::default();
    let mut seen_dirs: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if dump.legacy_native_lib_dir.is_none() {
            if let Some(value) = value_after(line, "legacyNativeLibraryDir=") {
                dump.legacy_native_lib_dir = Some(value.to_string());
            }
        }
        if dump.primary_cpu_abi.is_none() {
            if let Some(value) = value_after(line, "primaryCpuAbi=") {
                // `primaryCpuAbi=null` 是 Framework 的写法，不当成有值
                dump.primary_cpu_abi = match value {
                    "" | "null" => None,
                    other => Some(other.to_string()),
                };
            }
        }
        if dump.code_path.is_none() {
            if let Some(value) = value_after(line, "codePath=") {
                dump.code_path = (!value.is_empty()).then(|| value.to_string());
            }
        }
        if dump.splits.is_empty() {
            if let Some(value) = value_after(line, "splits=[") {
                dump.splits = value
                    .trim_end_matches(']')
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .map(str::to_string)
                    .collect();
            }
        }
        if let Some(value) = value_after(line, "legacyNativeLibraryDir=") {
            if !seen_dirs.iter().any(|seen| seen == value) {
                seen_dirs.push(value.to_string());
            }
        }
    }
    dump.distinct_native_lib_dirs = seen_dirs.len();
    dump
}

fn value_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let idx = line.find(key)?;
    let value = line[idx + key.len()..].trim();
    Some(value)
}

/// `arm64-v8a`/`arm64` → `arm64`；`armeabi-v7a`/`armeabi` → `arm`；其余按 arm64 处理
/// 但调用方可以从 `primary_cpu_abi` 自己看出这不是一次可靠判定。
fn abi_from_primary(primary: Option<&str>) -> &'static str {
    match primary {
        Some(value) if value.contains("armeabi") && !value.contains("arm64") => "arm",
        _ => "arm64",
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Substituted {
    Exact(String),
    /// Framework 给的不是 `<pkg目录>/lib[/<abi>]` 形态（framework 包常这样）：
    /// 按原值返回，不猜 ABI 子目录
    Unsubstituted(String),
}

/// 与 Desktop `adb::lib_dir_for_abi` 同规则：`…/lib` 或 `…/lib/<abi>` 才能换 ABI。
fn substitute_abi(legacy_dir: &str, abi: &str) -> Substituted {
    let dir = legacy_dir.trim_end_matches('/');
    if abi != "arm64" && abi != "arm" {
        return Substituted::Unsubstituted(dir.to_string());
    }
    if dir.ends_with("/lib") {
        return Substituted::Exact(format!("{dir}/{abi}"));
    }
    for known in ["/lib/arm64", "/lib/arm"] {
        if let Some(base) = dir.strip_suffix(known) {
            return Substituted::Exact(format!("{base}/lib/{abi}"));
        }
    }
    Substituted::Unsubstituted(dir.to_string())
}

fn validate_package(package: &str) -> Result<(), AgentError> {
    // 规则与 ActivityProvider 完全一致：不额外要求「必须含点」，
    // 否则 framework 资源包 `android` 这种合法名字会被自己的校验挡掉
    if !super::activity::is_safe_package_name(package) {
        return Err(
            AgentError::new(ErrorCode::InvalidRequest, format!("包名非法: {package}"))
                .with_details(serde_json::json!({ "reason": "invalid_package_name" })),
        );
    }
    Ok(())
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AgentError> {
    serde_json::from_value(params).map_err(|error| {
        AgentError::new(ErrorCode::InvalidRequest, "invalid package parameters")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn invalid(reason: &str, message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::InvalidRequest, message)
        .with_details(serde_json::json!({ "reason": reason }))
}

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|error| {
        AgentError::new(
            ErrorCode::Internal,
            format!("failed to encode package result: {error}"),
        )
    })
}

/// AR8.4：把主机侧修补好的 `.so` 原子装到包的 native lib 目录。
///
/// 目标目录属 `system:system`，shell 写不进去，所以走 D037 的特权层，且只允许
/// 代码里写死的固定脚本（D038）。顺序：校验输入 → 校验暂存件（ELF + sha256 算得出）
/// → 备份原件 → 同目录临时名落盘（权限/属主/SELinux 上下文跟随原件 + `sync`）→
/// 原子 `rename` → **复核目标 sha256 与暂存件一致**；任何一步失败立即回滚
/// （原本存在→装回备份；原本不存在→删掉我们写的文件），回滚结果进步骤链。
async fn replace_native_library(params: Value) -> Result<Value, AgentError> {
    let params: ReplaceNativeLibraryParams = parse_params(params)?;
    super::activity::begin_write(PACKAGE_REPLACE_NAME, &params.package, &params.operation_id)?;
    if let Some(cached) = super::operations::lookup(&params.operation_id) {
        return Ok(super::operations::mark_replayed(cached));
    }
    let mut steps: Vec<OperationStep> = Vec::new();
    let mut push = |name: &str, ok: bool, detail: Option<String>| {
        steps.push(OperationStep {
            name: name.to_owned(),
            ok,
            detail,
        });
    };

    // ① 输入形状：abi / so 名 / 暂存路径全部白名单校验，路径还要过特权层的字符集检查
    if params.abi != "arm64" && params.abi != "arm" {
        return Err(invalid(
            "invalid_abi",
            format!("ABI 只支持 arm64/arm，收到 {}", params.abi),
        ));
    }
    if !is_safe_so_name(&params.so_name) {
        return Err(invalid(
            "invalid_so_name",
            format!("非法 so 文件名: {}", params.so_name),
        ));
    }
    super::privileged::validate_path(&params.staged_path)?;
    if !is_staged_in_own_dir(&params.staged_path) {
        return Err(invalid(
            "staged_dir_denied",
            format!(
                "暂存件必须位于 {STAGED_DIR}/<本次操作目录>/ 下（一次操作一个目录，备份与原件不互相覆盖）: {}",
                params.staged_path
            ),
        ));
    }
    push("validate_input", true, None);

    // ② 目标范围：已安装的第三方应用（未安装 not_found / 系统包 protected 两种误导是相反的）
    if let Err(error) = super::activity::guard_writable_package(&params.package).await {
        push("guard_target", false, Some(error.message.clone()));
        return Err(with_steps(error, &steps));
    }
    push("guard_target", true, None);

    // ③ 目标路径由包信息推导，不接受调用方传入
    let native =
        match resolve_native_lib_dir(&params.package, Some(params.abi.as_str()), None).await {
            Ok(native) => native,
            Err(error) => {
                push("resolve_target", false, Some(error.message.clone()));
                return Err(with_steps(error, &steps));
            }
        };
    let target_dir = if native.native_lib_dir.ends_with("/lib") {
        format!("{}/{}", native.native_lib_dir, native.abi)
    } else {
        native.native_lib_dir.clone()
    };
    let target = format!("{target_dir}/{}", params.so_name);
    if let Err(error) = super::privileged::validate_path(&target) {
        push("resolve_target", false, Some(error.message.clone()));
        return Err(with_steps(error, &steps));
    }
    push("resolve_target", true, Some(target.clone()));
    let replaced_existing = std::fs::symlink_metadata(&target).is_ok();

    // ④ 暂存件必须是有效 ELF 且算得出 sha256（不是「文件存在」就算合法）
    let staged_sha = match file_sha256(&params.staged_path).await {
        Ok(sha) => sha,
        Err(error) => {
            push("verify_staged", false, Some(error.message.clone()));
            return Err(with_steps(error, &steps));
        }
    };
    if !is_elf_file(Path::new(&params.staged_path)) {
        push("verify_staged", false, Some("ELF magic 不对".to_owned()));
        return Err(with_steps(
            invalid(
                "staged_not_elf",
                format!("暂存件不是有效的 ELF 共享库: {}", params.staged_path),
            ),
            &steps,
        ));
    }
    push(
        "verify_staged",
        true,
        Some(format!("sha256={}…", &staged_sha[..12])),
    );

    // ⑤ 备份原件（root 读 /data/app 更稳，shell 有时真读不到）。备份件放在**本次操作的
    // 唯一暂存目录**里：同一 so 被替换两次也不会互相覆盖掉第一个备份。
    let staged_dir = std::path::Path::new(&params.staged_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| STAGED_DIR.to_owned());
    let backup = format!("{staged_dir}/{}.artbak", params.so_name);
    super::privileged::validate_path(&backup)?;
    if replaced_existing {
        match super::privileged::run_privileged(
            "backup_original",
            &super::privileged::backup_script(&target, &backup),
            "BACKED_UP",
        )
        .await
        {
            Ok(_) => push("backup_original", true, Some(backup.clone())),
            Err(error) => {
                push("backup_original", false, Some(error.message.clone()));
                return Err(with_steps(error, &steps));
            }
        }
    } else {
        push(
            "backup_original",
            true,
            Some("目标原本不存在，无需备份".to_owned()),
        );
    }

    // ⑥ 原子安装
    let tmp = format!("{target}.arttmp");
    if let Err(error) = super::privileged::validate_dir(&target_dir) {
        push("install", false, Some(error.message.clone()));
        return Err(with_steps(error, &steps));
    }
    if let Err(error) = super::privileged::run_privileged(
        "install",
        &super::privileged::install_script(&params.staged_path, &target, &tmp, &target_dir),
        "INSTALLED",
    )
    .await
    {
        push("install", false, Some(error.message.clone()));
        let rolled_back = rollback(&backup, &target, replaced_existing).await;
        push(
            "rollback",
            rolled_back,
            Some(if rolled_back {
                "已恢复原状".to_owned()
            } else {
                "恢复失败，需要人工处理".to_owned()
            }),
        );
        return Err(with_steps(error, &steps));
    }
    push("install", true, Some(target.clone()));

    // ⑦ 复核：目标 sha256 必须等于暂存件（「cp 没报错」不算成功）
    let (verified, verify_detail) = match file_sha256(&target).await {
        Ok(sha) => (
            sha == staged_sha,
            if sha == staged_sha {
                format!("sha256={}… 一致", &staged_sha[..12])
            } else {
                format!("目标 sha256={}… 与暂存件不一致", &sha[..12])
            },
        ),
        Err(error) => (false, format!("复核失败: {}", error.message)),
    };
    push("verify_installed", verified, Some(verify_detail));
    let rolled_back = if verified {
        None
    } else {
        let done = rollback(&backup, &target, replaced_existing).await;
        push(
            "rollback",
            done,
            Some(if done {
                "复核不过，已恢复原状".to_owned()
            } else {
                "复核不过且恢复失败，需要人工处理".to_owned()
            }),
        );
        Some(done)
    };
    let value = serialize(ReplaceNativeLibraryResult {
        package: params.package.clone(),
        target_path: target.clone(),
        staged_path: params.staged_path.clone(),
        operation_id: params.operation_id.clone(),
        // 走到这里一定真的执行过安装；成不成由 verified 与复核步骤表达
        outcome: WriteOutcome::Executed,
        verified,
        replaced_existing,
        steps: steps.clone(),
        rolled_back,
        backup_path: replaced_existing.then_some(backup.clone()),
        detail: Some(if verified {
            "sha256_matched".to_owned()
        } else {
            "verify_failed_rolled_back".to_owned()
        }),
    })?;
    super::activity::finish_write(
        PACKAGE_REPLACE_NAME,
        &params.operation_id,
        &params.package,
        &value,
    );
    if !verified {
        return Err(with_steps(
            AgentError::new(ErrorCode::Internal, format!("SO 替换复核未通过: {target}"))
                .with_details(serde_json::json!({ "reason": "verify_failed" })),
            &steps,
        ));
    }
    Ok(value)
}

/// 恢复备份或删掉新增文件；返回是否成功，不掩盖失败。
async fn rollback(backup: &str, target: &str, restore_backup: bool) -> bool {
    super::privileged::run_privileged(
        "rollback",
        &super::privileged::rollback_script(backup, target, restore_backup),
        if restore_backup {
            "ROLLED_BACK"
        } else {
            "REMOVED"
        },
    )
    .await
    .is_ok()
}

/// 暂存件必须躺在 `{STAGED_DIR}/<本次操作目录>/` 里：**恰好一层**、目录名不以 `.` 开头、
/// 不得再有二级路径。这条判据是真机腿跑出来的第一个缺陷——原先要求父目录严格等于
/// `{STAGED_DIR}`，与 Desktop「每次操作一个唯一子目录」的约定对不上，产品链路必挂。
fn is_staged_in_own_dir(path: &str) -> bool {
    let Some(parent) = Path::new(path).parent() else {
        return false;
    };
    let Ok(relative) = parent.strip_prefix(Path::new(STAGED_DIR)) else {
        return false;
    };
    let mut components = relative.components();
    let Some(first) = components.next() else {
        return false;
    };
    if components.next().is_some() {
        return false;
    }
    let name = first.as_os_str().to_string_lossy();
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '+'))
}

/// 常规 so 名：`[A-Za-z0-9._+-]` + `.so` 结尾 + 不以 `.` 开头。
///
/// `+` 必须放行：真实 NDK 库就叫 `libc++_shared.so`，砍掉它等于砍掉最常见的目标。
/// 这里禁的是 shell 元字符与路径分隔，不是 C++ ABI 名字里的符号。
fn is_safe_so_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name.ends_with(".so")
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '+'))
}

/// ELF magic + 32/64 位 class；size 太小直接判否。
fn is_elf_file(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0_u8; 5];
    if std::io::Read::read_exact(&mut file, &mut head).is_err() {
        return false;
    }
    head[..4] == [0x7f, b'E', b'L', b'F'] && matches!(head[4], 1 | 2)
}

/// 设备上算 sha256：`sha256sum`，缺失退 `toybox sha256sum`。**参数数组执行**，
/// 不做 shell 拼接；两条都不通就报错，绝不拿「没算出来」冒充「算过且一致」。
async fn file_sha256(path: &str) -> Result<String, AgentError> {
    let candidates: [(&str, Vec<&str>); 2] = [
        ("/system/bin/sha256sum", vec![path]),
        ("/system/bin/toybox", vec!["sha256sum", path]),
    ];
    for (program, args) in candidates {
        let attempt = tokio::time::timeout(SHA_TIMEOUT, Command::new(program).args(&args).output())
            .await
            .ok()
            .and_then(|inner| inner.ok())
            .filter(|output| output.status.success());
        if let Some(output) = attempt {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            if let Some(sha) = text
                .split_whitespace()
                .next()
                .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
            {
                return Ok(sha.to_ascii_lowercase());
            }
        }
    }
    Err(AgentError::new(
        ErrorCode::Internal,
        format!("算不出 {path} 的 sha256（设备缺 sha256sum？）"),
    )
    .with_details(serde_json::json!({ "reason": "sha256_unavailable" })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(value: serde_json::Value) -> Value {
        value
    }
    #[test]
    fn so_names_that_could_escape_the_lib_dir_are_rejected() {
        for bad in [
            "",
            ".hidden.so",
            "../lib.so",
            "a b.so",
            "libfoo.so;rm",
            "libfoo.txt",
            &("x".repeat(96) + ".so"),
        ] {
            assert!(!is_safe_so_name(bad), "{bad:?} 必须被拒");
        }
        for good in ["libfoo.so", "libc++_shared.so", "lib-v1_2.so"] {
            assert!(is_safe_so_name(good), "{good:?} 应该合法");
        }
    }

    #[test]
    fn elf_probe_accepts_both_classes_and_rejects_junk() {
        let dir = tempfile::tempdir().unwrap();
        let elf64 = dir.path().join("a.so");
        std::fs::write(&elf64, [0x7f, b'E', b'L', b'F', 2, 1, 0, 0]).unwrap();
        let elf32 = dir.path().join("b.so");
        std::fs::write(&elf32, [0x7f, b'E', b'L', b'F', 1, 1, 0, 0]).unwrap();
        let script = dir.path().join("c.so");
        std::fs::write(&script, b"#!/system/bin/sh\n").unwrap();
        let tiny = dir.path().join("d.so");
        std::fs::write(&tiny, b"ab").unwrap();
        assert!(is_elf_file(&elf64));
        assert!(is_elf_file(&elf32), "32 位 ELF 同样是合法 so");
        assert!(!is_elf_file(&script), "脚本不是 ELF");
        assert!(!is_elf_file(&tiny), "短到读不满头的文件不能算 ELF");
        assert!(!is_elf_file(&dir.path().join("missing.so")));
    }

    /// 输入校验必须**先于**任何特权调用：宿主上没有 su，如果顺序错了这些断言
    /// 会变成 su_unavailable 而不是 invalid_request，一眼能看出来。
    #[tokio::test]
    async fn replace_rejects_bad_input_before_touching_su() {
        let cases = [
            json(serde_json::json!({
                "package": "com.x", "abi": "x86", "so_name": "a.so",
                "staged_path": "/data/local/tmp/app-reverse-tools-so/com.x-1/a.so",
                "operation_id": "op-1"
            })),
            json(serde_json::json!({
                "package": "com.x", "abi": "arm64", "so_name": "../a.so",
                "staged_path": "/data/local/tmp/app-reverse-tools-so/com.x-1/a.so",
                "operation_id": "op-2"
            })),
            json(serde_json::json!({
                "package": "com.x", "abi": "arm64", "so_name": "a.so",
                "staged_path": "/data/local/tmp/elsewhere/a.so",
                "operation_id": "op-3"
            })),
            json(serde_json::json!({
                "package": "com.x", "abi": "arm64", "so_name": "a.so",
                "staged_path": "/data/app/com.x/lib/arm64/a.so",
                "operation_id": "op-4"
            })),
            json(serde_json::json!({
                "package": "com.x", "abi": "arm64", "so_name": "a.so",
                "staged_path": "/data/local/tmp/app-reverse-tools-so/com.x-1/a.so",
                "operation_id": ""
            })),
        ];
        for params in cases {
            let error = replace_native_library(params)
                .await
                .expect_err("非法输入必须在起 su 之前被拒");
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
        }
    }

    /// 暂存目录形状：Desktop 每次操作用一个唯一子目录，Agent 必须认这个形状。
    /// （回归来自真机腿：原先要求父目录严格等于暂存根，产品链路一步都走不通。）
    #[test]
    fn staged_file_must_sit_in_its_own_operation_dir() {
        let good = [
            "/data/local/tmp/app-reverse-tools-so/com.x-1f2e3d4c/libfoo.so",
            "/data/local/tmp/app-reverse-tools-so/com.amazon.mShop.android.shopping-84a1b2c3/libc++_shared.so",
        ];
        for path in good {
            assert!(is_staged_in_own_dir(path), "{path} 应该被接受");
        }
        let bad = [
            // 直接躺在根目录下：备份会互相覆盖
            "/data/local/tmp/app-reverse-tools-so/libfoo.so",
            // 二级嵌套：路径来源不可控
            "/data/local/tmp/app-reverse-tools-so/a/b/libfoo.so",
            // 根目录之外
            "/data/local/tmp/libfoo.so",
            "/data/local/tmp/app-reverse-tools-so/../evil/libfoo.so",
            "/data/data/com.x/libfoo.so",
            // 隐藏目录名
            "/data/local/tmp/app-reverse-tools-so/.op/libfoo.so",
        ];
        for path in bad {
            assert!(!is_staged_in_own_dir(path), "{path} 必须被拒");
        }
    }

    /// 算不出 sha256 时必须**报错**，不能拿「没算」当「算过且一致」。
    #[tokio::test]
    async fn sha256_failure_is_an_error_not_a_silent_pass() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.so");
        std::fs::write(&file, [0x7f, b'E', b'L', b'F', 2]).unwrap();
        let result = file_sha256(&file.display().to_string()).await;
        if let Err(error) = result {
            assert_eq!(error.code, ErrorCode::Internal);
            assert_eq!(error.details.unwrap()["reason"], "sha256_unavailable");
        }
    }

    /// Android 14 真机形状：`legacyNativeLibraryDir` 以 `/lib` 结尾，ABI 子目录要我们自己补
    const CHROME: &str = concat!(
        "  codePath=/data/app/~~fl-8paORbyMWqEKMbAQCFQ==/com.android.chrome-s0QJMh6yB4fXyLKUvjTKuw==\n",
        "  legacyNativeLibraryDir=/data/app/~~fl-8paORbyMWqEKMbAQCFQ==/com.android.chrome-s0QJMh6yB4fXyLKUvjTKuw==/lib\n",
        "  primaryCpuAbi=arm64-v8a\n",
        "  splits=[base, chrome, config.zh, dev_ui, on_demand]\n",
        "  User 0: ceDataInode=20304 installed=true\n",
    );

    #[test]
    fn parses_dumpsys_fields_and_abi_substitution() {
        let dump = parse_package_dump(CHROME);
        assert!(
            dump.code_path
                .unwrap()
                .ends_with("com.android.chrome-s0QJMh6yB4fXyLKUvjTKuw==")
        );
        assert_eq!(
            dump.legacy_native_lib_dir.as_deref().map(|v| v.to_string()),
            Some(
                "/data/app/~~fl-8paORbyMWqEKMbAQCFQ==/com.android.chrome-s0QJMh6yB4fXyLKUvjTKuw==/lib"
                    .to_string()
            )
        );
        assert_eq!(dump.primary_cpu_abi.as_deref(), Some("arm64-v8a"));
        assert_eq!(dump.splits.len(), 5);
        assert_eq!(dump.splits[2], "config.zh");
        assert_eq!(dump.distinct_native_lib_dirs, 1);
        assert_eq!(
            substitute_abi(dump.legacy_native_lib_dir.as_deref().unwrap(), "arm64"),
            Substituted::Exact(
                "/data/app/~~fl-8paORbyMWqEKMbAQCFQ==/com.android.chrome-s0QJMh6yB4fXyLKUvjTKuw==/lib/arm64"
                    .to_string()
            )
        );
    }

    #[test]
    fn abi_substitution_handles_all_framework_dir_shapes() {
        assert_eq!(
            substitute_abi("/data/app/~~x/com.y-==/lib/arm64", "arm"),
            Substituted::Exact("/data/app/~~x/com.y-==/lib/arm".to_string())
        );
        assert_eq!(
            substitute_abi("/data/app/~~x/com.y-==/lib/", "arm64"),
            Substituted::Exact("/data/app/~~x/com.y-==/lib/arm64".to_string())
        );
        // framework 包：/system/lib64/framework-res 没有 <pkg>/lib 结构，不能瞎猜
        assert_eq!(
            substitute_abi("/system/lib64/framework-res", "arm64"),
            Substituted::Unsubstituted("/system/lib64/framework-res".to_string())
        );
        assert_eq!(
            substitute_abi("/data/app/x/lib/arm64", "mips"),
            Substituted::Unsubstituted("/data/app/x/lib/arm64".to_string())
        );
    }

    #[test]
    fn primary_cpu_abi_null_is_not_a_value_and_drives_abi_choice() {
        let dump = parse_package_dump(
            "  codePath=/product/app/Chrome-Stub\n  legacyNativeLibraryDir=/product/app/Chrome-Stub/lib\n  primaryCpuAbi=null\n",
        );
        assert!(
            dump.primary_cpu_abi.is_none(),
            "primaryCpuAbi=null 必须当成没有"
        );
        assert_eq!(abi_from_primary(None), "arm64");
        assert_eq!(abi_from_primary(Some("arm64-v8a")), "arm64");
        assert_eq!(abi_from_primary(Some("armeabi-v7a")), "arm");
    }

    /// 同一个包里出现两块不同 native lib 目录（data-app 与 system stub）时必须留证据，
    /// 否则调用方拿到哪一块全凭运气。
    #[test]
    fn multiple_package_blocks_are_counted() {
        let text = CHROME.to_string()
            + "  codePath=/product/app/Chrome-Stub\n  legacyNativeLibraryDir=/product/app/Chrome-Stub/lib\n";
        let dump = parse_package_dump(&text);
        assert_eq!(dump.distinct_native_lib_dirs, 2);
        // 取第一块，与 Legacy 的「第一个匹配」规则一致
        assert!(
            dump.legacy_native_lib_dir
                .unwrap()
                .contains("com.android.chrome")
        );
    }

    #[test]
    fn splits_absent_stays_empty_not_missing() {
        let dump = parse_package_dump("  codePath=/system/framework/framework-res.apk\n");
        assert!(dump.splits.is_empty());
        assert_eq!(dump.distinct_native_lib_dirs, 0);
    }

    #[test]
    fn package_names_that_could_reach_the_shell_are_rejected() {
        for bad in [
            "",
            "com.x; rm -rf /",
            "com.x$(id)",
            "com.x|grep",
            "com x",
            "com.x\nrm",
            "com.x:extra",
        ] {
            let error = validate_package(bad).expect_err(&format!("{bad:?} 必须被拒"));
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert_eq!(error.details.unwrap()["reason"], "invalid_package_name");
        }
        assert!(validate_package("com.android.chrome").is_ok());
        assert!(validate_package("io.github.vvb2060.mahoshojo").is_ok());
        // framework 资源包没有点号，也是合法包名（真机 AR8.3 腿就依赖这条）
        assert!(validate_package("android").is_ok());
    }

    #[tokio::test]
    async fn unknown_abi_is_rejected_before_dumpsys_runs() {
        let error = native_lib_dir(serde_json::json!({ "package": "com.x", "abi": "x86" }))
            .await
            .expect_err("非法 ABI 必须拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }

    #[tokio::test]
    async fn missing_package_is_not_found_rather_than_empty_result() {
        // 非 Android 宿主上 dumpsys 不存在 → 报的是执行失败；这里只验证参数校验后的
        // 错误链路可分辨，不依赖 dumpsys 是否可用
        let error = native_lib_dir(serde_json::json!({ "package": "!!!" }))
            .await
            .expect_err("非法包名必须被拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
}

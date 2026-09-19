//! PackageProvider（AR8.3 起）：包相关的只读解析先落地，写操作（launch/force-stop/
//! uninstall/替换 SO）在同一 provider 上按 AR8.1/8.4 补齐，统一带 operation_id 与审计。
//!
//! 迁移前 Desktop 把 `dumpsys package <pkg>` 全文拉回宿主，再按字符串找
//! `legacyNativeLibraryDir=`：几十 KB 文本过一遍 IPC，且解析只认「第一个匹配」，
//! 对 32/64 位换算、多实例（Chrome 那种 data-app + system stub 两块）、
//! framework 包（`/system/lib64/framework-res` 根本不是 `<dir>/lib/<abi>` 形态）
//! 一律当异常报错。这里在设备端解析并把这些情况分开表达。

use std::process::Stdio;

use agent_protocol::method::PACKAGE_NATIVE_LIB_DIR;
use agent_protocol::{
    AgentError, ErrorCode, NativeLibDirSource, PackageNativeLibDirParams,
    PackageNativeLibDirResult, ProviderHealth, ProviderInfo,
};
use serde_json::Value;
use tokio::process::Command;

use super::{Provider, ProviderFuture, RequestContext};

const PACKAGE_METHODS: &[&str] = &[PACKAGE_NATIVE_LIB_DIR];
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
    validate_package(&params.package)?;
    let requested = match params.abi.as_deref() {
        None => None,
        Some(value @ ("arm64" | "arm")) => Some(value),
        Some(other) => {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                format!("ABI 只支持 arm64/arm，收到 {other}"),
            ));
        }
    };

    let text = dumpsys_package(&params.package, params.user).await?;
    let dump = parse_package_dump(&text);
    if dump.legacy_native_lib_dir.is_none() && dump.code_path.is_none() {
        // 两个字段都没有 = 这个包在 dumpsys 里根本没有描述，即未安装/无权限
        return Err(AgentError::new(
            ErrorCode::NotFound,
            format!("未找到 {} 的安装信息（未安装或无权限）", params.package),
        )
        .with_details(serde_json::json!({ "reason": "package_not_found" })));
    }

    let (native_lib_dir, source, mut detail) = match dump.legacy_native_lib_dir.as_deref() {
        Some(legacy) => {
            let abi =
                requested.unwrap_or_else(|| abi_from_primary(dump.primary_cpu_abi.as_deref()));
            match substitute_abi(legacy, abi) {
                Substituted::Exact(path) => (path, NativeLibDirSource::FrameworkField, None),
                Substituted::Unsubstituted(path) => (
                    path,
                    NativeLibDirSource::FrameworkField,
                    Some("native_lib_dir_not_substitutable".to_string()),
                ),
            }
        }
        // Framework 没给 legacyNativeLibraryDir（老 ROM/特殊包）：只能从 codePath 推，
        // 来源必须标出来，让调用方知道这是推导值而不是 Framework 承诺
        None => {
            let abi =
                requested.unwrap_or_else(|| abi_from_primary(dump.primary_cpu_abi.as_deref()));
            let base = dump.code_path.clone().unwrap_or_default();
            (
                format!("{base}/lib/{abi}"),
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

    let abi = requested
        .unwrap_or_else(|| abi_from_primary(dump.primary_cpu_abi.as_deref()))
        .to_owned();
    serialize(PackageNativeLibDirResult {
        package: params.package.clone(),
        native_lib_dir,
        abi,
        primary_cpu_abi: dump.primary_cpu_abi.clone(),
        code_path: dump.code_path.clone(),
        splits: dump.splits.clone(),
        user: params.user,
        source,
        detail,
    })
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

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|error| {
        AgentError::new(
            ErrorCode::Internal,
            format!("failed to encode package result: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

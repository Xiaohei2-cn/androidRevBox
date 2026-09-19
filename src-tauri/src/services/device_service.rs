//! DeviceService：设备生命周期与 ADB 能力的业务入口（P3）。
//! 设计（PHASES §1.5 三平台约束）：
//! - 所有 adb 基础指令收敛到 `AdbRunner` 这个 Rust trait（不是网络接口）；
//! - RealAdbRunner 真实执行短命令（capture stdout/exit），长操作统一生成 TaskService 任务走事件流；
//! - MockAdbRunner 返回固定数据，CI 无真机也能测行为分支与热插拔 diff。
//!
//! adb 路径解析优先级：手动配置(app.adb.path) → ANDROID_HOME/ANDROID_SDK_ROOT → PATH 环境变量；
//! 全部落空返回「未安装」状态并给出前端提示文案。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_protocol::method::{
    DEVICE_INFO, PACKAGE_LIST, PROCESS_BY_PORT, PROCESS_KILL, PROCESS_PORTS,
};
use agent_protocol::{
    DeviceInfoParams, DeviceInfoResult, KillSignal, ListeningPort, PackageListParams,
    PackageListResult, PackageScope, PortHoldingProcess, ProcessByPortParams, ProcessByPortResult,
    ProcessKillParams, ProcessKillResult, ProcessPortsParams, ProcessPortsResult, SocketFamily,
};
use async_trait::async_trait;
use serde::Serialize;
use tauri::Emitter;

use crate::adapters::adb::{self, AdbVersionInfo, DeviceEntry, DeviceInfo, FileEntry};
use crate::core::error::{CoreError, CoreResult};
use crate::core::ipc::{AppEvent, event_names};
use crate::db::Db;
use crate::models::agent::AndroidBackendSource;
use crate::services::android_backend::{CapabilityRouter, OperationKind, RouteError};
use crate::services::config_service::ConfigService;
use crate::services::process_service::CommandSpec;
use crate::services::task_service::TaskService;

/// adb 单次调用的输出（短命令 capture 模式）
#[derive(Debug, Clone, Default)]
pub struct AdbRunOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// 一条端口转发规则（forward --list 行）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardRule {
    pub serial: String,
    pub local: String,
    pub remote: String,
}

/// AdbRunner：全部 adb 基础指令的统一 Rust 接口（三端一致，差异只进实现）。
#[async_trait]
pub trait AdbRunner: Send + Sync {
    /// args 为不含可执行文件的完整参数（已由 build_args 拼好 -s 前缀）。
    /// 超时后必须杀掉进程并返回错误。
    async fn run(
        &self,
        adb_path: &str,
        args: &[String],
        timeout: Duration,
    ) -> CoreResult<AdbRunOutput>;
    /// 当前生效的 adb 环境（解析结果 + 版本探测），供仪表盘卡片。
    async fn environment(&self) -> AdbEnvironment;
    /// 手动指定/清空 adb 路径后使缓存失效（下次 environment() 重新解析探测）。
    fn invalidate_cache(&self);

    /// 只读缓存里已解析的 adb 路径（无则 None）；供同步快速路径使用。
    fn cached_path(&self) -> Option<String> {
        None
    }
}

/// adb 环境状态（前端仪表盘的 adb 卡片直接消费）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdbEnvironment {
    /// true = 找到 adb 且 `adb version` 探测成功
    pub installed: bool,
    /// 生效的可执行文件路径（installed=false 时为 None）
    pub path: Option<String>,
    /// 来源：config(手动配置) / android_home / sdk_root / path_env
    pub source: Option<String>,
    #[serde(flatten)]
    pub version: Option<AdbVersionInfo>,
    /// 未配置时的用户提示（前端原样展示）
    pub hint: Option<String>,
    /// 探测失败原因（如找到二进制但执行报错）
    pub probe_error: Option<String>,
}

impl AdbEnvironment {
    pub fn not_found() -> Self {
        Self {
            installed: false,
            path: None,
            source: None,
            version: None,
            hint: Some(
                "未检测到 adb：请安装 Android platform-tools 并加入 PATH（或设置 ANDROID_HOME），也可在 设置 → ADB 路径 手动指定。"
                    .to_string(),
            ),
            probe_error: None,
        }
    }
}

const DEFAULT_WATCH_INTERVAL: Duration = Duration::from_secs(3);
const SHORT_CMD_TIMEOUT: Duration = Duration::from_secs(10);
const LIST_TIMEOUT: Duration = Duration::from_secs(8);
/// AR6.2 端口互查：Agent 要扫 `/proc/*/fd` 建 inode→pid 索引，进程数多时比
/// 单条 shell 慢，超时给到 15 s（Legacy 侧同量级：真机非 root 全量 ls 约 2~4 s）。
const PORT_SCAN_TIMEOUT: Duration = Duration::from_secs(15);
/// adb push 大 so 文件用长超时（USB 下数十 MB 也留足余量）
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);
/// su -c cat 覆写（设备内拷贝，磁盘写为主）
const CAT_TIMEOUT: Duration = Duration::from_secs(30);

// ===== 真实实现：直接 tokio 进程 capture =====

pub struct RealAdbRunner {
    config: Arc<ConfigService>,
    cache: Mutex<Option<CachedProbe>>,
}

#[derive(Clone)]
struct CachedProbe {
    resolved: ResolvedAdb,
    version: Option<AdbVersionInfo>,
    probe_error: Option<String>,
}

/// 解析出的 adb 可执行文件与来源
#[derive(Debug, Clone)]
struct ResolvedAdb {
    path: String,
    source: &'static str,
}

impl RealAdbRunner {
    pub fn new(config: Arc<ConfigService>) -> Self {
        Self {
            config,
            cache: Mutex::new(None),
        }
    }

    /// 按优先级解析 adb：手动配置 → ANDROID_HOME → ANDROID_SDK_ROOT → PATH。
    fn resolve(&self) -> Option<ResolvedAdb> {
        if let Ok(p) = self
            .config
            .get(crate::services::config_service::KEY_ADB_PATH, "")
        {
            let p = p.trim().to_string();
            if !p.is_empty() && std::path::Path::new(&p).exists() {
                return Some(ResolvedAdb {
                    path: p,
                    source: "config",
                });
            }
        }
        let candidates = build_candidates();
        candidates
            .into_iter()
            .find(|cand| std::path::Path::new(&cand.path).exists())
    }

    async fn probe(&self, resolved: &ResolvedAdb) -> (Option<AdbVersionInfo>, Option<String>) {
        let args = adb::build_args(None, &adb::cmd_version());
        match self.run(&resolved.path, &args, SHORT_CMD_TIMEOUT).await {
            Ok(out) if out.exit_code == Some(0) => match adb::parse_version(&out.stdout) {
                Some(v) => (Some(v), None),
                None => (
                    None,
                    Some(format!("adb version 输出无法解析: {}", out.stdout.trim())),
                ),
            },
            Ok(out) => (
                None,
                Some(format!(
                    "adb version 退出码 {:?}: {}",
                    out.exit_code,
                    out.stderr.trim()
                )),
            ),
            Err(e) => (None, Some(e.to_string())),
        }
    }
}

/// 候选路径 + 来源标注：ANDROID_HOME → ANDROID_SDK_ROOT → PATH（纯拼接逻辑在 adapter）。
fn build_candidates() -> Vec<ResolvedAdb> {
    let exe = adb::adb_exe_names();
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let home = env("ANDROID_HOME");
    let sdk_root = env("ANDROID_SDK_ROOT");
    let path = env("PATH");

    let home_cands: Vec<String> = home
        .as_deref()
        .map(|h| adb::build_adb_candidates(&exe, Some(h), None, None, std::path::MAIN_SEPARATOR))
        .unwrap_or_default();
    let root_cands: Vec<String> = sdk_root
        .as_deref()
        .map(|h| adb::build_adb_candidates(&exe, Some(h), None, None, std::path::MAIN_SEPARATOR))
        .unwrap_or_default();
    let path_cands =
        adb::build_adb_candidates(&exe, None, None, path.as_deref(), std::path::MAIN_SEPARATOR);

    let mut out = Vec::new();
    out.extend(home_cands.into_iter().map(|p| ResolvedAdb {
        path: p,
        source: "android_home",
    }));
    out.extend(root_cands.into_iter().map(|p| ResolvedAdb {
        path: p,
        source: "sdk_root",
    }));
    out.extend(path_cands.into_iter().map(|p| ResolvedAdb {
        path: p,
        source: "path_env",
    }));
    out
}

#[async_trait]
impl AdbRunner for RealAdbRunner {
    async fn run(
        &self,
        adb_path: &str,
        args: &[String],
        timeout: Duration,
    ) -> CoreResult<AdbRunOutput> {
        let mut cmd = tokio::process::Command::new(adb_path);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            // tokio::process::Command 在 Windows 自带 inherent creation_flags，
            // 不要 std 的 CommandExt（对 tokio Command 无效 → unused import）
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW); // 防止闪黑窗
        }
        let spawned = cmd
            .spawn()
            .map_err(|e| CoreError::Internal(format!("启动 {adb_path} 失败: {e}")))?;
        let output = tokio::time::timeout(timeout, spawned.wait_with_output())
            .await
            .map_err(|_| CoreError::Internal(format!("adb 命令超时（{}s）", timeout.as_secs())))?
            .map_err(|e| CoreError::Internal(format!("等待 adb 失败: {e}")))?;
        Ok(AdbRunOutput {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code(),
        })
    }

    async fn environment(&self) -> AdbEnvironment {
        // 缓存命中直接返回（版本不常变；手动改配置会 invalidate）
        if let Some(hit) = self.cache.lock().expect("adb cache lock").clone() {
            return AdbEnvironment {
                installed: hit.version.is_some(),
                path: Some(hit.resolved.path.clone()),
                source: Some(hit.resolved.source.to_string()),
                version: hit.version.clone(),
                hint: if hit.version.is_some() {
                    None
                } else {
                    hit.probe_error.clone()
                },
                probe_error: hit.probe_error.clone(),
            };
        }
        let Some(resolved) = self.resolve() else {
            return AdbEnvironment::not_found();
        };
        let (version, probe_error) = self.probe(&resolved).await;
        *self.cache.lock().expect("adb cache lock") = Some(CachedProbe {
            resolved: resolved.clone(),
            version: version.clone(),
            probe_error: probe_error.clone(),
        });
        AdbEnvironment {
            installed: version.is_some(),
            path: Some(resolved.path.clone()),
            source: Some(resolved.source.to_string()),
            version,
            hint: if probe_error.is_some() {
                Some("已找到 adb 但版本探测失败，详情见错误信息".to_string())
            } else {
                None
            },
            probe_error,
        }
    }

    fn invalidate_cache(&self) {
        *self.cache.lock().expect("adb cache lock") = None;
    }

    fn cached_path(&self) -> Option<String> {
        self.cache
            .lock()
            .ok()
            .and_then(|c| c.as_ref().map(|p| p.resolved.path.clone()))
    }
}

// ===== Mock 实现：CI / 无真机验证（仅测试构造） =====

/// 脚本化 mock：按子命令关键字匹配返回预置输出。
#[cfg(test)]
pub struct MockAdbRunner {
    /// (args 包含任意关键字 => 输出) 按顺序匹配；未命中返回 exit_code=-1
    scripts: Vec<(Vec<String>, AdbRunOutput)>,
    available: bool,
}

#[cfg(test)]
impl MockAdbRunner {
    pub fn new(available: bool) -> Self {
        Self {
            scripts: Vec::new(),
            available,
        }
    }

    pub fn with_script(mut self, keywords: &[&str], out: AdbRunOutput) -> Self {
        self.scripts
            .push((keywords.iter().map(|s| s.to_string()).collect(), out));
        self
    }

    pub fn ok_output(stdout: &str) -> AdbRunOutput {
        AdbRunOutput {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: Some(0),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl AdbRunner for MockAdbRunner {
    async fn run(
        &self,
        _adb_path: &str,
        args: &[String],
        _timeout: Duration,
    ) -> CoreResult<AdbRunOutput> {
        if !self.available {
            return Err(CoreError::Internal("adb 不存在（mock）".into()));
        }
        for (keys, out) in &self.scripts {
            if keys
                .iter()
                .all(|k| args.iter().any(|a| a.contains(k.as_str())))
            {
                return Ok(out.clone());
            }
        }
        Ok(AdbRunOutput {
            stdout: String::new(),
            stderr: "mock: unmatched args".into(),
            exit_code: Some(-1),
        })
    }

    async fn environment(&self) -> AdbEnvironment {
        if !self.available {
            return AdbEnvironment::not_found();
        }
        AdbEnvironment {
            installed: true,
            path: Some("/mock/adb".into()),
            source: Some("mock".into()),
            version: Some(AdbVersionInfo {
                version: "1.0.41".into(),
                build: "mock".into(),
            }),
            hint: None,
            probe_error: None,
        }
    }

    fn invalidate_cache(&self) {}
}

// ===== DeviceService =====

/// 设备热插拔事件 payload（ipc-conventions §4：device://changed）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceChangedPayload {
    pub serial: String,
    pub transport: String,
    /// true = 上线/状态变化，false = 掉线
    pub present: bool,
    pub state: String,
    pub last_seen: i64,
}

pub struct DeviceService {
    runner: Arc<dyn AdbRunner>,
    android: Arc<CapabilityRouter>,
    tasks: Arc<TaskService>,
    db: Arc<Db>,
    app: tauri::AppHandle,
    /// 最近一次轮询到的设备快照（serial → state），watch diff 用
    known: Mutex<HashMap<String, String>>,
    watch_started: Mutex<bool>,
}

impl DeviceService {
    pub fn new(
        runner: Arc<dyn AdbRunner>,
        android: Arc<CapabilityRouter>,
        tasks: Arc<TaskService>,
        db: Arc<Db>,
        app: tauri::AppHandle,
    ) -> Self {
        Self {
            runner,
            android,
            tasks,
            db,
            app,
            known: Mutex::new(HashMap::new()),
            watch_started: Mutex::new(false),
        }
    }

    /// 仪表盘：adb 环境状态
    pub async fn environment(&self) -> AdbEnvironment {
        self.runner.environment().await
    }

    /// 设置页修改 adb 路径后调用：失效缓存，立即重新解析
    pub async fn reprobe(&self) -> AdbEnvironment {
        self.runner.invalidate_cache();
        *self.known.lock().expect("known lock") = HashMap::new();
        self.runner.environment().await
    }

    async fn run_adb(&self, args: &[String]) -> CoreResult<AdbRunOutput> {
        self.run_adb_with(args, LIST_TIMEOUT).await
    }

    /// 带自定义超时的 adb 短命令（大文件 push 等慢操作用长档）
    async fn run_adb_with(&self, args: &[String], timeout: Duration) -> CoreResult<AdbRunOutput> {
        self.android.legacy().run(args, timeout).await
    }

    async fn run_transport_adb(&self, args: &[String]) -> CoreResult<AdbRunOutput> {
        let env = self.runner.environment().await;
        let path = env
            .path
            .ok_or_else(|| CoreError::Internal(env.hint.unwrap_or_else(|| "adb 不可用".into())))?;
        self.runner.run(&path, args, LIST_TIMEOUT).await
    }

    /// 设备列表（adb devices -l）
    pub async fn list_devices(&self) -> CoreResult<Vec<DeviceEntry>> {
        let args = adb::build_args(None, &adb::cmd_devices());
        let out = self.run_transport_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb devices 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(adb::parse_devices(&out.stdout))
    }

    /// 设备信息（getprop 常用字段 + wlan0 IP）
    pub async fn device_info(&self, serial: &str) -> CoreResult<DeviceInfo> {
        let route = self
            .android
            .select(serial, DEVICE_INFO, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.device_info_legacy(serial).await;
        }

        let agent_request = self.android.agent().request::<_, DeviceInfoResult>(
            serial,
            DEVICE_INFO,
            &DeviceInfoParams {},
            LIST_TIMEOUT,
        );
        let (agent_result, legacy_result) = if device_info_shadow_enabled() {
            let legacy_request = self.device_info_legacy(serial);
            let (agent_result, legacy_result) = tokio::join!(agent_request, legacy_request);
            (agent_result, Some(legacy_result))
        } else {
            (agent_request.await, None)
        };
        match agent_result {
            Ok(result) => {
                let info = map_agent_device_info(serial, result);
                if let Some(Ok(legacy)) = legacy_result {
                    log_device_info_shadow_diff(serial, &info, &legacy);
                }
                Ok(info)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        DEVICE_INFO,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                match legacy_result {
                    Some(result) => result,
                    None => self.device_info_legacy(serial).await,
                }
            }
        }
    }

    async fn device_info_legacy(&self, serial: &str) -> CoreResult<DeviceInfo> {
        let args = adb::build_args(Some(serial), &adb::cmd_getprop());
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "getprop 失败: {}",
                out.stderr.trim()
            )));
        }
        let mut info = adb::device_info_from_props(serial, &adb::parse_getprop(&out.stdout));
        // IP 读取尽力而为：失败（未连 Wi-Fi/旧 ROM）不影响 info 其他字段
        info.ip = self.device_ip_legacy(serial).await.ok().flatten();
        Ok(info)
    }

    /// 设备侧目录列表（短命令 ls -lA）
    pub async fn list_files(&self, serial: &str, path: &str) -> CoreResult<Vec<FileEntry>> {
        let args = adb::build_args(Some(serial), &adb::cmd_ls(path));
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "ls 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(out.stdout.lines().filter_map(adb::parse_ls_long).collect())
    }

    /// 第三方应用列表
    /// 三方包列表（保持既有 command 语义）。AR5.5 起默认走 Agent `package.list`，
    /// 解析在设备端完成；Agent 不可用或旧版 Agent 时才回退 Legacy ADB（只读幂等，
    /// 删除条件见 Legacy 能力表）。
    pub async fn list_packages(&self, serial: &str) -> CoreResult<Vec<String>> {
        let route = self
            .android
            .select(serial, PACKAGE_LIST, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.list_packages_legacy(serial).await;
        }

        let agent_request = self.android.agent().request::<_, PackageListResult>(
            serial,
            PACKAGE_LIST,
            &PackageListParams {
                scope: PackageScope::User,
                include_disabled: false,
            },
            LIST_TIMEOUT,
        );
        let (agent_result, legacy_result) = if package_list_shadow_enabled() {
            let legacy = self.list_packages_legacy(serial);
            let (agent, legacy) = tokio::join!(agent_request, legacy);
            (agent, Some(legacy))
        } else {
            (agent_request.await, None)
        };

        match agent_result {
            Ok(result) => {
                let names = package_names(&result);
                if let Some(Ok(legacy)) = legacy_result {
                    log_package_list_shadow_diff(serial, &names, &legacy);
                }
                Ok(names)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        PACKAGE_LIST,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                match legacy_result {
                    Some(result) => result,
                    None => self.list_packages_legacy(serial).await,
                }
            }
        }
    }

    async fn list_packages_legacy(&self, serial: &str) -> CoreResult<Vec<String>> {
        let args = adb::build_args(Some(serial), &adb::cmd_list_packages(true));
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "pm list packages 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(adb::parse_packages(&out.stdout))
    }

    /// 设备 wlan0 IPv4：`adb -s <serial> shell ip addr show wlan0`
    /// （用户指定命令；解析不到返回 None——未连 Wi-Fi / 双卡数据流量）
    pub async fn device_ip(&self, serial: &str) -> CoreResult<Option<String>> {
        self.device_ip_legacy(serial).await
    }

    async fn device_ip_legacy(&self, serial: &str) -> CoreResult<Option<String>> {
        let args = adb::build_args(Some(serial), &adb::cmd_ip_addr());
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "ip addr 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(adb::parse_wlan0_ip(&out.stdout))
    }

    // ===== 端口转发管理（P9：ADB 页端口转发 tab；全部调用 -s 绑定设备）=====

    /// 建立转发规则。返回后端实际规则行（serial, local, remote）。
    /// 裸数字规格自动按 tcp: 处理（用户只填端口的习惯输入）。
    pub async fn forward_setup(
        &self,
        serial: &str,
        local: &str,
        remote: &str,
    ) -> CoreResult<(String, String, String)> {
        let local = adb::normalize_forward_spec(local);
        let remote = adb::normalize_forward_spec(remote);
        if !adb::is_valid_forward_spec(&local) || !adb::is_valid_forward_spec(&remote) {
            return Err(CoreError::Internal(format!(
                "转发规格非法（允许 tcp:1-65535 / localabstract:name / localreserved:name）: {local} → {remote}"
            )));
        }
        let args = adb::build_args(Some(serial), &adb::cmd_forward(&local, &remote));
        let out = self.run_transport_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb forward 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok((serial.to_string(), local, remote))
    }

    /// 列出该设备当前全部转发规则。
    pub async fn forward_list(&self, serial: &str) -> CoreResult<Vec<ForwardRule>> {
        let args = adb::build_args(Some(serial), &adb::cmd_forward_list());
        let out = self.run_transport_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb forward --list 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(adb::parse_forward_list(&out.stdout)
            .into_iter()
            .map(|(s, l, r)| ForwardRule {
                serial: s,
                local: l,
                remote: r,
            })
            .collect())
    }

    /// 删除一条转发（local=None 删全部）。裸数字规格自动按 tcp: 处理。
    pub async fn forward_remove(&self, serial: &str, local: Option<&str>) -> CoreResult<()> {
        let local = local.map(adb::normalize_forward_spec);
        if let Some(l) = &local {
            if !adb::is_valid_forward_spec(l) {
                return Err(CoreError::Internal(format!("转发规格非法: {l}")));
            }
        }
        let args = adb::build_args(Some(serial), &adb::cmd_forward_remove(local.as_deref()));
        let out = self.run_transport_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb forward --remove 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    // ===== 二进制托管（P9：/data/local/tmp 下 ELF 的浏览 / chmod / 后台运行 / kill）=====

    /// 列出托管目录下的 ELF 可执行文件：`ls -l` 拿权限 + `file <dir>/*` 判 ELF。
    /// file 命令本身不可用时报错（不猜测——避免把文本文件当二进制展示）。
    pub async fn hosted_binaries(&self, serial: &str) -> CoreResult<Vec<adb::HostedBinary>> {
        let dir = adb::HOSTED_DIR;
        let ls_args = adb::build_args(Some(serial), &adb::cmd_ls(dir));
        let ls_out = self.run_adb(&ls_args).await?;
        if ls_out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "读取 {dir} 失败: {}",
                ls_out.stderr.trim()
            )));
        }
        let file_cmd = format!("file {dir}/*");
        let file_args = adb::build_args(Some(serial), &adb::cmd_shell(&file_cmd));
        let file_out = self.run_adb(&file_args).await?;
        // file 缺失（exit!=0 且输出含 not found 类）时给可读错误
        if file_out.exit_code != Some(0) && !file_out.stdout.contains(':') {
            return Err(CoreError::Internal(format!(
                "设备不支持 file 命令，无法识别 ELF: {}",
                file_out.stderr.trim()
            )));
        }
        Ok(adb::hosted_binaries(&ls_out.stdout, &file_out.stdout))
    }

    /// 探测设备 su 是否可用（`su -c id` 输出含 uid=0）。
    /// 设备信息卡 Root 横幅与二进制托管 Root 开关共用此链路。
    pub async fn su_available(&self, serial: &str) -> CoreResult<bool> {
        let cmd = adb::su_wrap("id");
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&cmd));
        let out = self.run_adb(&args).await?;
        Ok(out.exit_code == Some(0) && adb::is_root_probe_ok(&out.stdout))
    }

    /// 赋予执行权限：`chmod +x <dir>/<name>`（name 过安全白名单校验，防注入）。
    /// root=true 时整体走 su -c（root 属主的文件 shell 用户 chmod 会被拒）。
    pub async fn hosted_chmod(&self, serial: &str, name: &str, root: bool) -> CoreResult<()> {
        let cmd = Self::hosted_shell(name, "chmod +x")?;
        let cmd = if root { adb::su_wrap(&cmd) } else { cmd };
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&cmd));
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "chmod 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// 后台启动并返回 pid。⚠️ 用 `;` 而非 `&&`：`&&` 的优先级低于 `&`，
    /// 会把整个 `cd && nohup` 复合式后台化，`$!` 拿到的是子 shell pid 而非
    /// 二进制 pid（kill/复查就全错了）；`;` 确保只有 nohup 一段进后台，
    /// nohup exec 后 pid 即二进制 pid。
    /// root=true 时整段经 su -c '…' 单引号包裹：外层 shell 不动 `&`/`$!`/重定向，
    /// 由 root 内层 shell 解释（否则 su 只收到 `cd`）。
    /// stdout/stderr 落盘 `.<name>.run.log`（隐藏文件不污染 file 列表）：
    /// 秒退真因（CANNOT LINK / exec format / Permission denied）多在 stderr，
    /// 复查失败时读日志尾部给出真实死因。
    pub async fn hosted_run(&self, serial: &str, name: &str, root: bool) -> CoreResult<u32> {
        if !adb::is_safe_hosted_name(name) {
            return Err(CoreError::Internal(format!(
                "文件名非法（仅允许字母数字与 _.-，且不以 . 开头）: {name}"
            )));
        }
        let log = adb::hosted_run_log(name);
        let run_cmd = adb::hosted_run_cmd(name, &log, root);
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&run_cmd));
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "启动失败: {}",
                out.stderr.trim()
            )));
        }
        let pid = adb::parse_run_pid(&out.stdout).ok_or_else(|| {
            CoreError::Internal(format!(
                "未能解析启动 pid，输出: {}{}",
                out.stdout.trim(),
                if out.stderr.trim().is_empty() {
                    String::new()
                } else {
                    format!("（stderr: {}）", out.stderr.trim())
                }
            ))
        })?;
        // 存活复查（nohup 秒退场景）。root 启动的进程 shell 用户 kill -0 会
        // 得 EPERM 误判死亡，复查必须与启动同一身份。
        let check = {
            let c = format!("sleep 0.3; kill -0 {pid} 2>/dev/null && echo alive || echo dead");
            let cmd = if root { adb::su_wrap(&c) } else { c };
            let args = adb::build_args(Some(serial), &adb::cmd_shell(&cmd));
            self.run_adb(&args).await?
        };
        if check.stdout.trim() != "alive" {
            let cause = self
                .read_hosted_log(serial, &log, root)
                .await
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let detail = adb::run_log_diagnostics(&cause)
                .unwrap_or_else(|| format!("（无输出可参考；完整日志见 {log}）"));
            return Err(CoreError::Internal(format!(
                "{name} 启动后立即退出：{detail}"
            )));
        }
        Ok(pid)
    }

    /// 读托管启动日志尾部（tail 截 2KB 防日志爆炸；root 启动的日志同身份读）。
    async fn read_hosted_log(
        &self,
        serial: &str,
        log_path: &str,
        root: bool,
    ) -> CoreResult<String> {
        let c = format!("tail -c 2048 {log_path} 2>/dev/null");
        let cmd = if root { adb::su_wrap(&c) } else { c };
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&cmd));
        let out = self.run_adb(&args).await?;
        Ok(out.stdout)
    }

    /// 终止进程（AR6.3，写操作）。路由规则与只读能力**不同**：
    ///
    /// - `root=false` → Agent `process.kill`，Agent 不可用即失败，
    ///   **绝不自动回退 Legacy**（§3.6：写操作重复执行会留下半途状态）；
    /// - `root=true` → 继续 Legacy `su -c kill -9`，因为 Agent 由 adb shell 以 shell
    ///   身份启动，对 root 属主进程只会 EPERM（同 D026 的身份边界）；
    /// - 两条路径都写 `target = "audit"` 的结构化审计行（§3.7），成功与失败都记。
    ///
    /// `expected_comm` 是 PID 复用防护：Agent 在发信号前重读 `/proc/<pid>` 身份，
    /// 对不上以 `precondition_failed` 拒止；前端把列表里显示的进程名带过来即可。
    pub async fn process_kill(
        &self,
        serial: &str,
        pid: u32,
        expected_comm: Option<String>,
        root: bool,
    ) -> CoreResult<()> {
        if root {
            let result = self.hosted_kill_legacy(serial, pid).await;
            audit_process_kill(
                serial,
                pid,
                expected_comm.as_deref(),
                "legacy_adb",
                &result
                    .as_ref()
                    .map(|_| "ok".to_string())
                    .map_err(|error| error.to_string()),
            );
            return result;
        }
        // 写操作要求 Agent 在线：这里不静默安装，也不回退 adb shell，
        // 而是给可执行指引（设备页 → 安装/连接 Agent），避免用户以为进程已被杀。
        if !matches!(
            self.android.agent_status(serial).state,
            crate::models::agent::AgentSessionState::Ready
                | crate::models::agent::AgentSessionState::Degraded
        ) {
            let error = CoreError::AgentUnavailable(
                "终止进程需要 Agent 在线（设备页 → 安装/连接 Agent）；写操作不自动回退 adb shell"
                    .to_string(),
            );
            audit_process_kill(
                serial,
                pid,
                expected_comm.as_deref(),
                "agent_unavailable",
                &Err(error.to_string()),
            );
            return Err(error);
        }
        let route = self
            .android
            .select(serial, PROCESS_KILL, OperationKind::Mutating)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend != AndroidBackendSource::Agent {
            // 路由层已禁止 Mutating 回退；真走到这里说明注册表被改坏，宁可拒执行
            let error = CoreError::Internal(
                "process.kill 只允许 Agent 通道，拒绝在非 Agent 后端执行写操作".to_string(),
            );
            audit_process_kill(
                serial,
                pid,
                expected_comm.as_deref(),
                "rejected",
                &Err(error.to_string()),
            );
            return Err(error);
        }
        let params = ProcessKillParams {
            pid,
            expected_comm: expected_comm.clone(),
            signal: KillSignal::Kill,
            require_root: false,
        };
        let result = self
            .android
            .agent()
            .request::<_, ProcessKillResult>(serial, PROCESS_KILL, &params, SHORT_CMD_TIMEOUT)
            .await;
        let summary = match &result {
            Ok(value) => Ok(match value.outcome {
                agent_protocol::KillOutcome::Signaled => {
                    format!("signaled verified_dead={}", value.verified_dead)
                }
                agent_protocol::KillOutcome::AlreadyGone => "already_gone".to_string(),
            }),
            Err(error) => Err(error.to_string()),
        };
        audit_process_kill(serial, pid, expected_comm.as_deref(), "agent", &summary);
        result
            .map(|_| ())
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)))
    }

    /// Legacy 终止路径（仅 root 支路使用）：`su -c kill -9 <pid>`。
    async fn hosted_kill_legacy(&self, serial: &str, pid: u32) -> CoreResult<()> {
        let cmd = adb::su_wrap(&format!("kill -9 {pid}"));
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&cmd));
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "kill 失败（进程可能已退出）: {}",
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// 兼容入口：托管页仍按 (pid, root) 调用，内部转 `process_kill`。
    pub async fn hosted_kill(&self, serial: &str, pid: u32, root: bool) -> CoreResult<()> {
        self.process_kill(serial, pid, None, root).await
    }

    /// 查托管进程监听端口：
    /// `for i in $(ls -l /proc/<pid>/fd | sed -n ...socket...); do grep "$i" /proc/net/tcp /proc/net/tcp6; done`
    /// （用户指定命令；root 启动的进程同身份查询）。输出经 parse_listening_ports
    /// 还原十六进制端口/IP，只返回 LISTEN 态、去重升序。
    pub async fn hosted_ports(
        &self,
        serial: &str,
        pid: u32,
        root: bool,
    ) -> CoreResult<Vec<adb::ListenPort>> {
        let c = adb::hosted_ports_cmd(pid);
        let cmd = if root { adb::su_wrap(&c) } else { c };
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&cmd));
        let out = self.run_adb(&args).await?;
        // grep 无匹配 exit=1 是常态（进程没监听端口），不当错误
        Ok(adb::parse_listening_ports(&out.stdout))
    }

    /// 查任意进程监听端口（PID→端口方向）。AR6.2 起默认走 Agent `process.ports`：
    /// fd→inode 与 `/proc/net/tcp(6)` 的匹配在设备端一次快照完成，Desktop 不再
    /// cat 全文 + 宿主解析（只读幂等，可回退，删除条件见 Legacy 能力表）。
    ///
    /// ⚠️ `root=true` 仍走 Legacy `su -c`：Agent 由 adb shell 以 shell 身份启动，
    /// 读不到别人 uid 的 `/proc/<pid>/fd`，若把它路由过去，「只有 root 看得见」的
    /// 端口会凭空消失——那正是「空列表当成没端口」的假象。等价性优先于入口统一（D026），
    /// 等 Agent 有自己的 root 通道后再收敛这条分支。
    pub async fn process_ports(
        &self,
        serial: &str,
        pid: u32,
        root: bool,
    ) -> CoreResult<Vec<adb::ListenPort>> {
        if root {
            return self.process_ports_legacy(serial, pid, true).await;
        }
        let route = self
            .android
            .select(serial, PROCESS_PORTS, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.process_ports_legacy(serial, pid, false).await;
        }

        let params = ProcessPortsParams { pid };
        let agent_request = self.android.agent().request::<_, ProcessPortsResult>(
            serial,
            PROCESS_PORTS,
            &params,
            PORT_SCAN_TIMEOUT,
        );
        let (agent_result, legacy_result) = if process_ports_shadow_enabled() {
            let legacy = self.process_ports_legacy(serial, pid, false);
            let (agent, legacy) = tokio::join!(agent_request, legacy);
            (agent, Some(legacy))
        } else {
            (agent_request.await, None)
        };

        match agent_result {
            Ok(result) => {
                if !result.unreadable.is_empty() {
                    tracing::debug!(
                        serial,
                        pid,
                        method = PROCESS_PORTS,
                        unreadable = ?result.unreadable,
                        truncated = result.truncated,
                        "process.ports 有不可读项：空列表不等于无监听端口"
                    );
                }
                let ports = map_agent_listening_ports(&result.ports);
                if let Some(Ok(legacy)) = legacy_result {
                    log_process_ports_shadow_diff(serial, pid, &ports, &legacy);
                }
                Ok(ports)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        PROCESS_PORTS,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                tracing::warn!(serial, pid, "process.ports 回退 Legacy ADB");
                match legacy_result {
                    Some(result) => result,
                    None => self.process_ports_legacy(serial, pid, false).await,
                }
            }
        }
    }

    /// Legacy 路径（仅作回退）：单条 shell 里 `ls -l` fd + grep `/proc/net`，
    /// 十六进制还原在宿主侧 `adb::parse_listening_ports` 完成。
    async fn process_ports_legacy(
        &self,
        serial: &str,
        pid: u32,
        root: bool,
    ) -> CoreResult<Vec<adb::ListenPort>> {
        self.hosted_ports(serial, pid, root).await
    }

    /// 端口→PID 反查。AR6.2 起默认走 Agent `process.by_port`：读 `/proc/net/*`
    /// 与 `/proc/<pid>/fd` 符号链接、inode→pid 归属匹配全在设备端一次完成，
    /// Desktop 不再拉 `/proc/net` 全文到宿主，也不再 `ls -l /proc/[0-9]*/fd`。
    ///
    /// ⚠️ 不开 root 时 shell 用户读不到别人的 `/proc/<pid>/fd`，无论 Agent 还是
    /// Legacy 都只能命中 shell 自属进程，前端默认引导勾选 Root 的语义保持不变；
    /// `root=true` 与 PID→端口方向同理继续走 Legacy `su -c`（见 D026）。
    pub async fn pids_by_port(
        &self,
        serial: &str,
        port: u16,
        root: bool,
    ) -> CoreResult<Vec<adb::PortHolder>> {
        if root {
            return self.pids_by_port_legacy(serial, port, true).await;
        }
        let route = self
            .android
            .select(serial, PROCESS_BY_PORT, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.pids_by_port_legacy(serial, port, false).await;
        }

        let params = ProcessByPortParams { port };
        let agent_request = self.android.agent().request::<_, ProcessByPortResult>(
            serial,
            PROCESS_BY_PORT,
            &params,
            PORT_SCAN_TIMEOUT,
        );
        let (agent_result, legacy_result) = if process_ports_shadow_enabled() {
            let legacy = self.pids_by_port_legacy(serial, port, false);
            let (agent, legacy) = tokio::join!(agent_request, legacy);
            (agent, Some(legacy))
        } else {
            (agent_request.await, None)
        };

        match agent_result {
            Ok(result) => {
                let holders = map_agent_port_holders(&result);
                if let Some(Ok(legacy)) = legacy_result {
                    log_pids_by_port_shadow_diff(serial, port, &holders, &result.unowned, &legacy);
                }
                Ok(holders)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        PROCESS_BY_PORT,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                tracing::warn!(serial, port, "process.by_port 回退 Legacy ADB");
                match legacy_result {
                    Some(result) => result,
                    None => self.pids_by_port_legacy(serial, port, false).await,
                }
            }
        }
    }

    /// Legacy 路径（仅作回退，三步）：
    /// ① grep 端口十六进制 → 解析取 LISTEN 且端口精确匹配的 inode；
    /// ② inode_scan_cmd 单次 `ls -l /proc/[0-9]*/fd` → 宿主侧 parse_fd_scan
    ///    找持有这些 inode 的 pid（绝不在设备端逐进程循环——真机数百次
    ///    ls/grep 孵化实测超过 8s 命令超时）；
    /// ③ comm_batch_cmd 仅对命中的少量 pid 批量取进程名。
    async fn pids_by_port_legacy(
        &self,
        serial: &str,
        port: u16,
        root: bool,
    ) -> CoreResult<Vec<adb::PortHolder>> {
        let wrap = |c: String| if root { adb::su_wrap(&c) } else { c };

        let args = adb::build_args(
            Some(serial),
            &adb::cmd_shell(&wrap(adb::port_grep_cmd(port))),
        );
        let out = self.run_adb(&args).await?;
        let inodes: std::collections::HashSet<u64> = adb::parse_proc_net_entries(&out.stdout)
            .into_iter()
            .filter(|e| e.listen && e.listen_port.port == port && e.inode != 0)
            .map(|e| e.inode)
            .collect();
        if inodes.is_empty() {
            return Ok(Vec::new());
        }

        let args = adb::build_args(Some(serial), &adb::cmd_shell(&wrap(adb::inode_scan_cmd())));
        let out = self.run_adb(&args).await?;
        let holder_pids = adb::parse_fd_scan(&out.stdout, &inodes);
        if holder_pids.is_empty() {
            return Ok(Vec::new());
        }

        let args = adb::build_args(
            Some(serial),
            &adb::cmd_shell(&wrap(adb::comm_batch_cmd(&holder_pids))),
        );
        let out = self.run_adb(&args).await?;
        Ok(adb::parse_port_holders(&out.stdout))
    }

    /// 查询包安装 lib 目录（so 替换 UI 预览用，只读）：
    /// dumpsys package → legacyNativeLibraryDir → 按 ABI 换 arm64/arm 尾段。
    pub async fn pkg_lib_dir(&self, serial: &str, pkg: &str, abi: &str) -> CoreResult<String> {
        if !adb::is_safe_pkg_name(pkg) {
            return Err(CoreError::Internal(format!("包名非法: {pkg}")));
        }
        if abi != "arm64" && abi != "arm" {
            return Err(CoreError::Internal(format!("ABI 仅支持 arm64/arm: {abi}")));
        }
        let args = adb::build_args(
            Some(serial),
            &adb::cmd_shell(&format!("dumpsys package {pkg}")),
        );
        let out = self.run_adb(&args).await?;
        let legacy = crate::services::env_service::parse_legacy_native_lib(&out.stdout)
            .ok_or_else(|| {
                CoreError::Internal(format!(
                    "未找到 {pkg} 的 legacyNativeLibraryDir（未安装？）"
                ))
            })?;
        adb::lib_dir_for_abi(&legacy, abi).ok_or_else(|| {
            CoreError::Internal(format!("lib 目录结构异常，无法按 ABI {abi} 解析: {legacy}"))
        })
    }

    /// so 替换（免重打包）：主机侧修补好的 .so 直接写回 APK 安装目录。
    /// 三步（用户指定底层流程）：
    /// ① `adb push <local> /data/local/tmp/<name>`；
    /// ② dumpsys package 查 legacyNativeLibraryDir，按所选 ABI 拼
    ///    `.../lib/arm64`（64 位）或 `.../lib/arm`（32 位）目录，
    ///    `su -c 'cat <tmp> > <target>'` 以 root 覆写；
    /// ③ `rm <tmp>` 清理临时文件（尽力而为，失败不影响结果）。
    /// 返回实际写入的目标路径。前置：设备必须已 root（cat 步写 /data/app）。
    pub async fn so_replace(
        &self,
        serial: &str,
        local: &std::path::Path,
        pkg: &str,
        abi: &str,
    ) -> CoreResult<String> {
        if !adb::is_safe_pkg_name(pkg) {
            return Err(CoreError::Internal(format!("包名非法: {pkg}")));
        }
        if abi != "arm64" && abi != "arm" {
            return Err(CoreError::Internal(format!(
                "ABI 仅支持 arm64(64位)/arm(32位): {abi}"
            )));
        }
        let name = local
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| CoreError::Internal("本地文件路径无法解析文件名".into()))?;
        if !adb::is_safe_hosted_name(name) || !name.ends_with(".so") {
            return Err(CoreError::Internal(format!(
                "文件名需为 .so 且不含空格/特殊字符: {name}"
            )));
        }
        if !local.is_file() {
            return Err(CoreError::Internal(format!(
                "本地文件不存在: {}",
                local.display()
            )));
        }
        // 前置检查：cat 步要 root 写 /data/app——先探测 su，省得半途失败
        if !self.su_available(serial).await? {
            return Err(CoreError::Internal(
                "so 替换需要 root（su -c cat 写安装目录），设备 su 不可用".into(),
            ));
        }

        // 目标路径：dumpsys package 查 legacyNativeLibraryDir → 换 abi 子目录
        let ds_args = adb::build_args(
            Some(serial),
            &adb::cmd_shell(&format!("dumpsys package {pkg}")),
        );
        let ds = self.run_adb(&ds_args).await?;
        let legacy =
            crate::services::env_service::parse_legacy_native_lib(&ds.stdout).ok_or_else(|| {
                CoreError::Internal(format!(
                    "未找到 {pkg} 的 legacyNativeLibraryDir（应用未安装？包名拼错？）"
                ))
            })?;
        let target = adb::so_target_path(&legacy, abi, name)
            .ok_or_else(|| CoreError::Internal(format!("无法按 ABI {abi} 拼目标路径: {legacy}")))?;
        let tmp = format!("{}/{}", adb::HOSTED_DIR, name);
        if !adb::is_safe_android_path(&tmp) || !adb::is_safe_android_path(&target) {
            return Err(CoreError::Internal("设备侧路径含非法字符，已中止".into()));
        }

        // ① push 到临时目录（长超时档）
        let push_args = adb::build_args(
            Some(serial),
            &adb::cmd_push(&local.display().to_string(), &tmp),
        );
        let out = self.run_adb_with(&push_args, PUSH_TIMEOUT).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb push 失败: {}",
                out.stderr.trim()
            )));
        }

        // ② su -c 'cat tmp > target'（整段单引号包裹，重定向归 root shell）
        let cat_cmd = adb::su_wrap(&format!("cat {tmp} > {target}"));
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&cat_cmd));
        let out = self.run_adb_with(&args, CAT_TIMEOUT).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "cat 写入失败: {}",
                out.stderr.trim()
            )));
        }

        // ③ 清理临时文件（尽力而为）
        let rm_args = adb::build_args(Some(serial), &adb::cmd_shell(&format!("rm {tmp}")));
        if let Err(e) = self.run_adb(&rm_args).await {
            tracing::warn!(error = %e, tmp, "so_replace 临时文件清理失败（可忽略）");
        }

        tracing::info!(serial, pkg, abi, target = %target, "so 替换完成");
        Ok(target)
    }

    /// 拼 `<verb> <dir>/<name>` 并做名称安全校验（所有托管文件操作共用入口）。
    fn hosted_shell(name: &str, verb: &str) -> CoreResult<String> {
        if !adb::is_safe_hosted_name(name) {
            return Err(CoreError::Internal(format!(
                "文件名非法（仅允许字母数字与 _.-，且不以 . 开头）: {name}"
            )));
        }
        Ok(format!("{verb} {}/{}", adb::HOSTED_DIR, name))
    }

    // ===== 长操作：全部生成 TaskService 任务（事件流 + 历史）=====

    async fn adb_task(&self, serial: Option<&str>, subcommand: &[String]) -> CoreResult<String> {
        let env = self.runner.environment().await;
        let path = env
            .path
            .ok_or_else(|| CoreError::Internal(env.hint.unwrap_or_else(|| "adb 不可用".into())))?;
        let spec = CommandSpec {
            executable: path,
            args: adb::build_args(serial, subcommand),
            cwd: None,
            timeout: None, // 长任务不设超时，由用户取消
            env_extra: HashMap::new(),
            ..Default::default()
        };
        self.tasks.start(spec)
    }

    pub async fn start_shell(&self, serial: &str, command: &str) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_shell(command)).await
    }

    pub async fn start_install(&self, serial: &str, local_apk: &str) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_install(local_apk))
            .await
    }

    pub async fn start_uninstall(&self, serial: &str, pkg: &str) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_uninstall(pkg)).await
    }

    pub async fn start_launch(&self, serial: &str, pkg: &str) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_launch(pkg)).await
    }

    pub async fn start_force_stop(&self, serial: &str, pkg: &str) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_force_stop(pkg)).await
    }

    pub async fn start_push(&self, serial: &str, local: &str, remote: &str) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_push(local, remote))
            .await
    }

    pub async fn start_pull(&self, serial: &str, remote: &str, local: &str) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_pull(remote, local))
            .await
    }

    pub async fn start_logcat(&self, serial: &str, filter: Option<&str>) -> CoreResult<String> {
        self.adb_task(Some(serial), &adb::cmd_logcat(filter)).await
    }

    // ===== 热插拔轮询（仪表盘「新开线程轮询 adb 链接」）=====

    /// 启动后台轮询（幂等，只生效一次）。独立 tokio 任务，间隔 3s，
    /// 不阻塞任何 IPC；结果经 device://changed 事件推给前端。
    pub fn start_watch(self: &Arc<Self>) {
        {
            let mut started = self.watch_started.lock().expect("watch flag lock");
            if *started {
                return;
            }
            *started = true;
        }
        let svc = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                if let Err(e) = svc.poll_once().await {
                    tracing::debug!(error = %e, "watch 轮询异常");
                }
                tokio::time::sleep(DEFAULT_WATCH_INTERVAL).await;
            }
        });
    }

    /// 强制立即轮询一次（手动拉基线用），返回本轮 diff 事件
    pub async fn poll_once_manual(&self) -> CoreResult<Vec<DeviceChangedPayload>> {
        self.poll_once().await
    }

    /// 单次轮询：拉设备列表 → 与已知快照 diff → 发事件。返回本轮 diff。
    async fn poll_once(&self) -> CoreResult<Vec<DeviceChangedPayload>> {
        let env = self.runner.environment().await;
        if !env.installed {
            // adb 不可用：清空已知快照（下次装好会重新报上线），不发噪音事件
            let disconnected: Vec<_> = self
                .known
                .lock()
                .expect("known lock")
                .drain()
                .map(|(serial, _)| serial)
                .collect();
            for serial in disconnected {
                self.android.device_disconnected(&serial).await;
            }
            return Ok(Vec::new());
        }
        let devices = match self.list_devices().await {
            Ok(d) => d,
            Err(e) => {
                tracing::debug!(error = %e, "设备轮询失败（下轮重试）");
                return Ok(Vec::new());
            }
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let events = {
            let mut known = self.known.lock().expect("known lock");
            let mut seen: HashSet<String> = HashSet::new();
            let mut events = Vec::new();
            for d in &devices {
                let prev = known.get(&d.serial);
                if prev != Some(&d.state) {
                    events.push(DeviceChangedPayload {
                        serial: d.serial.clone(),
                        transport: d.transport.clone(),
                        present: true,
                        state: d.state.clone(),
                        last_seen: now,
                    });
                }
                known.insert(d.serial.clone(), d.state.clone());
                seen.insert(d.serial.clone());
            }
            for gone in known
                .keys()
                .filter(|s| !seen.contains(*s))
                .cloned()
                .collect::<Vec<_>>()
            {
                known.remove(&gone);
                events.push(DeviceChangedPayload {
                    serial: gone,
                    transport: String::new(),
                    present: false,
                    state: "offline".into(),
                    last_seen: now,
                });
            }
            events
        };
        for event in &events {
            if !event.present || event.state != "device" {
                self.android.device_disconnected(&event.serial).await;
            }
        }
        self.persist_devices_cache(&devices, now);
        for evt_payload in &events {
            let evt = AppEvent::new(event_names::DEVICE_CHANGED, evt_payload);
            if let Err(e) = self.app.emit(evt.event, &evt) {
                tracing::debug!(error = %e, "推送设备事件失败");
            }
        }
        Ok(events)
    }

    /// devices 表缓存最近一次已知设备（重启后 UI 可先显示历史）
    fn persist_devices_cache(&self, devices: &[DeviceEntry], now: i64) {
        for d in devices {
            let serial = d.serial.clone();
            let transport = d.transport.clone();
            let model = d.model.clone();
            let db = self.db.clone();
            let _ = db.with(move |conn| {
                rusqlite::Connection::execute(
                    conn,
                    "INSERT INTO devices(serial, transport, name, last_seen) VALUES(?1,?2,?3,?4) \
                     ON CONFLICT(serial) DO UPDATE SET transport=excluded.transport, \
                     name=excluded.name, last_seen=excluded.last_seen",
                    rusqlite::params![serial, transport, model, now],
                )?;
                Ok(())
            });
        }
    }
}

fn map_agent_device_info(serial: &str, result: DeviceInfoResult) -> DeviceInfo {
    DeviceInfo {
        model: result.model.unwrap_or_default(),
        manufacturer: result.manufacturer.unwrap_or_default(),
        android_version: result.android_version.unwrap_or_default(),
        sdk_int: result
            .api_level
            .map(|value| value.to_string())
            .unwrap_or_default(),
        serial: serial.to_owned(),
        ip: result.wlan_ipv4,
    }
}

fn package_list_shadow_enabled() -> bool {
    !std::env::var("APP_REVERSE_TOOLS_PACKAGE_LIST_SHADOW")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off"))
}

fn package_names(result: &PackageListResult) -> Vec<String> {
    result
        .items
        .iter()
        .map(|item| item.package_name.clone())
        .collect()
}

/// Agent 与 Legacy 的三方包集合差异只写日志，不影响返回值（迁移期观测用）。
fn log_package_list_shadow_diff(serial: &str, agent: &[String], legacy: &[String]) {
    let agent_set: std::collections::HashSet<&str> = agent.iter().map(String::as_str).collect();
    let legacy_set: std::collections::HashSet<&str> = legacy.iter().map(String::as_str).collect();
    let only_agent: Vec<&str> = agent_set.difference(&legacy_set).copied().collect();
    let only_legacy: Vec<&str> = legacy_set.difference(&agent_set).copied().collect();
    if only_agent.is_empty() && only_legacy.is_empty() {
        tracing::debug!(
            serial,
            method = PACKAGE_LIST,
            "Agent/Legacy package list matched"
        );
    } else {
        tracing::warn!(
            serial,
            method = PACKAGE_LIST,
            agent_only = ?only_agent,
            legacy_only = ?only_legacy,
            "Agent/Legacy package list shadow compare differed"
        );
    }
}

/// §3.7 写操作审计：一条结构化 `target="audit"` 事件，成功与失败都记。
/// 只含 serial/pid/预期身份/通道/结论，不含命令正文、路径与任何令牌。
fn audit_process_kill(
    serial: &str,
    pid: u32,
    expected_comm: Option<&str>,
    backend: &str,
    outcome: &Result<String, String>,
) {
    match outcome {
        Ok(summary) => tracing::info!(
            target: "audit",
            serial,
            method = PROCESS_KILL,
            pid,
            signal = "kill",
            expected_comm = expected_comm.unwrap_or("-"),
            backend,
            outcome = %summary,
            "process.kill 已执行"
        ),
        Err(reason) => tracing::warn!(
            target: "audit",
            serial,
            method = PROCESS_KILL,
            pid,
            signal = "kill",
            expected_comm = expected_comm.unwrap_or("-"),
            backend,
            error = %reason,
            "process.kill 失败"
        ),
    }
}

/// AR6.2 端口互查的 Agent/Legacy 对照开关（默认开，设 0/false/off 关闭）。
fn process_ports_shadow_enabled() -> bool {
    !std::env::var("APP_REVERSE_TOOLS_PROCESS_PORTS_SHADOW")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off"))
}

fn socket_family_label(family: SocketFamily) -> &'static str {
    match family {
        SocketFamily::Ipv4 => "tcp",
        SocketFamily::Ipv6 => "tcp6",
    }
}

/// Agent `process.ports` → 前端既有 `ListenPort[]` 契约：只留 LISTEN、
/// (端口,地址,族) 去重、端口→族→地址升序，与 `adb::parse_listening_ports` 逐项一致。
/// `pub(crate)` 是为了让真机对照测试（agent_manager）能拿同一个映射函数比对 Legacy，
/// 而不是在测试里再抄一份规则。
pub(crate) fn map_agent_listening_ports(ports: &[ListeningPort]) -> Vec<adb::ListenPort> {
    let mut seen = HashSet::new();
    let mut out: Vec<adb::ListenPort> = ports
        .iter()
        .filter(|entry| entry.state == "listen")
        .map(|entry| adb::ListenPort {
            address: entry.address.clone(),
            port: entry.port,
            listen: true,
            family: socket_family_label(entry.family),
        })
        .filter(|entry| seen.insert((entry.port, entry.address.clone(), entry.family)))
        .collect();
    out.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then(a.family.cmp(b.family))
            .then(a.address.cmp(&b.address))
    });
    out
}

/// Agent `process.by_port` → 前端既有 `PortHolder[]` 契约。
/// `pid=0` 是「socket 在、属主查不到」（权限或竞态），Legacy 从不返回 pid=0，
/// 为了不改前端语义这里不编成假进程，只在 shadow 日志里留证据。
fn map_agent_port_holders(result: &ProcessByPortResult) -> Vec<adb::PortHolder> {
    let mut seen = HashSet::new();
    let mut out: Vec<adb::PortHolder> = result
        .sockets
        .iter()
        .filter(|socket| socket.pid != 0)
        .map(|socket| adb::PortHolder {
            pid: socket.pid,
            name: socket
                .comm
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "?".to_string()),
        })
        .filter(|holder| seen.insert(holder.pid))
        .collect();
    out.sort_by_key(|holder| holder.pid);
    out
}

fn log_process_ports_shadow_diff(
    serial: &str,
    pid: u32,
    agent: &[adb::ListenPort],
    legacy: &[adb::ListenPort],
) {
    let key = |entry: &adb::ListenPort| (entry.port, entry.address.clone(), entry.family);
    let agent_set: HashSet<_> = agent.iter().map(key).collect();
    let legacy_set: HashSet<_> = legacy.iter().map(key).collect();
    let only_agent: Vec<_> = agent_set.difference(&legacy_set).collect();
    let only_legacy: Vec<_> = legacy_set.difference(&agent_set).collect();
    if only_agent.is_empty() && only_legacy.is_empty() {
        tracing::debug!(
            serial,
            pid,
            method = PROCESS_PORTS,
            "Agent/Legacy process.ports matched"
        );
    } else {
        tracing::warn!(
            serial,
            pid,
            method = PROCESS_PORTS,
            agent_only = ?only_agent,
            legacy_only = ?only_legacy,
            "Agent/Legacy process.ports shadow compare differed"
        );
    }
}

fn log_pids_by_port_shadow_diff(
    serial: &str,
    port: u16,
    agent: &[adb::PortHolder],
    agent_unowned: &[PortHoldingProcess],
    legacy: &[adb::PortHolder],
) {
    let agent_set: HashSet<u32> = agent.iter().map(|holder| holder.pid).collect();
    let legacy_set: HashSet<u32> = legacy.iter().map(|holder| holder.pid).collect();
    if agent_set == legacy_set {
        tracing::debug!(
            serial,
            port,
            method = PROCESS_BY_PORT,
            unowned = agent_unowned.len(),
            "Agent/Legacy process.by_port matched"
        );
        return;
    }
    tracing::warn!(
        serial,
        port,
        method = PROCESS_BY_PORT,
        agent_only = ?agent_set.difference(&legacy_set).collect::<Vec<_>>(),
        legacy_only = ?legacy_set.difference(&agent_set).collect::<Vec<_>>(),
        unowned = ?agent_unowned
            .iter()
            .map(|socket| socket.inode)
            .collect::<Vec<_>>(),
        "Agent/Legacy process.by_port shadow compare differed"
    );
}

fn device_info_shadow_enabled() -> bool {
    !std::env::var("APP_REVERSE_TOOLS_DEVICE_INFO_SHADOW")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off"))
}

fn log_device_info_shadow_diff(serial: &str, agent: &DeviceInfo, legacy: &DeviceInfo) {
    let mut differing_fields = Vec::new();
    if agent.model.trim() != legacy.model.trim() {
        differing_fields.push("model");
    }
    if agent.manufacturer.trim() != legacy.manufacturer.trim() {
        differing_fields.push("manufacturer");
    }
    if agent.android_version.trim() != legacy.android_version.trim() {
        differing_fields.push("android_version");
    }
    if agent.sdk_int.trim() != legacy.sdk_int.trim() {
        differing_fields.push("sdk_int");
    }
    if agent.serial.trim() != legacy.serial.trim() {
        differing_fields.push("serial");
    }
    if agent.ip != legacy.ip {
        differing_fields.push("ip");
    }
    if differing_fields.is_empty() {
        tracing::debug!(
            serial,
            method = DEVICE_INFO,
            "Agent/Legacy shadow compare matched"
        );
    } else {
        tracing::warn!(
            serial,
            method = DEVICE_INFO,
            fields = ?differing_fields,
            "Agent/Legacy shadow compare differed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    // 测试环境没有 Tauri AppHandle（TaskService 构造需要），因此这里只测
    // runner 抽象层与解析链路——这正是三平台 CI 无真机的核心验证面。
    // DeviceService 的集成行为在 dev 手动回测覆盖。

    fn mock_available() -> MockAdbRunner {
        MockAdbRunner::new(true)
            .with_script(&["version"], MockAdbRunner::ok_output(
                "Android Debug Bridge version 1.0.41\nVersion 37.0.0-mock\n",
            ))
            .with_script(&["devices"], MockAdbRunner::ok_output(
                "List of devices attached\nemulator-5554          device product:sdk model:Pixel_7 transport_id:1\n",
            ))
            .with_script(&["getprop"], MockAdbRunner::ok_output(
                "[ro.product.model]: [Pixel 7]\n[ro.build.version.release]: [14]\n",
            ))
            .with_script(&["shell", "ls"], MockAdbRunner::ok_output(
                "total 2\n-rw-r--r-- 1 root root 5 2024-01-01 08:00 a.txt\n",
            ))
    }

    fn listening(port: u16, address: &str, family: SocketFamily, state: &str) -> ListeningPort {
        ListeningPort {
            port,
            address: address.into(),
            family,
            state: state.into(),
            inode: 1,
            uid: 0,
        }
    }

    /// AR6.2 等价性：Agent 已还原的结构化端口经 Desktop 映射后，必须与 Legacy
    /// 解析器对同一份 `/proc/net` 原文的输出逐项一致（含只留 LISTEN、去重、排序）。
    #[test]
    fn agent_listening_ports_match_legacy_parser_output() {
        const RAW: &str = concat!(
            "/proc/net/tcp:   0: 0100007F:2CEC 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 62728 1 0000000000000000 100 0 0 10 0\n",
            "/proc/net/tcp:   1: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1046        0 51406 1 0000000000000000 100 0 0 10 0\n",
            "/proc/net/tcp:   2: B165B40A:1F90 0100007F:1F94 01 00000000:00000000 00:00000000 00000000 10107        0 98765 1 0000000000000000 100 0 0 10 0\n",
            "/proc/net/tcp6:  3: 00000000000000000000000000000000:1F91 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 4242 1 0000000000000000 100 0 0 10 0\n",
        );
        let legacy = adb::parse_listening_ports(RAW);
        let agent = map_agent_listening_ports(&[
            listening(11500, "127.0.0.1", SocketFamily::Ipv4, "listen"),
            listening(11500, "127.0.0.1", SocketFamily::Ipv4, "listen"), // 重复行必须合并
            listening(8080, "0.0.0.0", SocketFamily::Ipv4, "listen"),
            listening(8080, "10.180.101.177", SocketFamily::Ipv4, "established"),
            listening(8081, "::", SocketFamily::Ipv6, "listen"),
        ]);
        assert_eq!(
            agent, legacy,
            "Agent 映射结果必须与 Legacy 解析器完全同形同序"
        );
        // 排序是「端口→族→地址」，与 Legacy 一致：8080/8081(tcp6)/11500
        assert_eq!(agent.len(), 3);
        assert_eq!((agent[0].port, agent[0].family), (8080, "tcp"));
        assert_eq!((agent[1].port, agent[1].family), (8081, "tcp6"));
        assert_eq!(
            (agent[2].port, agent[2].address.as_str()),
            (11500, "127.0.0.1")
        );
    }

    #[test]
    fn agent_port_holders_drop_unknown_owner_and_sort_by_pid() {
        let holder = |pid: u32, comm: Option<&str>| PortHoldingProcess {
            pid,
            uid: 0,
            family: SocketFamily::Ipv4,
            address: "127.0.0.1".into(),
            state: "listen".into(),
            inode: 7,
            comm: comm.map(str::to_string),
        };
        let result = ProcessByPortResult {
            port: 24567,
            sockets: vec![
                holder(300, Some("toybox")),
                holder(12, None),
                holder(300, Some("x")),
            ],
            unowned: vec![holder(0, None)],
            truncated: false,
            skipped: vec![],
        };
        let holders = map_agent_port_holders(&result);
        assert_eq!(
            holders,
            vec![
                adb::PortHolder {
                    pid: 12,
                    name: "?".into()
                },
                adb::PortHolder {
                    pid: 300,
                    name: "toybox".into()
                },
            ],
            "pid=0（属主未知）不得编成假进程，重复 pid 要去重，comm 缺失回退 ?"
        );
    }

    #[tokio::test]
    async fn mock_environment_reports_installed() {
        let env = mock_available().environment().await;
        assert!(env.installed);
        assert_eq!(env.version.as_ref().unwrap().version, "1.0.41");
    }

    #[tokio::test]
    async fn mock_environment_reports_not_found() {
        let env = MockAdbRunner::new(false).environment().await;
        assert!(!env.installed);
        assert!(env.hint.unwrap().contains("adb"));
    }

    #[tokio::test]
    async fn mock_run_matches_scripts_and_build_args_flow() {
        let runner = mock_available();
        let args = adb::build_args(None, &adb::cmd_devices());
        let out = runner.run("/mock/adb", &args, LIST_TIMEOUT).await.unwrap();
        let devs = adb::parse_devices(&out.stdout);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].serial, "emulator-5554");
        assert!(devs[0].is_ready());
    }

    #[tokio::test]
    async fn mock_run_unmatched_returns_error_code() {
        let runner = mock_available();
        let out = runner
            .run("/mock/adb", &["reboot".to_string()], LIST_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(out.exit_code, Some(-1));
    }

    #[tokio::test]
    async fn legacy_device_and_package_query_outputs_remain_parseable() {
        let runner = MockAdbRunner::new(true)
            .with_script(
                &["getprop"],
                MockAdbRunner::ok_output(
                    "[ro.product.model]: [Test Device]\r\n[ro.build.version.sdk]: [35]\r\n",
                ),
            )
            .with_script(
                &["pm", "list", "packages", "-3"],
                MockAdbRunner::ok_output("package:com.example.one\r\npackage:com.example.two\r\n"),
            );

        let getprop_args = adb::build_args(Some("SERIAL_REDACTED"), &adb::cmd_getprop());
        let props_out = runner
            .run("/mock/adb", &getprop_args, LIST_TIMEOUT)
            .await
            .unwrap();
        let info =
            adb::device_info_from_props("SERIAL_REDACTED", &adb::parse_getprop(&props_out.stdout));
        assert_eq!(info.model, "Test Device");
        assert_eq!(info.sdk_int, "35");
        assert_eq!(info.manufacturer, "");

        let package_args = adb::build_args(Some("SERIAL_REDACTED"), &adb::cmd_list_packages(true));
        let packages_out = runner
            .run("/mock/adb", &package_args, LIST_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(
            adb::parse_packages(&packages_out.stdout),
            ["com.example.one", "com.example.two"]
        );
    }

    #[tokio::test]
    async fn legacy_runner_preserves_permission_and_missing_command_failures() {
        let runner = MockAdbRunner::new(true).with_script(
            &["pm", "list", "packages"],
            AdbRunOutput {
                stdout: String::new(),
                stderr: "Security exception: Permission denied".into(),
                exit_code: Some(1),
            },
        );
        let args = adb::build_args(Some("SERIAL_REDACTED"), &adb::cmd_list_packages(true));
        let denied = runner.run("/mock/adb", &args, LIST_TIMEOUT).await.unwrap();
        assert_eq!(denied.exit_code, Some(1));
        assert!(denied.stderr.contains("Permission denied"));
        assert!(adb::parse_packages(&denied.stdout).is_empty());

        let missing = runner
            .run(
                "/mock/adb",
                &adb::build_args(
                    Some("SERIAL_REDACTED"),
                    &adb::cmd_shell("definitely_missing_command"),
                ),
                LIST_TIMEOUT,
            )
            .await
            .unwrap();
        assert_eq!(missing.exit_code, Some(-1));
        assert_eq!(missing.stderr, "mock: unmatched args");
    }

    #[test]
    fn agent_device_info_maps_optional_protocol_fields_to_existing_command_dto() {
        let info = map_agent_device_info(
            "SERIAL-1",
            DeviceInfoResult {
                serial: Some("device-property-serial".into()),
                model: Some("Pixel Test".into()),
                manufacturer: None,
                android_version: Some("16".into()),
                api_level: Some(36),
                primary_abi: Some("arm64-v8a".into()),
                wlan_ipv4: Some("192.0.2.4".into()),
            },
        );
        assert_eq!(info.serial, "SERIAL-1");
        assert_eq!(info.model, "Pixel Test");
        assert_eq!(info.manufacturer, "");
        assert_eq!(info.android_version, "16");
        assert_eq!(info.sdk_int, "36");
        assert_eq!(info.ip.as_deref(), Some("192.0.2.4"));
    }

    #[test]
    fn build_candidates_marks_sources_in_priority_order() {
        // 本机环境不可控（PATH 里可能有真 adb），只断言结构：
        // 第一个候选来自 android_home（若 ANDROID_HOME 已设）或 path_env
        let cands = build_candidates();
        assert!(!cands.is_empty(), "PATH 至少给出候选");
        let home_set = std::env::var("ANDROID_HOME").is_ok();
        if home_set {
            assert_eq!(cands[0].source, "android_home");
        }
        // 所有候选都带文件名
        assert!(cands.iter().all(|c| !c.path.is_empty()));
    }

    /// 真机验证（#[ignore]，CI 不跑，开发机手动 `cargo test -- --ignored`）：
    /// 用用户环境（PATH/ANDROID_HOME）真实解析 adb 并解析 adb version 输出。
    #[tokio::test]
    #[ignore = "需要本机真实 adb，手动跑：cargo test -- --ignored"]
    async fn real_environment_with_user_env() {
        let db = Arc::new(Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        let runner = RealAdbRunner::new(config);
        let env = runner.environment().await;
        // 本机无 adb 时也应给出结构化 not_found（而非 panic）
        if env.installed {
            let path = env.path.clone().unwrap();
            let version = env.version.as_ref().expect("installed 必有 version");
            eprintln!(
                "[real] 探测成功 path={path} version={} build={}",
                version.version, version.build
            );
            assert!(!version.version.is_empty());
            // devices 也应能跑通（空列表或含设备都合法）
            let args = adb::build_args(None, &adb::cmd_devices());
            let out = runner
                .run(&path, &args, LIST_TIMEOUT)
                .await
                .expect("devices 调用");
            assert_eq!(out.exit_code, Some(0));
            let _ = adb::parse_devices(&out.stdout);
        } else {
            // 本机跑 --ignored 时若真无 adb 才走这里；用 eprintln 让 --nocapture 可见
            eprintln!("[real] adb 未检测到：{}", env.hint.unwrap_or_default());
            assert!(!env.installed);
        }
    }

    #[test]
    fn real_runner_cached_path_is_none_before_probe() {
        let db = Arc::new(Db::in_memory().unwrap());
        let config = Arc::new(ConfigService::new(db));
        let runner = RealAdbRunner::new(config);
        assert_eq!(runner.cached_path(), None);
    }
}

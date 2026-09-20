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
    ACTIVITY_FORCE_STOP, ACTIVITY_LAUNCH, DEVICE_INFO, DEVICE_ROOT_CHECK, FILESYSTEM_LIST,
    FILESYSTEM_PREVIEW, FILESYSTEM_STAT, HOSTED_CHMOD, HOSTED_LIST, HOSTED_START, HOSTED_STATUS,
    HOSTED_STOP, PACKAGE_LIST, PACKAGE_NATIVE_LIB_DIR, PACKAGE_REPLACE_NATIVE_LIBRARY,
    PACKAGE_UNINSTALL, PROCESS_BY_PORT, PROCESS_KILL, PROCESS_PORTS,
};

use agent_protocol::{
    ActivityForceStopParams, ActivityLaunchParams, DeviceInfoParams, DeviceInfoResult,
    DeviceRootCheckParams, DeviceRootCheckResult, FileKind, FilesystemListParams,
    FilesystemListResult, FilesystemPreviewParams, FilesystemPreviewResult, FilesystemStatParams,
    FilesystemStatResult, HostedBinaryInfo, HostedChmodParams, HostedChmodResult, HostedListParams,
    HostedListResult, HostedRunRecord, HostedRunState, HostedStartParams, HostedStartResult,
    HostedStatusParams, HostedStatusResult, HostedStopParams, HostedStopResult, KillSignal,
    ListeningPort, PackageListParams, PackageListResult, PackageNativeLibDirParams,
    PackageNativeLibDirResult, PackageScope, PackageUninstallParams, PackageUninstallResult,
    PackageWriteResult, PortHoldingProcess, PreviewEncoding, ProcessByPortParams,
    ProcessByPortResult, ProcessKillParams, ProcessKillResult, ProcessPortsParams,
    ProcessPortsResult, ReplaceNativeLibraryParams, ReplaceNativeLibraryResult, SO_STAGED_ROOT,
    SocketFamily,
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
/// 托管启动日志尾读长度（与 Legacy `tail -c 2048` 等价，迁移期保持一致便于对照）。
const HOSTED_LOG_TAIL_BYTES: u32 = 2048;
/// AR6.2 端口互查：Agent 要扫 `/proc/*/fd` 建 inode→pid 索引，进程数多时比
/// 单条 shell 慢，超时给到 15 s（Legacy 侧同量级：真机非 root 全量 ls 约 2~4 s）。
const PORT_SCAN_TIMEOUT: Duration = Duration::from_secs(15);
/// adb push 大 so 文件用长超时（USB 下数十 MB 也留足余量）
const PUSH_TIMEOUT: Duration = Duration::from_secs(120);
/// 包写操作（launch/force_stop/uninstall）的等待上限：Agent 内部是单条 `am`/`pm`
/// 加一次复核，真机实测都在秒级；给到 30 s 是留给冷启动与慢 ROM，不再往上放——
/// 写操作等太久，用户就会连点，那时候幂等比超时更该负责。
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// AR8.4 SO 替换：Desktop 等待上限必须**大于** Agent 内部各步之和
/// （sha256 复核 10 s×2 + su 步骤 20 s×3），否则宿主先放弃而设备还在写——
/// 那才是最坏局面（用户以为失败、其实写了一半）。宁可让 UI 多等。
const SO_REPLACE_TIMEOUT: Duration = Duration::from_secs(100);

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

    /// 设备侧目录列表。AR7.1 起默认走 Agent `filesystem.list`：条目由设备端 `lstat`
    /// 直接产出（type/mode/uid/gid/size/mtime/link target 全在），Desktop 不再解析
    /// `ls -l` 文本；只读幂等，Agent 不可用时回退 Legacy（删除条件见能力表 AR12.1）。
    pub async fn list_files(&self, serial: &str, path: &str) -> CoreResult<Vec<FileEntry>> {
        let route = self
            .android
            .select(serial, FILESYSTEM_LIST, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.list_files_legacy(serial, path).await;
        }

        // Legacy 用的是 `ls -lA`：含隐藏项、不含 `.`/`..`，Agent 侧必须同语义才可比
        let params = FilesystemListParams {
            path: path.to_owned(),
            include_hidden: true,
        };
        let agent_request = self.android.agent().request::<_, FilesystemListResult>(
            serial,
            FILESYSTEM_LIST,
            &params,
            LIST_TIMEOUT,
        );
        let (agent_result, legacy_result) = if filesystem_shadow_enabled() {
            let legacy = self.list_files_legacy(serial, path);
            let (agent, legacy) = tokio::join!(agent_request, legacy);
            (agent, Some(legacy))
        } else {
            (agent_request.await, None)
        };

        match agent_result {
            Ok(result) => {
                if !result.unreadable.is_empty() || result.truncated {
                    tracing::debug!(
                        serial,
                        path = %result.path,
                        method = FILESYSTEM_LIST,
                        unreadable = ?result.unreadable,
                        truncated = result.truncated,
                        "filesystem.list 有不可读或被截断的条目：空/短列表不等于空目录"
                    );
                }
                let entries = map_agent_file_entries(&result);
                if let Some(Ok(legacy)) = legacy_result {
                    log_filesystem_list_shadow_diff(serial, path, &entries, &legacy);
                }
                Ok(entries)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        FILESYSTEM_LIST,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                tracing::warn!(serial, path, "filesystem.list 回退 Legacy ADB");
                match legacy_result {
                    Some(result) => result,
                    None => self.list_files_legacy(serial, path).await,
                }
            }
        }
    }

    /// 单路径元数据（Agent only）：Legacy 侧没有等价能力（`ls -l` 文本不算），
    /// 因此不注册回退；Agent 不可用时直接返回结构化错误，绝不用 shell 拼一个。
    pub async fn file_stat(
        &self,
        serial: &str,
        path: &str,
        follow_symlink: bool,
    ) -> CoreResult<FilesystemStatResult> {
        let route = self
            .android
            .select(serial, FILESYSTEM_STAT, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        debug_assert_eq!(route.backend, AndroidBackendSource::Agent);
        let params = FilesystemStatParams {
            path: path.to_owned(),
            follow_symlink,
        };
        self.android
            .agent()
            .request::<_, FilesystemStatResult>(serial, FILESYSTEM_STAT, &params, LIST_TIMEOUT)
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)))
    }

    /// 受限预览（Agent only）：替代 `head -c` / `tail -c`，正文按文本或 hex 返回，
    /// 大文件由 Agent 侧硬上限夹住，不会把几十 MB 塞进 JSON 帧。
    pub async fn file_preview(
        &self,
        serial: &str,
        path: &str,
        max_bytes: Option<u32>,
        from_end: bool,
    ) -> CoreResult<FilesystemPreviewResult> {
        let route = self
            .android
            .select(
                serial,
                FILESYSTEM_PREVIEW,
                OperationKind::ReadOnlyIdempotent,
            )
            .map_err(CapabilityRouter::core_error)?;
        debug_assert_eq!(route.backend, AndroidBackendSource::Agent);
        let params = FilesystemPreviewParams {
            path: path.to_owned(),
            max_bytes,
            from_end,
        };
        self.android
            .agent()
            .request::<_, FilesystemPreviewResult>(
                serial,
                FILESYSTEM_PREVIEW,
                &params,
                LIST_TIMEOUT,
            )
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)))
    }

    /// Legacy 目录列表（仅作回退）：`ls -lA` + 宿主侧按空格切列解析。
    /// 文件名带空格只能靠「第 8 列之后全拼回去」猜，uid/gid 数值与 mtime 时间戳丢失。
    async fn list_files_legacy(&self, serial: &str, path: &str) -> CoreResult<Vec<FileEntry>> {
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

    /// 列出托管目录下的 ELF 可执行文件。AR7.2 起默认走 Agent `hosted.list`：
    /// ELF 由文件头 magic 判定（不再依赖设备端有没有 `file` 命令），权限/大小/uid/mtime
    /// 一次取回；Legacy 的 `ls -l` + `file` 两串 shell 保留为回退腿（删除条件见能力表）。
    pub async fn hosted_binaries(&self, serial: &str) -> CoreResult<Vec<adb::HostedBinary>> {
        let route = self
            .android
            .select(serial, HOSTED_LIST, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.hosted_binaries_legacy(serial).await;
        }
        let params = HostedListParams {};
        let agent_request = self.android.agent().request::<_, HostedListResult>(
            serial,
            HOSTED_LIST,
            &params,
            LIST_TIMEOUT,
        );
        let (agent_result, legacy_result) = if hosted_shadow_enabled() {
            let legacy = self.hosted_binaries_legacy(serial);
            let (agent, legacy) = tokio::join!(agent_request, legacy);
            (agent, Some(legacy))
        } else {
            (agent_request.await, None)
        };
        match agent_result {
            Ok(result) => {
                if result.truncated || !result.unreadable.is_empty() {
                    tracing::debug!(
                        serial,
                        method = HOSTED_LIST,
                        truncated = result.truncated,
                        unreadable = ?result.unreadable,
                        "hosted.list 有截断或读不到的条目：列表不完整不等于目录内容如此"
                    );
                }
                let binaries = map_agent_hosted_binaries(&result.binaries);
                if let Some(Ok(legacy)) = legacy_result {
                    log_hosted_list_shadow_diff(serial, &binaries, &legacy);
                }
                Ok(binaries)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        HOSTED_LIST,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                tracing::warn!(serial, "hosted.list 回退 Legacy ADB");
                match legacy_result {
                    Some(result) => result,
                    None => self.hosted_binaries_legacy(serial).await,
                }
            }
        }
    }

    /// 托管运行表（Agent only）：稳定句柄 + pid + start time + 状态 + 退出码。
    /// 页面刷新或 Desktop 重启后仍能显示「谁真的在跑」，不再依赖前端本地状态。
    pub async fn hosted_runs(&self, serial: &str) -> CoreResult<Vec<HostedRunRecord>> {
        let route = self
            .android
            .select(serial, HOSTED_LIST, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend != AndroidBackendSource::Agent {
            return Err(CoreError::AgentUnavailable(
                "托管运行表只能由 Agent 提供，请先在设备页连接 Agent".into(),
            ));
        }
        let params = HostedListParams {};
        self.android
            .agent()
            .request::<_, HostedListResult>(serial, HOSTED_LIST, &params, LIST_TIMEOUT)
            .await
            .map(|result| result.runs)
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)))
    }

    /// Legacy 托管列表（仅作回退）：`ls -l` 拿权限 + `file <dir>/*` 判 ELF。
    /// file 命令不可用时报错（不猜测，避免把文本文件当二进制展示）。
    async fn hosted_binaries_legacy(&self, serial: &str) -> CoreResult<Vec<adb::HostedBinary>> {
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

    /// 托管写操作前置（AR7.2）：Agent 必须在线，且只能走 Agent 通道。
    /// 与 AR6.3 `process_kill` 同一条规则（§3.6 写操作不自动回退），
    /// 这里只是把「检查 + 路由 + 防御」收成一个函数给 chmod/start 共用。
    fn require_agent_write_route(&self, serial: &str, method: &str) -> CoreResult<()> {
        if !matches!(
            self.android.agent_status(serial).state,
            crate::models::agent::AgentSessionState::Ready
                | crate::models::agent::AgentSessionState::Degraded
        ) {
            return Err(CoreError::AgentUnavailable(format!(
                "{method} 需要 Agent 在线（设备页 → 安装/连接 Agent）；写操作不自动回退 adb shell"
            )));
        }
        let route = self
            .android
            .select(serial, method, OperationKind::Mutating)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend != AndroidBackendSource::Agent {
            return Err(CoreError::Internal(format!(
                "{method} 只允许 Agent 通道，拒绝在非 Agent 后端执行写操作"
            )));
        }
        Ok(())
    }

    /// 探测设备 su 是否可用。AR9.1 前置：默认走 Agent `device.root_check`，
    /// Desktop 不再自己拼 `su -c id`。两条链路都必须「只回答 su 可用性」，
    /// 且 Agent 侧额外带回自身 uid——**su 可用 ≠ Agent 有 root**（D026 的根因），
    /// UI 之后要按这个区分「root 支路能不能走 Agent」。只读幂等，可回退。
    pub async fn su_available(&self, serial: &str) -> CoreResult<bool> {
        let route = self
            .android
            .select(serial, DEVICE_ROOT_CHECK, OperationKind::ReadOnlyIdempotent)
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.su_available_legacy(serial).await;
        }
        let params = DeviceRootCheckParams {};
        let agent_request = self.android.agent().request::<_, DeviceRootCheckResult>(
            serial,
            DEVICE_ROOT_CHECK,
            &params,
            SHORT_CMD_TIMEOUT,
        );
        let (agent_result, legacy_result) = if root_shadow_enabled() {
            let legacy = self.su_available_legacy(serial);
            let (agent, legacy) = tokio::join!(agent_request, legacy);
            (agent, Some(legacy))
        } else {
            (agent_request.await, None)
        };
        match agent_result {
            Ok(result) => {
                if let Some(Ok(legacy)) = legacy_result {
                    if legacy != result.root {
                        tracing::warn!(
                            serial,
                            method = DEVICE_ROOT_CHECK,
                            agent = result.root,
                            legacy,
                            detail = ?result.detail,
                            "Agent/Legacy root 探测不一致（一侧超时或 su 包装脚本行为差异）"
                        );
                    }
                }
                tracing::debug!(
                    serial,
                    method = DEVICE_ROOT_CHECK,
                    root = result.root,
                    agent_uid = result.agent_uid,
                    probe_ms = result.probe_ms,
                    detail = ?result.detail,
                    "root 探测完成（su 可用性与 Agent 自身身份是两件事）"
                );
                Ok(result.root)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        DEVICE_ROOT_CHECK,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                match legacy_result {
                    Some(result) => result,
                    None => self.su_available_legacy(serial).await,
                }
            }
        }
    }

    /// Legacy root 探测（仅作回退）：`su -c id` + `uid=0` 判定。
    async fn su_available_legacy(&self, serial: &str) -> CoreResult<bool> {
        let cmd = adb::su_wrap("id");
        let args = adb::build_args(Some(serial), &adb::cmd_shell(&cmd));
        let out = self.run_adb(&args).await?;
        Ok(out.exit_code == Some(0) && adb::is_root_probe_ok(&out.stdout))
    }

    /// 赋予执行权限。AR7.2：`root=false` 走 Agent `hosted.chmod`（写操作，不回退 + 审计）；
    /// `root=true` 仍走 Legacy `su -c chmod +x`——root 属主的文件 shell 用户改不动（D026 身份边界）。
    pub async fn hosted_chmod(&self, serial: &str, name: &str, root: bool) -> CoreResult<()> {
        if !root {
            let params = HostedChmodParams {
                name: name.to_owned(),
            };
            let result: CoreResult<HostedChmodResult> = match self
                .require_agent_write_route(serial, HOSTED_CHMOD)
            {
                Ok(()) => self
                    .android
                    .agent()
                    .request::<_, HostedChmodResult>(
                        serial,
                        HOSTED_CHMOD,
                        &params,
                        SHORT_CMD_TIMEOUT,
                    )
                    .await
                    .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error))),
                Err(error) => Err(error),
            };
            audit_hosted_write(
                serial,
                HOSTED_CHMOD,
                name,
                "agent",
                &to_audit_summary(&result, |value| format!("mode={:o}", value.mode)),
            );
            return result.map(|_| ());
        }
        let cmd = Self::hosted_shell(name, "chmod +x")?;
        let cmd = adb::su_wrap(&cmd);
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

    /// 后台启动托管二进制并返回 pid。AR7.2：`root=false` 走 Agent `hosted.start`
    /// （参数数组 exec + 落盘运行记录 + 稳定句柄），`root=true` 走 Legacy `su -c`（D026）。
    /// 两条路径都在启动后复查存活并在失败时读日志尾部给真实死因——秒退的原因
    /// （CANNOT LINK / exec format / Permission denied）几乎只存在于 stderr。
    pub async fn hosted_run(&self, serial: &str, name: &str, root: bool) -> CoreResult<u32> {
        if root {
            return self.hosted_run_as_root(serial, name).await;
        }
        // AR7.2：非 root 启动走 Agent。Agent 侧用参数数组 exec，PID 与
        // `/proc/<pid>/stat` 的 start time 一起构成身份，记录落盘可跨重启对账；
        // 秒退复查改成按句柄取状态，不再靠 `sleep 0.3; kill -0` 加文件名反查。
        let params = HostedStartParams {
            name: name.to_owned(),
            args: Vec::new(),
            root: false,
        };
        let started: CoreResult<HostedStartResult> = match self
            .require_agent_write_route(serial, HOSTED_START)
        {
            Ok(()) => self
                .android
                .agent()
                .request::<_, HostedStartResult>(serial, HOSTED_START, &params, SHORT_CMD_TIMEOUT)
                .await
                .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error))),
            Err(error) => Err(error),
        };
        audit_hosted_write(
            serial,
            HOSTED_START,
            name,
            "agent",
            &to_audit_summary(&started, |value| {
                format!(
                    "handle={} pid={} log={}",
                    value.record.handle, value.record.pid, value.record.log_path
                )
            }),
        );
        let record = started.map(|value| value.record)?;
        // 原实现的 300 ms 复查窗口保留：秒退的真因基本都落在启动日志里
        tokio::time::sleep(Duration::from_millis(300)).await;
        let params = HostedStatusParams {
            handle: record.handle.clone(),
        };
        let status = self
            .android
            .agent()
            .request::<_, HostedStatusResult>(serial, HOSTED_STATUS, &params, SHORT_CMD_TIMEOUT)
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)))?;
        if status.record.state != HostedRunState::Running {
            let cause = self
                .read_hosted_log(serial, &status.record.log_path, false)
                .await
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            let detail = adb::run_log_diagnostics(&cause).unwrap_or_else(|| {
                format!("（无输出可参考；完整日志见 {}）", status.record.log_path)
            });
            let exit = status
                .record
                .exit_code
                .map(|code| format!("（退出码 {code}）"))
                .unwrap_or_default();
            return Err(CoreError::Internal(format!(
                "{name} 启动后立即退出：{detail}{exit}"
            )));
        }
        Ok(status.record.pid)
    }

    /// 按句柄停止托管进程（AR7.3，写操作）。
    ///
    /// 只走 Agent：句柄、start time 与「是不是我启动的子进程」都只有 Agent 知道，
    /// Legacy 的 `kill -9 <pid>` 给不出这些保证，所以既不回退也不假装等价（同 D028）。
    /// `root=true` 启动的托管进程没有句柄可寻（Agent 无 root 通道），那条路径继续用
    /// `device_binary_kill(root=true)`。
    ///
    /// `expected_pid` 是界面当前显示的 PID：与记录不符说明列表已过期（PID 可能易主），
    /// Agent 以 `precondition_failed` 拒止而不是按数字杀；目标已消失时是幂等成功。
    pub async fn hosted_stop(
        &self,
        serial: &str,
        handle: &str,
        expected_pid: Option<u32>,
    ) -> CoreResult<HostedStopResult> {
        let params = HostedStopParams {
            handle: handle.to_owned(),
            expected_pid,
            signal: KillSignal::Kill,
        };
        let result: CoreResult<HostedStopResult> =
            match self.require_agent_write_route(serial, HOSTED_STOP) {
                Ok(()) => self
                    .android
                    .agent()
                    .request::<_, HostedStopResult>(serial, HOSTED_STOP, &params, SHORT_CMD_TIMEOUT)
                    .await
                    .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error))),
                Err(error) => Err(error),
            };
        let summary = to_audit_summary(&result, |value| {
            format!(
                "pid={} outcome={:?} verified={} dropped={}",
                value.record.pid, value.outcome, value.identity_verified, value.record_dropped
            )
        });
        audit_hosted_write(serial, HOSTED_STOP, handle, "agent", &summary);
        result
    }

    /// 单条托管运行状态（Agent only）。
    pub async fn hosted_status(
        &self,
        serial: &str,
        handle: &str,
    ) -> CoreResult<HostedStatusResult> {
        let params = HostedStatusParams {
            handle: handle.to_owned(),
        };
        self.android
            .agent()
            .request::<_, HostedStatusResult>(serial, HOSTED_STATUS, &params, LIST_TIMEOUT)
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)))
    }

    /// Legacy 的 root 启动路径（AR7.4 起只剩这一条）：非 root 启动已由 Agent
    /// `hosted.start` 接管，且写操作不自动回退（D028），所以这里不再有 non-root 分支，
    /// 免得留下「看着像两条等价实现」的死路。
    ///
    /// ⚠️ 用 `;` 而非 `&&`：`&&` 的优先级低于 `&`，会把整个 `cd && nohup` 复合式后台化，
    /// `$!` 拿到的是子 shell pid 而非二进制 pid（kill/复查就全错了）；`;` 确保只有
    /// nohup 一段进后台，nohup exec 后 pid 即二进制 pid。整段经 su -c 单引号包裹：
    /// 外层 shell 不动 `&`/`$!`/重定向，由 root 内层 shell 解释（否则 su 只收到 `cd`）。
    async fn hosted_run_as_root(&self, serial: &str, name: &str) -> CoreResult<u32> {
        let root = true;
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

    /// 读托管启动日志尾部（截 2KB 防日志爆炸）。
    ///
    /// AR7.1：Agent 可用时走 `filesystem.preview{from_end:true}`——设备端 seek 后只读
    /// 尾块，正文按字节返回，不再让路径进 shell；`root=true` 的日志（root 启动的进程写的）
    /// 仍走 Legacy `su -c tail`，因为 Agent 以 shell 身份运行读不到（同 D026 身份边界）。
    async fn read_hosted_log(
        &self,
        serial: &str,
        log_path: &str,
        root: bool,
    ) -> CoreResult<String> {
        if !root
            && matches!(
                self.android
                    .select(serial, FILESYSTEM_PREVIEW, OperationKind::ReadOnlyIdempotent),
                Ok(decision) if decision.backend == AndroidBackendSource::Agent
            )
        {
            let params = FilesystemPreviewParams {
                path: log_path.to_owned(),
                max_bytes: Some(HOSTED_LOG_TAIL_BYTES),
                from_end: true,
            };
            if let Ok(preview) = self
                .android
                .agent()
                .request::<_, FilesystemPreviewResult>(
                    serial,
                    FILESYSTEM_PREVIEW,
                    &params,
                    LIST_TIMEOUT,
                )
                .await
            {
                return Ok(preview_text(&preview));
            }
            // 日志可能还没生成（进程刚起）：预览失败按「无日志」处理，交给调用方兜底文案
        }
        // Agent 可用时上面已经走 filesystem.preview；这里只剩 root 日志与
        // 「Agent 不在线」两种局面，非 root 的那条保持原命令形状。
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

    /// 查托管进程监听端口。AR7.3：直接复用 AR6.2 的 Agent `process.ports`
    /// （fd→inode 与 `/proc/net` 匹配都在设备端），Legacy 的那串 `ls -l` + grep
    /// 只在 `root=true` 时保留（Agent 是 shell 身份，读不到 root 进程的 fd 目录）。
    pub async fn hosted_ports(
        &self,
        serial: &str,
        pid: u32,
        root: bool,
    ) -> CoreResult<Vec<adb::ListenPort>> {
        if !root {
            return self.process_ports(serial, pid, false).await;
        }
        self.hosted_ports_legacy(serial, pid, true).await
    }

    /// Legacy 的 fd+grep 原始链路（只服务 `root=true` 与 AR6.2 的回退腿）。
    /// 必须与 `process_ports` 分开：两者互相调用会形成 async 递归（需装箱），
    /// 而且读起来像「有两条等价实现」，实际只有一条。
    async fn hosted_ports_legacy(
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
        self.hosted_ports_legacy(serial, pid, root).await
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

    /// 查询包安装 lib 目录（SO 替换页只读预览）。AR8.3 起默认走 Agent
    /// `package.native_lib_dir`：`dumpsys package` 在设备端解析，不再把几十 KB 文本
    /// 拖回宿主；ABI 换算规则与 Legacy `lib_dir_for_abi` 一致，但 framework 包那种
    /// 非 `<pkg>/lib/<abi>` 形态的目录不再当异常报错，而是按原值返回并标注来源。
    /// 只读幂等，Agent 不可用时回退 Legacy（删除条件见能力表 AR12.1）。
    pub async fn pkg_lib_dir(&self, serial: &str, pkg: &str, abi: &str) -> CoreResult<String> {
        if !adb::is_safe_pkg_name(pkg) {
            return Err(CoreError::Internal(format!("包名非法: {pkg}")));
        }
        if abi != "arm64" && abi != "arm" {
            return Err(CoreError::Internal(format!("ABI 仅支持 arm64/arm: {abi}")));
        }
        let route = self
            .android
            .select(
                serial,
                PACKAGE_NATIVE_LIB_DIR,
                OperationKind::ReadOnlyIdempotent,
            )
            .map_err(CapabilityRouter::core_error)?;
        if route.backend == AndroidBackendSource::LegacyAdb {
            return self.pkg_lib_dir_legacy(serial, pkg, abi).await;
        }
        let params = PackageNativeLibDirParams {
            package: pkg.to_owned(),
            abi: Some(abi.to_owned()),
            user: None,
        };
        let agent_request = self
            .android
            .agent()
            .request::<_, PackageNativeLibDirResult>(
                serial,
                PACKAGE_NATIVE_LIB_DIR,
                &params,
                LIST_TIMEOUT,
            );
        let (agent_result, legacy_result) = if native_lib_shadow_enabled() {
            let legacy = self.pkg_lib_dir_legacy(serial, pkg, abi);
            let (agent, legacy) = tokio::join!(agent_request, legacy);
            (agent, Some(legacy))
        } else {
            (agent_request.await, None)
        };
        match agent_result {
            Ok(result) => {
                let dir = result.native_lib_dir.clone();
                if let Some(Ok(legacy)) = legacy_result {
                    if legacy != dir {
                        tracing::warn!(
                            serial,
                            pkg,
                            abi,
                            method = PACKAGE_NATIVE_LIB_DIR,
                            agent = %dir,
                            legacy = %legacy,
                            source = ?result.source,
                            detail = ?result.detail,
                            "Agent/Legacy native lib dir 不一致（Legacy 报错时这里是改进，不是缺陷）"
                        );
                    }
                } else if let Some(Err(error)) = legacy_result {
                    tracing::debug!(
                        serial,
                        pkg,
                        abi,
                        method = PACKAGE_NATIVE_LIB_DIR,
                        legacy_error = %error,
                        "Legacy 解析失败而 Agent 给出结果：framework 包属预期差异"
                    );
                }
                Ok(dir)
            }
            Err(error) => {
                let fallback = self
                    .android
                    .fallback_after_agent_error(
                        serial,
                        PACKAGE_NATIVE_LIB_DIR,
                        OperationKind::ReadOnlyIdempotent,
                        &error,
                    )
                    .map_err(CapabilityRouter::core_error)?;
                debug_assert_eq!(fallback.backend, AndroidBackendSource::LegacyAdb);
                tracing::warn!(serial, pkg, "package.native_lib_dir 回退 Legacy ADB");
                match legacy_result {
                    Some(result) => result,
                    None => self.pkg_lib_dir_legacy(serial, pkg, abi).await,
                }
            }
        }
    }

    /// Legacy 解析（仅作回退）：`dumpsys package` 全文回宿主 + 字符串找字段。
    async fn pkg_lib_dir_legacy(&self, serial: &str, pkg: &str, abi: &str) -> CoreResult<String> {
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

    /// AR8.4：SO 替换（免重打包）——主机侧修补好的 `.so` 交给 Agent 原子装进包的 native lib 目录。
    ///
    /// 用户指定的底层流程没变（push → 查目录 → root 写入 → 清理），变的是**谁来保证正确性**：
    /// ① 目标路径由 Agent 从包信息推导，Desktop 不再 `dumpsys` + 拼字符串（解析已在 AR8.3 迁走）；
    /// ② 备份 → 同目录临时名 → fsync → 权限/属主/SELinux 上下文 → rename → sha256 复核，
    ///    全在设备侧一次会话内完成，Desktop 拿到的是**步骤链**而不是「命令没报错」；
    /// ③ 任一步失败 Agent 自动回滚（原有文件装回备份、原本没有就删掉我们写的）；
    /// ④ 写操作不自动回退（§3.6）：Agent 不在线或设备无 root 都直接报错并给出下一步，
    ///    绝不偷偷退回旧的 `su -c cat`——那条路既没备份也没复核。
    pub async fn replace_native_library(
        &self,
        serial: &str,
        local: &std::path::Path,
        pkg: &str,
        abi: &str,
    ) -> CoreResult<ReplaceNativeLibraryResult> {
        // ① 形状校验 + 暂存规划（纯函数，可脱离 AppHandle 单测）
        let (name, staged_dir, staged_path) = plan_so_staging(pkg, local, abi)?;

        // ② 路由：Agent 必须在线，且只允许 Agent 通道
        self.require_agent_write_route(serial, PACKAGE_REPLACE_NATIVE_LIBRARY)?;

        // ③ root 前置探测：/data/app 属 system，Agent 以 shell 身份写不进去，
        //    特权步骤由 Agent 起 su 子进程执行（D037）。su 不可用时给准确原因。
        if !self.su_available(serial).await? {
            return Err(CoreError::Internal(
                "SO 替换需要 root：设备 su 不可用（Agent 内的特权步骤要起 su 子进程）".into(),
            ));
        }

        // ④ 传输仍走 Desktop（adb push 是宿主能力，Agent 不需要也不该有网络出口）
        let push_args = adb::build_args(
            Some(serial),
            &adb::cmd_push(&local.display().to_string(), &staged_path),
        );
        let pushed = self.run_adb_with(&push_args, PUSH_TIMEOUT).await?;
        if pushed.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb push 到暂存目录失败: {}",
                pushed.stderr.trim()
            )));
        }

        // ⑤ 交给 Agent：备份/安装/复核/回滚都在设备侧一次做完
        let operation_id = format!("so-replace-{}", uuid::Uuid::new_v4().simple());
        let params = ReplaceNativeLibraryParams {
            package: pkg.to_owned(),
            abi: abi.to_owned(),
            so_name: name.to_owned(),
            staged_path: staged_path.clone(),
            operation_id,
        };
        let result: CoreResult<ReplaceNativeLibraryResult> = self
            .android
            .agent()
            .request::<_, ReplaceNativeLibraryResult>(
                serial,
                PACKAGE_REPLACE_NATIVE_LIBRARY,
                &params,
                SO_REPLACE_TIMEOUT,
            )
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)));

        // ⑥ 暂存件用完即删；**备份件留着**——那是唯一的撤销点，删它属于用户决定
        let cleanup = adb::build_args(
            Some(serial),
            &adb::cmd_shell(&format!("rm -f {staged_path}")),
        );
        if let Err(error) = self.run_adb(&cleanup).await {
            tracing::warn!(serial, %error, "暂存件清理失败（可忽略，不影响替换结果）");
        }

        audit_so_replace(
            serial,
            pkg,
            abi,
            &staged_dir,
            &to_audit_summary(&result, describe_replace),
        );
        result
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

    /// 长操作统一入口。`kind` 会写进任务的 `task_type`：AR7.4 起前端要按类型判断
    /// 「这个任务完成后设备侧文件变了没有」（push/install/uninstall），不能再靠
    /// 解析可读任务名猜，所以类型必须显式且唯一。
    async fn adb_task(
        &self,
        kind: &str,
        serial: Option<&str>,
        subcommand: &[String],
    ) -> CoreResult<String> {
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
        self.tasks.start_with_kind(kind, spec)
    }

    /// Raw Tool（§2.3）：用户在 Shell 页主动敲的原始命令，唯一入口是
    /// `commands::device::device_shell`；Rust 的 `pub(in ...)` 不能跨分支限制，
    /// 所以这条边界由 `tests/raw_tool_boundary.rs` 用源码扫描钉住（AR9.3：
    /// 「业务 Service 不调用 raw shell API」）。命令仍走参数数组 + TaskService，
    /// 保留取消与日志回放；名称带 `_task` 也是提醒：它产任务，不产业务结果。
    pub async fn raw_shell_task(&self, serial: &str, command: &str) -> CoreResult<String> {
        self.adb_task("adb.shell", Some(serial), &adb::cmd_shell(command))
            .await
    }

    pub async fn start_install(&self, serial: &str, local_apk: &str) -> CoreResult<String> {
        self.adb_task("adb.install", Some(serial), &adb::cmd_install(local_apk))
            .await
    }

    /// 启动应用（AR8.1 收尾：改走 Agent typed，**不再产任务卡**）。
    ///
    /// 与旧 `adb_task("adb.launch")` 的区别不只是少一张卡：旧路径只能告诉你
    /// 「`am start` 这条命令返回 0」，新路径回的是**复核过的事实**——
    /// `verified=true` 表示真的看到了新 pid，`outcome=replayed` 表示这是幂等命中
    /// （用户连点或网络重试不会二次启动），`no_op` 表示目标已在期望状态。
    pub async fn launch(&self, serial: &str, pkg: &str) -> CoreResult<PackageWriteResult> {
        let method = ACTIVITY_LAUNCH;
        let operation_id = plan_package_write(pkg, "launch")?;
        self.require_agent_write_route(serial, method)?;
        let result = self
            .android
            .agent()
            .request::<_, PackageWriteResult>(
                serial,
                method,
                &ActivityLaunchParams {
                    package: pkg.to_owned(),
                    operation_id,
                },
                WRITE_TIMEOUT,
            )
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)));
        audit_package_write(
            serial,
            method,
            pkg,
            &to_audit_summary(&result, |value| {
                format!(
                    "outcome={:?} verified={} pid={:?} detail={:?}",
                    value.outcome, value.verified, value.pid, value.detail
                )
            }),
        );
        result
    }

    /// 强制停止（同上：Agent typed + 幂等 + 审计，不产卡）。
    pub async fn force_stop(&self, serial: &str, pkg: &str) -> CoreResult<PackageWriteResult> {
        let method = ACTIVITY_FORCE_STOP;
        let operation_id = plan_package_write(pkg, "force-stop")?;
        self.require_agent_write_route(serial, method)?;
        let result = self
            .android
            .agent()
            .request::<_, PackageWriteResult>(
                serial,
                method,
                &ActivityForceStopParams {
                    package: pkg.to_owned(),
                    operation_id,
                },
                WRITE_TIMEOUT,
            )
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)));
        audit_package_write(
            serial,
            method,
            pkg,
            &to_audit_summary(&result, |value| {
                format!(
                    "outcome={:?} verified={} pid={:?} detail={:?}",
                    value.outcome, value.verified, value.pid, value.detail
                )
            }),
        );
        result
    }

    /// 卸载（同上）。`keep_data` 默认 false —— 与迁移前 `pm uninstall <pkg>` 的语义
    /// 完全一致，不能借迁移悄悄改成 `-k`（那样磁盘不释放，用户以为清掉了）。
    pub async fn uninstall(
        &self,
        serial: &str,
        pkg: &str,
        keep_data: bool,
    ) -> CoreResult<PackageUninstallResult> {
        let method = PACKAGE_UNINSTALL;
        let operation_id = plan_package_write(pkg, "uninstall")?;
        self.require_agent_write_route(serial, method)?;
        let result = self
            .android
            .agent()
            .request::<_, PackageUninstallResult>(
                serial,
                method,
                &PackageUninstallParams {
                    package: pkg.to_owned(),
                    operation_id,
                    keep_data,
                    user: None,
                },
                WRITE_TIMEOUT,
            )
            .await
            .map_err(|error| CapabilityRouter::core_error(RouteError::AgentFailure(error)));
        audit_package_write(
            serial,
            method,
            pkg,
            &to_audit_summary(&result, |value| {
                format!(
                    "outcome={:?} verified={} keep_data={} steps={}",
                    value.outcome,
                    value.verified,
                    value.keep_data,
                    value.steps.len()
                )
            }),
        );
        result
    }

    pub async fn start_push(&self, serial: &str, local: &str, remote: &str) -> CoreResult<String> {
        self.adb_task("adb.push", Some(serial), &adb::cmd_push(local, remote))
            .await
    }

    pub async fn start_pull(&self, serial: &str, remote: &str, local: &str) -> CoreResult<String> {
        self.adb_task("adb.pull", Some(serial), &adb::cmd_pull(remote, local))
            .await
    }

    /// Raw Tool（§2.3）：`adb logcat` 本身就是传输/调试工具，保留 Desktop 路径；
    /// 与 raw_shell_task 同样受 `tests/raw_tool_boundary.rs` 的源码扫描保护。
    /// 过滤参数交给 `cmd_logcat` 组数组，不做字符串拼接。
    pub async fn raw_logcat_task(&self, serial: &str, filter: Option<&str>) -> CoreResult<String> {
        self.adb_task("adb.logcat", Some(serial), &adb::cmd_logcat(filter))
            .await
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

/// 写操作结果转审计字符串：成功时带上调用方最关心的证据，失败时保留结构化错误文本。
fn to_audit_summary<T, F>(
    result: &CoreResult<T>,
    describe: F,
) -> std::result::Result<String, String>
where
    F: FnOnce(&T) -> String,
{
    result
        .as_ref()
        .map(describe)
        .map_err(|error| error.to_string())
}

/// 包写操作的宿主侧把关（纯函数，AR8.1 收尾）。
///
/// 只负责两件事：包名形状、`operation_id` 生成。路由与审计留在方法里，因为那两件
/// 事需要 `AppHandle` 才能构造出来。**每次用户动作生成一个 id**（不是每次请求）：
/// 同一请求的网络重试会复用同一个 id 从而命中设备侧幂等台账，而用户连点两次是两次
/// 合法意图，必须分别执行。
fn plan_package_write(pkg: &str, id_prefix: &str) -> CoreResult<String> {
    if !adb::is_safe_pkg_name(pkg) {
        return Err(CoreError::Internal(format!("包名非法: {pkg}")));
    }
    if !adb::is_safe_pkg_name(id_prefix) {
        return Err(CoreError::Internal(format!(
            "内部错误：操作前缀非法 {id_prefix}"
        )));
    }
    let seed = uuid::Uuid::new_v4().simple().to_string();
    Ok(format!("{id_prefix}-{seed}"))
}

/// §3.7 包写操作审计：卸载/强停/启动都会改设备状态，必须留下「谁、对哪个包、结果」。
/// 只含结构化字段，不含令牌与命令正文。
fn audit_package_write(serial: &str, method: &str, pkg: &str, outcome: &Result<String, String>) {
    match outcome.as_ref() {
        Ok(summary) => tracing::info!(
            target: "audit",
            serial,
            method,
            pkg,
            backend = "agent",
            outcome = %summary,
            "包写操作已执行"
        ),
        Err(reason) => tracing::warn!(
            target: "audit",
            serial,
            method,
            pkg,
            backend = "agent",
            error = %reason,
            "包写操作失败"
        ),
    }
}

/// 审计摘要：只留「是否真执行、是否复核、落在哪」，不含文件内容与任何令牌。
fn describe_replace(result: &ReplaceNativeLibraryResult) -> String {
    format!(
        "outcome={:?} verified={} rolled_back={} replaced_existing={} target={}",
        result.outcome,
        result.verified,
        result.rolled_back.unwrap_or(false),
        result.replaced_existing,
        result.target_path
    )
}

/// §3.7 SO 替换审计：写安装目录必须留下「谁、动了哪个包的哪个文件、结果如何」。
/// 含目标目录（审计要看的就是这个），不含文件内容。
fn audit_so_replace(
    serial: &str,
    pkg: &str,
    abi: &str,
    staged_dir: &str,
    outcome: &Result<String, String>,
) {
    match outcome.as_ref() {
        Ok(summary) => tracing::info!(
            target: "audit",
            serial,
            method = PACKAGE_REPLACE_NATIVE_LIBRARY,
            pkg,
            abi,
            staged_dir,
            outcome = %summary,
            "SO 替换已执行"
        ),
        Err(reason) => tracing::warn!(
            target: "audit",
            serial,
            method = PACKAGE_REPLACE_NATIVE_LIBRARY,
            pkg,
            abi,
            staged_dir,
            error = %reason,
            "SO 替换失败"
        ),
    }
}

/// SO 替换的入参把关 + 暂存路径规划（AR8.4，纯函数）。
///
/// 抽出来的理由：`DeviceService::new` 需要 `tauri::AppHandle`，服务方法里的分支在
/// 单测里根本构造不出来，写操作的「什么请求会被挡在宿主」就永远只能靠真机腿镜像。
/// 把关与规划做成纯函数后，规则本身有单测，腿只需要负责设备侧那半段。
/// 返回 `(so 文件名, 本次暂存目录, 暂存文件路径)`。
fn plan_so_staging(
    pkg: &str,
    local: &std::path::Path,
    abi: &str,
) -> CoreResult<(String, String, String)> {
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
        .and_then(|value| value.to_str())
        .ok_or_else(|| CoreError::Internal("本地文件路径无法解析文件名".into()))?;
    if !adb::is_safe_so_name(name) {
        return Err(CoreError::Internal(format!(
            "文件名需为 .so 且只含字母数字与 _.-+（与设备侧同一套规则）: {name}"
        )));
    }
    if !local.is_file() {
        return Err(CoreError::Internal(format!(
            "本地文件不存在: {}",
            local.display()
        )));
    }
    // 每次操作一个唯一子目录：Agent 只认 `<root>/<本次目录>/<name>.so`，
    // 备份件与暂存件同级，同名 so 连替两次也不会互相踩掉第一个备份。
    let seed = uuid::Uuid::new_v4().simple().to_string();
    let dir = format!("{SO_STAGED_ROOT}/{pkg}-{}", &seed[..8]);
    let path = format!("{dir}/{name}");
    Ok((name.to_owned(), dir, path))
}

/// §3.7 托管写操作（chmod/start）审计：字段化、成功失败都记，不含命令正文与令牌。
fn audit_hosted_write(
    serial: &str,
    method: &str,
    subject: &str,
    backend: &str,
    outcome: &std::result::Result<String, String>,
) {
    match outcome.as_ref() {
        Ok(summary) => tracing::info!(
            target: "audit",
            serial,
            method,
            subject,
            backend,
            outcome = %summary,
            "托管写操作已执行"
        ),
        Err(reason) => tracing::warn!(
            target: "audit",
            serial,
            method,
            subject,
            backend,
            error = %reason,
            "托管写操作失败"
        ),
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

/// root 探测的 Agent/Legacy 对照开关（默认开，设 0/false/off 关闭）。
fn root_shadow_enabled() -> bool {
    !std::env::var("APP_REVERSE_TOOLS_ROOT_SHADOW")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off"))
}

/// AR8.3 native lib 目录的 Agent/Legacy 对照开关（默认开，设 0/false/off 关闭）。
fn native_lib_shadow_enabled() -> bool {
    !std::env::var("APP_REVERSE_TOOLS_NATIVE_LIB_SHADOW")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off"))
}

/// AR7.2 托管列表的 Agent/Legacy 对照开关（默认开，设 0/false/off 关闭）。
fn hosted_shadow_enabled() -> bool {
    !std::env::var("APP_REVERSE_TOOLS_HOSTED_SHADOW")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off"))
}

/// Agent `hosted.list` → 前端既有 `HostedBinary[]` 契约（name/path/size/perms/hasExec）。
fn map_agent_hosted_binaries(binaries: &[HostedBinaryInfo]) -> Vec<adb::HostedBinary> {
    let mut out: Vec<adb::HostedBinary> = binaries
        .iter()
        .map(|item| adb::HostedBinary {
            name: item.name.clone(),
            path: item.path.clone(),
            size: i64::try_from(item.size).unwrap_or(i64::MAX),
            perms: item.mode_text.clone(),
            has_exec: item.has_exec,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 托管列表的 Agent/Legacy 差异只写日志（迁移期观测用），判据同 D024：名字集合 + 逐项字段。
fn log_hosted_list_shadow_diff(
    serial: &str,
    agent: &[adb::HostedBinary],
    legacy: &[adb::HostedBinary],
) {
    let agent_names: HashSet<&str> = agent.iter().map(|item| item.name.as_str()).collect();
    let legacy_names: HashSet<&str> = legacy.iter().map(|item| item.name.as_str()).collect();
    let only_agent: Vec<&str> = agent_names.difference(&legacy_names).copied().collect();
    let only_legacy: Vec<&str> = legacy_names.difference(&agent_names).copied().collect();
    let mut field_diffs: Vec<String> = Vec::new();
    for legacy_item in legacy {
        let Some(agent_item) = agent.iter().find(|item| item.name == legacy_item.name) else {
            continue;
        };
        if agent_item.has_exec != legacy_item.has_exec {
            field_diffs.push(format!("{}:has_exec", legacy_item.name));
        }
        if agent_item.size != legacy_item.size {
            field_diffs.push(format!("{}:size", legacy_item.name));
        }
        if !legacy_item.perms.is_empty() && agent_item.perms != legacy_item.perms {
            field_diffs.push(format!(
                "{}:perms({}!={})",
                legacy_item.name, agent_item.perms, legacy_item.perms
            ));
        }
    }
    if only_agent.is_empty() && only_legacy.is_empty() && field_diffs.is_empty() {
        tracing::debug!(serial, method = HOSTED_LIST, "hosted.list matched");
        return;
    }
    tracing::warn!(
        serial,
        method = HOSTED_LIST,
        agent_only = ?only_agent,
        legacy_only = ?only_legacy,
        field_diffs = ?field_diffs,
        "Agent/Legacy hosted.list shadow compare differed"
    );
}

/// AR7.1 文件列表的 Agent/Legacy 对照开关（默认开，设 0/false/off 关闭）。
fn filesystem_shadow_enabled() -> bool {
    !std::env::var("APP_REVERSE_TOOLS_FILESYSTEM_SHADOW")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off"))
}

/// Agent `filesystem.list` → 前端既有 `FileEntry[]` 契约。
/// 目录判定与 Legacy 一致：指向目录的符号链接仍算链接（`ls -l` 看首字符 `l`）。
fn map_agent_file_entries(result: &FilesystemListResult) -> Vec<FileEntry> {
    let mut entries: Vec<FileEntry> = result
        .entries
        .iter()
        .map(|entry| FileEntry {
            name: entry.name.clone(),
            is_dir: entry.kind == FileKind::Dir,
            // 目录项大小不会超过 i64；真超了就夹住，不用负数冒充
            size: i64::try_from(entry.size).unwrap_or(i64::MAX),
            symlink: entry.symlink_target.clone(),
            perms: entry.mode_text.clone(),
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// 预览结果还原成字符串：文本直接给，hex 先解回字节再 lossy 转文本
/// （日志尾读只关心可读内容；二进制日志本来 Legacy 也是 lossy 输出）。
pub(crate) fn preview_text(preview: &FilesystemPreviewResult) -> String {
    match (&preview.encoding, &preview.text, &preview.hex) {
        (PreviewEncoding::Utf8, Some(text), _) => text.clone(),
        (PreviewEncoding::Hex, _, Some(hex)) => {
            let bytes = (0..hex.len())
                .step_by(2)
                .filter_map(|index| u8::from_str_radix(&hex[index..index + 2], 16).ok())
                .collect::<Vec<u8>>();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        // 协议保证二者必有其一；真出现空结果就返回空串，不编造内容
        _ => String::new(),
    }
}

/// Agent 与 Legacy 的目录项差异只写日志，不影响返回值（迁移期观测用）。
/// 判据沿用 D024 的思路：按名字集合比较，逐项再比 is_dir/size/symlink/perms。
fn log_filesystem_list_shadow_diff(
    serial: &str,
    path: &str,
    agent: &[FileEntry],
    legacy: &[FileEntry],
) {
    let agent_names: HashSet<&str> = agent.iter().map(|entry| entry.name.as_str()).collect();
    let legacy_names: HashSet<&str> = legacy.iter().map(|entry| entry.name.as_str()).collect();
    let only_agent: Vec<&str> = agent_names.difference(&legacy_names).copied().collect();
    let only_legacy: Vec<&str> = legacy_names.difference(&agent_names).copied().collect();
    let mut field_diffs: Vec<String> = Vec::new();
    for legacy_entry in legacy {
        let Some(agent_entry) = agent.iter().find(|entry| entry.name == legacy_entry.name) else {
            continue;
        };
        if agent_entry.is_dir != legacy_entry.is_dir {
            field_diffs.push(format!("{}:is_dir", legacy_entry.name));
        }
        if agent_entry.size != legacy_entry.size {
            field_diffs.push(format!("{}:size", legacy_entry.name));
        }
        if agent_entry.symlink != legacy_entry.symlink {
            field_diffs.push(format!("{}:symlink", legacy_entry.name));
        }
        // 权限串只在两侧都非空时比：Legacy 解析失败会给空串，那是对照方的缺陷不是差异
        if !legacy_entry.perms.is_empty() && agent_entry.perms != legacy_entry.perms {
            field_diffs.push(format!(
                "{}:perms({}!={})",
                legacy_entry.name, agent_entry.perms, legacy_entry.perms
            ));
        }
    }
    if only_agent.is_empty() && only_legacy.is_empty() && field_diffs.is_empty() {
        tracing::debug!(
            serial,
            path,
            method = FILESYSTEM_LIST,
            "filesystem.list matched"
        );
        return;
    }
    tracing::warn!(
        serial,
        path,
        method = FILESYSTEM_LIST,
        agent_only = ?only_agent,
        legacy_only = ?only_legacy,
        field_diffs = ?field_diffs,
        "Agent/Legacy filesystem.list shadow compare differed"
    );
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

    /// AR7.2 等价性：Agent 的托管条目映射后必须与 Legacy `ls -l` + `file` 的组合结果同形。
    #[test]
    fn agent_hosted_binaries_match_legacy_listing() {
        let info = |name: &str, mode: u32, size: u64| HostedBinaryInfo {
            name: name.into(),
            path: format!("{}/{}", adb::HOSTED_DIR, name),
            size,
            mode,
            mode_text: agent_protocol::render_mode_text(FileKind::File, mode),
            has_exec: mode & 0o100 != 0,
            uid: 2000,
            mtime_unix: 1_760_000_000,
        };
        let agent = map_agent_hosted_binaries(&[
            info("zz-tool", 0o755, 4096),
            info("no-exec", 0o644, 128),
            info("a with space", 0o4755, 2048),
        ]);
        let legacy = adb::hosted_binaries(
            concat!(
                "-rwsr-xr-x 1 shell shell 2048 2024-01-01 08:00 a with space\n",
                "-rw-r--r-- 1 shell shell 128 2024-01-01 08:00 no-exec\n",
                "-rwxr-xr-x 1 shell shell 4096 2024-01-01 08:00 zz-tool\n",
            ),
            concat!(
                "/data/local/tmp/a with space: ELF 64-bit LSB pie executable\n",
                "/data/local/tmp/no-exec: ELF 64-bit LSB pie executable\n",
                "/data/local/tmp/zz-tool: ELF 64-bit LSB pie executable\n",
            ),
        );
        assert_eq!(
            agent, legacy,
            "映射必须与 Legacy 组合结果逐项相等（含按名排序）"
        );
        assert_eq!(agent[0].name, "a with space", "带空格的文件名不能被切错列");
        assert_eq!(agent[0].perms, "-rwsr-xr-x", "setuid 位必须与 ls 一致");
        assert!(!agent[1].has_exec, "no-exec 必须标为不可执行");
    }

    #[test]
    fn hosted_binary_mapping_flags_truncated_dirs_without_hiding_them() {
        let result = HostedListResult {
            dir: adb::HOSTED_DIR.into(),
            binaries: vec![],
            runs: vec![],
            truncated: true,
            unreadable: vec!["libfoo.so: Permission denied".into()],
        };
        assert!(map_agent_hosted_binaries(&result.binaries).is_empty());
        // 截断/不可读只存在于 HostedListResult 上，映射后靠 list_files 那条 debug 日志留证
        assert!(result.truncated && result.unreadable.len() == 1);
    }

    fn file_stat(
        name: &str,
        kind: FileKind,
        mode: u32,
        size: u64,
        symlink_target: Option<&str>,
    ) -> agent_protocol::FileStat {
        agent_protocol::FileStat {
            name: name.to_owned(),
            kind,
            mode,
            mode_text: agent_protocol::render_mode_text(kind, mode),
            uid: 2000,
            gid: 2000,
            size,
            mtime_unix: 1_760_000_000,
            symlink_target: symlink_target.map(str::to_string),
            readable: true,
        }
    }

    /// AR7.1 等价性：Agent 的结构化条目映射后必须与 Legacy `ls -lA` 解析结果同形，
    /// 包括「指向目录的符号链接不算目录」这条 Legacy 也遵守的规则。
    #[test]
    fn agent_file_entries_match_legacy_parser_output() {
        const RAW: &str = concat!(
            "total 24\n",
            "drwxrwx--x 2 root root 3452 2024-01-01 08:00 storage\n",
            "-rw-rw---- 1 u0_a1 u0_a1 1024 2024-01-01 08:00 my file.txt\n",
            "lrwxrwxrwx 1 root root 11 2024-01-01 08:00 init -> /init\n",
        );
        // Legacy 保留 `ls` 的输出顺序，Agent 侧固定按名字排序；顺序不是契约
        // （shadow 判据是名字集合 + 逐项字段，见 log_filesystem_list_shadow_diff），
        // 所以把对照方也排序后再比，免得把排序差异当成迁移缺陷。
        let mut legacy = RAW
            .lines()
            .filter_map(adb::parse_ls_long)
            .collect::<Vec<_>>();
        legacy.sort_by(|a, b| a.name.cmp(&b.name));
        let result = FilesystemListResult {
            path: "/data/local/tmp".into(),
            entries: vec![
                file_stat("init", FileKind::Symlink, 0o777, 11, Some("/init")),
                file_stat("my file.txt", FileKind::File, 0o660, 1024, None),
                file_stat("storage", FileKind::Dir, 0o771, 3452, None),
            ],
            truncated: false,
            unreadable: vec![],
        };
        let agent = map_agent_file_entries(&result);
        assert_eq!(agent, legacy, "映射结果必须与 Legacy 解析逐项相等");
        // 名字带空格、符号链接目标、目录判定这三处是 Legacy 文本解析最容易错的地方
        assert_eq!(agent[1].name, "my file.txt");
        assert_eq!(agent[0].symlink.as_deref(), Some("/init"));
        assert!(!agent[0].is_dir, "指向文件的链接不是目录");
        assert!(agent[2].is_dir);
        assert_eq!(agent[2].perms, "drwxrwx--x");
    }

    #[test]
    fn agent_file_entries_keep_unreadable_and_truncation_visible() {
        let result = FilesystemListResult {
            path: "/proc".into(),
            entries: vec![file_stat("1", FileKind::Dir, 0o555, 0, None)],
            truncated: true,
            unreadable: vec!["kcore: permission_denied".into()],
        };
        let entries = map_agent_file_entries(&result);
        assert_eq!(entries.len(), 1);
        // 截断与不可读不会体现在 FileEntry 数组里，必须靠日志留证（见 list_files）
        assert!(result.truncated && !result.unreadable.is_empty());
    }

    #[test]
    fn preview_text_decodes_hex_and_never_invents_content() {
        let text = FilesystemPreviewResult {
            path: "/data/local/tmp/a.log".into(),
            size: 12,
            offset: 0,
            returned_bytes: 12,
            encoding: PreviewEncoding::Utf8,
            text: Some("line-1\nline-2\n".into()),
            hex: None,
            truncated: false,
            detail: None,
        };
        assert_eq!(preview_text(&text), "line-1\nline-2\n");

        // "hi\n" 的 hex 形式：尾读日志时二进制内容也要还原成同样的字节
        let binary = FilesystemPreviewResult {
            encoding: PreviewEncoding::Hex,
            text: None,
            hex: Some("68690a".into()),
            ..text.clone()
        };
        assert_eq!(preview_text(&binary), "hi\n");

        let empty = FilesystemPreviewResult {
            text: None,
            hex: None,
            ..text
        };
        assert_eq!(preview_text(&empty), "", "两者都缺时返回空串，不编造内容");
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

    /// AR8.4：Desktop 侧的入参把关必须与设备侧同形，而且不合法就不该算出暂存路径。
    /// （服务方法要 `AppHandle` 才构造得出来，所以把关做成纯函数后在这里钉住。）
    #[test]
    fn so_staging_plan_mirrors_the_agent_side_rules() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("libc++_shared.so");
        std::fs::write(&good, b"x").unwrap();
        let (name, staged_dir, staged_path) =
            plan_so_staging("com.example.app", &good, "arm64").expect("合法输入必须通过");
        assert_eq!(name, "libc++_shared.so", "C++ 库名不能被宿主这层误拦");
        assert_eq!(
            staged_path,
            format!("{staged_dir}/{name}"),
            "暂存件必须躺在本次操作目录里"
        );
        assert!(
            staged_dir.starts_with(&format!("{SO_STAGED_ROOT}/com.example.app-")),
            "实际: {staged_dir}"
        );
        assert!(
            !staged_dir.ends_with('/'),
            "目录参数不该以 / 结尾（设备侧同样拒绝）"
        );
        // 两次调用的目录必须不同：同名 so 连替两次不能互相踩掉备份
        let again = plan_so_staging("com.example.app", &good, "arm64").unwrap();
        assert_ne!(staged_dir, again.1);

        let missing = dir.path().join("nope.so");
        let spaced = dir.path().join("a b.so");
        let not_so = dir.path().join("a.txt");
        std::fs::write(&spaced, b"x").unwrap();
        std::fs::write(&not_so, b"x").unwrap();
        let cases = [
            ("bad pkg;rm", &good, "arm64"),
            ("com.example.app", &good, "x86"),
            ("com.example.app", &missing, "arm64"),
            ("com.example.app", &spaced, "arm64"),
            ("com.example.app", &not_so, "arm64"),
        ];
        for (bad_pkg, bad_path, bad_abi) in cases {
            let error = plan_so_staging(bad_pkg, bad_path, bad_abi)
                .expect_err("非法输入必须在宿主就被拒，不能推到设备上再失败");
            let text = error.to_string();
            assert!(
                text.contains("包名")
                    || text.contains("ABI")
                    || text.contains("文件名")
                    || text.contains("本地文件"),
                "错误要说清拦在哪一项，实际: {text}"
            );
        }
    }

    /// AR8.1：包写操作的宿主侧把关。`operation_id` 形状必须满足设备侧白名单
    /// （`[A-Za-z0-9._-:]`，≤64）——写操作不允许匿名提交，id 不合法就等于请求被拒。
    #[test]
    fn package_write_plan_validates_name_and_shapes_an_idempotency_key() {
        let id = plan_package_write("com.example.app", "launch").expect("合法包名必须通过");
        assert!(id.starts_with("launch-"), "实际: {id}");
        assert!(id.len() <= 64, "operation_id 超过设备侧上限: {}", id.len());
        assert!(
            id.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')),
            "operation_id 含设备侧会拒的字符: {id}"
        );
        assert_ne!(
            id,
            plan_package_write("com.example.app", "launch").unwrap(),
            "两次用户动作必须是两个 id，否则第二次会被幂等台账吃掉"
        );
        for bad in ["", "com; rm -rf /", "com x", "pm path com.x"] {
            let error =
                plan_package_write(bad, "uninstall").expect_err(&format!("{bad:?} 必须被拒"));
            assert!(error.to_string().contains("包名"), "错误要看得懂: {error}");
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

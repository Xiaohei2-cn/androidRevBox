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

use async_trait::async_trait;
use serde::Serialize;
use tauri::Emitter;

use crate::adapters::adb::{self, AdbVersionInfo, DeviceEntry, DeviceInfo, FileEntry};
use crate::core::error::{CoreError, CoreResult};
use crate::core::ipc::{AppEvent, event_names};
use crate::db::Db;
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
        tasks: Arc<TaskService>,
        db: Arc<Db>,
        app: tauri::AppHandle,
    ) -> Self {
        Self {
            runner,
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
        let env = self.runner.environment().await;
        let path = env
            .path
            .ok_or_else(|| CoreError::Internal(env.hint.unwrap_or_else(|| "adb 不可用".into())))?;
        self.runner.run(&path, args, LIST_TIMEOUT).await
    }

    /// 设备列表（adb devices -l）
    pub async fn list_devices(&self) -> CoreResult<Vec<DeviceEntry>> {
        let args = adb::build_args(None, &adb::cmd_devices());
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb devices 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(adb::parse_devices(&out.stdout))
    }

    /// 设备信息（getprop 抽取常用字段）
    pub async fn device_info(&self, serial: &str) -> CoreResult<DeviceInfo> {
        let args = adb::build_args(Some(serial), &adb::cmd_getprop());
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "getprop 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(adb::device_info_from_props(
            serial,
            &adb::parse_getprop(&out.stdout),
        ))
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
    pub async fn list_packages(&self, serial: &str) -> CoreResult<Vec<String>> {
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
            self.known.lock().expect("known lock").clear();
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
        drop(known);
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

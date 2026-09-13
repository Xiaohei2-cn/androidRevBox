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

    /// 设备信息（getprop 常用字段 + wlan0 IP）
    pub async fn device_info(&self, serial: &str) -> CoreResult<DeviceInfo> {
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
        info.ip = self.device_ip(serial).await.ok().flatten();
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

    /// 设备 wlan0 IPv4：`adb -s <serial> shell ip addr show wlan0`
    /// （用户指定命令；解析不到返回 None——未连 Wi-Fi / 双卡数据流量）
    pub async fn device_ip(&self, serial: &str) -> CoreResult<Option<String>> {
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
        let out = self.run_adb(&args).await?;
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
        let out = self.run_adb(&args).await?;
        if out.exit_code != Some(0) {
            return Err(CoreError::Internal(format!(
                "adb forward --list 失败: {}",
                out.stderr.trim()
            )));
        }
        Ok(adb::parse_forward_list(&out.stdout)
            .into_iter()
            .map(|(s, l, r)| ForwardRule { serial: s, local: l, remote: r })
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
        let out = self.run_adb(&args).await?;
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

    /// 终止托管进程：`kill -9 <pid>`（pid 仅接受纯数字；root 进程需 su 终止）。
    pub async fn hosted_kill(&self, serial: &str, pid: u32, root: bool) -> CoreResult<()> {
        let c = format!("kill -9 {pid}");
        let cmd = if root { adb::su_wrap(&c) } else { c };
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

    /// 查任意进程监听端口（PID→端口方向；pid 不限于托管行，复用同链路）。
    pub async fn process_ports(
        &self,
        serial: &str,
        pid: u32,
        root: bool,
    ) -> CoreResult<Vec<adb::ListenPort>> {
        self.hosted_ports(serial, pid, root).await
    }

    /// 端口→PID 反查（两步链，全 -s 绑定）：
    /// ① grep 端口十六进制 → 解析取 LISTEN 且端口精确匹配的 inode；
    /// ② inode_owner_cmd 扫 /proc/[0-9]*/fd 找持有者（同行输出 pid+comm）。
    /// ⚠️ 不开 root 时 shell 用户读不到别人的 /proc/<pid>/fd，
    /// 只能命中 shell 自属进程——前端默认引导勾选 Root。
    pub async fn pids_by_port(
        &self,
        serial: &str,
        port: u16,
        root: bool,
    ) -> CoreResult<Vec<adb::PortHolder>> {
        let wrap = |c: String| if root { adb::su_wrap(&c) } else { c };

        let args =
            adb::build_args(Some(serial), &adb::cmd_shell(&wrap(adb::port_grep_cmd(port))));
        let out = self.run_adb(&args).await?;
        let mut inodes: Vec<u64> = adb::parse_proc_net_entries(&out.stdout)
            .into_iter()
            .filter(|e| e.listen && e.listen_port.port == port && e.inode != 0)
            .map(|e| e.inode)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        if inodes.is_empty() {
            return Ok(Vec::new());
        }
        inodes.sort_unstable();

        let args = adb::build_args(
            Some(serial),
            &adb::cmd_shell(&wrap(adb::inode_owner_cmd(&inodes))),
        );
        let out = self.run_adb(&args).await?;
        Ok(adb::parse_port_holders(&out.stdout))
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

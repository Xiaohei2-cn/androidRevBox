//! HookService（P10 Frida 会话工作台）：js 工作目录扫描、runner CommandSpec 组装、
//! 前置检查聚合、远程端点探活。设计 docs/frida-console-design.md §3/§4/§6。
//!
//! 纪律：Command 薄层只做转发；纯逻辑（目录扫描规则、runner 参数、端点解析）
//! 分别进本文件（IO 侧）与 adapters::frida（纯参数侧）单测；
//! 脚本内容零进宿主命令行——只传绝对路径，由 runner 从磁盘读。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tauri::Manager;

use crate::adapters::frida;
use crate::core::error::{CoreError, CoreResult};
use crate::services::config_service::{ConfigService, KEY_HOOK_WORKDIR, KEY_PYTHON_PATH};
use crate::services::env_service::{EnvService, FridaEnv};
use crate::services::process_service::CommandSpec;
use crate::services::task_service::TaskService;

/// 单次会话最多输出的事件类型对齐：hook_js_list 目录项上限（§4 防爆目录）
const JS_LIST_CAP: usize = 500;
/// 远程 frida-server 端口探活超时
const REMOTE_PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);
/// frida 握手要 spawn python 并在设备上找 server，比 TCP 探活慢一个量级
const CHANNEL_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// runner 随包资源名（tauri.conf bundle.resources 落点）
const RUNNER_RESOURCE: &str = "scripts/frida_runner.py";

// ===== DTO =====

/// 一个可启动的本地 JS 脚本（§4 文件列表行）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsFileDto {
    pub name: String,
    pub path: String,
    pub size: i64,
    /// Unix 秒
    pub mtime: i64,
}

/// 前置检查链聚合（§3）：adb → python → frida →（远程模式）TCP 可达。
/// 前端按 ok 标红 + 按 hint 给修复跳转。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightDto {
    pub adb_ok: bool,
    pub adb_hint: Option<String>,
    pub python_ok: bool,
    pub python_hint: Option<String>,
    /// 生效的 python 解释器（runner 用它；None = 未配置/不可用）
    pub python_path: Option<String>,
    /// 结构化（轨道）会话的要求：**这个 Python 里必须有 frida 绑定包**（我们的 runner 走
    /// `import frida`）。以前这里挂的是 `FridaEnv::installed`——那句是"frida 或 frida-tools
    /// 任一即通过"，于是只装了 frida-tools 的机器前置显示绿灯、一点起会话就收到
    /// `ModuleNotFoundError: frida`。终端里那条路（官方 REPL）要的是 frida-tools，
    /// 它自己在真终端页判（`src/lib/fridaTerminal.ts`）：**两边分开判，谁缺谁说**（D091）。
    pub frida_ok: bool,
    pub frida_version: Option<String>,
    pub frida_hint: Option<String>,
    /// 远程模式探活结果；非远程模式为 None（不探）
    pub remote_ok: Option<bool>,
    pub remote_hint: Option<String>,
    /// runner 脚本是否随包可达
    pub runner_ok: bool,
    pub runner_hint: Option<String>,
    /// 与设备 frida-server 的真实握手结果。`None` = 这一项没探（Python 未就绪），
    /// 三态是必需的：把"没探"画成绿色对勾就是骗人，画成红色又是噪音。
    #[serde(default)]
    pub channel_ok: Option<bool>,
    #[serde(default)]
    pub channel_hint: Option<String>,
}

/// 握手探活结果（checked=false 表示连试都没试）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelProbe {
    pub checked: bool,
    pub ok: bool,
    pub hint: Option<String>,
}

impl ChannelProbe {
    fn skipped() -> Self {
        Self {
            checked: false,
            ok: false,
            hint: None,
        }
    }
}

/// 解析探活脚本的输出。只认 `OK ...` / `ERR <类型> <消息>` 两种形状；
/// 别的（空输出、python 自己崩了）一律算"没探成"，不猜成功。
pub fn parse_channel_probe(stdout: &str, host_frida: Option<&str>) -> (bool, Option<String>) {
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut words = line.split_whitespace();
    match words.next() {
        Some("OK") => (true, None),
        Some("ERR") => {
            let kind = words.next().unwrap_or("Error");
            let msg: String = line.splitn(3, ' ').nth(2).unwrap_or("").to_string();
            let tail = match host_frida {
                Some(v) => format!("（工具用的 frida 是 {v}，两端主版本必须一致）"),
                None => String::new(),
            };
            (
                false,
                Some(format!(
                    "连不上设备上的 frida-server：{kind} {msg}{tail}。常见原因是 server 没起、                     或它的主版本与工具用的 frida 不同"
                )),
            )
        }
        _ => (
            false,
            Some("握手探测没有任何输出（python 起不来或 frida 未装？）".to_string()),
        ),
    }
}

/// hook_session_start 入参（命令层薄校验后传入）
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookSessionStartArgs {
    /// USB 模式：adb serial（远程模式为 None）
    pub serial: Option<String>,
    /// 远程模式：host:port（USB 模式为 None）
    pub remote: Option<String>,
    pub spawn: bool,
    /// 包名（spawn）/ 包名或 pid（attach）
    pub target: String,
    /// 工作目录下的 .js 文件名
    pub script: String,
}

pub struct HookService {
    config: std::sync::Arc<ConfigService>,
    env: std::sync::Arc<EnvService>,
    tasks: std::sync::Arc<TaskService>,
    app: tauri::AppHandle,
}

/// 结构化会话对 frida 环境的要求（纯函数，便于把四种组合都测到）：
/// 返回 `(能不能用, 该显示的原因)`。
///
/// 四种组合里只有一种放行：**绑定包在**。另外三种各有自己的话：
/// - 只有 frida-tools ⇒ 明说"只有 tools"，并点出**终端 REPL 不受这条影响**
///   （不然她会把整个 frida 能力当成坏了，而那条路是真能跑的）；
/// - 两个都没有 ⇒ 沿用 EnvService 那句原文（它已经在说 pip install frida-tools），不在这里改写；
/// - Python 压根没就绪 ⇒ 也沿用原文那句（此时去谈装哪个包是误导）。
fn frida_binding_requirement(env: &FridaEnv) -> (bool, Option<String>) {
    if env.frida_version.is_some() {
        return (true, None);
    }
    if env.python_ready && env.frida_tools_version.is_some() {
        return (
            false,
            Some(
                "这个 Python 里只有 frida-tools，没有 Python 绑定包：我们的 runner 走 `import frida`，装一下 frida（pip install frida）。真终端里的官方 REPL 不受这条影响。"
                    .to_string(),
            ),
        );
    }
    (
        false,
        env.hint
            .clone()
            .or_else(|| Some("这个 Python 里没有 frida：pip install frida".to_string())),
    )
}

impl HookService {
    pub fn new(
        config: std::sync::Arc<ConfigService>,
        env: std::sync::Arc<EnvService>,
        tasks: std::sync::Arc<TaskService>,
        app: tauri::AppHandle,
    ) -> Self {
        Self {
            config,
            env,
            tasks,
            app,
        }
    }

    /// 已配置的工作目录（空 = 未选择）。
    pub fn workdir(&self) -> String {
        self.config
            .get(KEY_HOOK_WORKDIR, "")
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    // ===== 脚本区（§4） =====

    /// 扫描工作目录一层 *.js（非递归、忽略符号链接、上限 500、名字过白名单）。
    /// 目录非法（不存在/非目录）返回 Err，前端提示重新选择。
    pub fn list_js(&self, dir: &str) -> CoreResult<Vec<JsFileDto>> {
        scan_js_dir(Path::new(dir.trim()))
    }

    /// 选择/更新工作目录前回探：必须是存在的目录。
    pub fn validate_workdir(&self, dir: &str) -> CoreResult<()> {
        let dir = dir.trim();
        if dir.is_empty() || !Path::new(dir).is_dir() {
            return Err(CoreError::Internal(format!("工作目录不存在: {dir}")));
        }
        Ok(())
    }

    // ===== 前置检查链（§3） =====

    /// 聚合探测 adb/python/frida（§10 剪枝由 EnvService 承担：Python 未配置
    /// 不发 frida 探测子进程）。remote 非空时追加 TCP 探活。
    pub async fn preflight(&self, remote: Option<&str>, serial: Option<&str>) -> PreflightDto {
        let (adb, (python, frida_env)) = tokio::join!(self.env.adb(), self.env.python_and_frida());
        // 握手探活放在版本探测之后：hint 里要带上"工具用的 frida 是哪一版"，
        // 用户看一眼就知道是不是两端版本不一致，而不是自己去猜
        let channel = self
            .probe_channel(serial, remote, frida_env.frida_version.as_deref())
            .await;
        let (remote_ok, remote_hint) = match remote {
            Some(endpoint) => {
                let (ok, hint) = self.probe_remote(endpoint).await;
                (Some(ok), Some(hint))
            }
            None => (None, None),
        };
        let runner = self.runner_path();
        let binding = frida_binding_requirement(&frida_env);
        PreflightDto {
            adb_ok: adb.installed,
            adb_hint: if adb.installed {
                None
            } else {
                Some(adb.hint.unwrap_or_else(|| "adb 不可用".into()))
            },
            python_ok: python.ready,
            python_hint: python.hint.clone(),
            python_path: python.path.clone(),
            frida_ok: binding.0,
            frida_version: frida_env.frida_version.clone(),
            frida_hint: binding.1,
            remote_ok,
            remote_hint,
            channel_ok: channel.checked.then_some(channel.ok),
            channel_hint: channel.hint,
            runner_ok: runner.is_some(),
            runner_hint: if runner.is_some() {
                None
            } else {
                Some(format!(
                    "未找到随包 runner（{RUNNER_RESOURCE}）：开发环境请确认 workspace scripts/ 目录"
                ))
            },
        }
    }

    /// frida 通道握手探活：**用工具配置的那个解释器**去连设备上的 frida-server，
    /// 成功标准是"真问得出一件事"（枚举进程），不是 TCP 端口活着。
    ///
    /// 为什么必须有这条：端口活着但版本不匹配 / server 没起 / 起的是 shell 身份够不着别的进程，
    /// 这三种情况在旧的 preflight 里全绿，用户只能对着"连不上"猜。
    /// 而且必须用 runner 同一个解释器探——版本结论只有对同一个 python 才成立
    /// （踩过：拿登录 shell 的 python 量出 17.x，工具用的 venv 其实是 16.5.7）。
    pub async fn probe_channel(
        &self,
        serial: Option<&str>,
        remote: Option<&str>,
        host_frida: Option<&str>,
    ) -> ChannelProbe {
        let python = self.env.python().await;
        if !python.ready {
            // Python 都没配好，握手谈不上：不显示这一项（而不是报一句红字噪音）
            return ChannelProbe::skipped();
        }
        let target = match (remote, serial) {
            (Some(ep), _) => ("remote".to_string(), ep.to_string()),
            (None, Some(sn)) if !sn.trim().is_empty() => ("usb".to_string(), sn.trim().to_string()),
            _ => ("none".to_string(), String::new()),
        };
        if target.0 == "none" {
            return ChannelProbe {
                checked: true,
                ok: false,
                hint: Some("还没选设备：USB 模式要在设备列表里选一台".to_string()),
            };
        }
        let script = frida::CHANNEL_PROBE_SCRIPT;
        let out = match crate::services::env_service::run_probe_with_timeout(
            &python.path.unwrap_or_default(),
            &["-c", script, &target.0, &target.1],
            CHANNEL_PROBE_TIMEOUT,
        )
        .await
        {
            Ok(out) => out,
            Err(error) => {
                return ChannelProbe {
                    checked: true,
                    ok: false,
                    hint: Some(format!("握手探测没跑起来：{error}")),
                };
            }
        };
        let (ok, hint) = parse_channel_probe(&out.stdout, host_frida);
        ChannelProbe {
            checked: true,
            ok,
            hint,
        }
    }

    /// 探测 host:port TCP 可达（远程模式 = 端口转发是否打通）。连不上是常态。
    pub async fn probe_remote(&self, endpoint: &str) -> (bool, String) {
        let (host, port) = match frida::parse_remote_endpoint(endpoint) {
            Ok(v) => v,
            Err(e) => return (false, e.to_string()),
        };
        let addr = format!("{host}:{port}");
        use tokio::net::TcpStream;
        match tokio::time::timeout(REMOTE_PROBE_TIMEOUT, TcpStream::connect(&addr)).await {
            Ok(Ok(_)) => (true, format!("{addr} 可达")),
            Ok(Err(e)) => (
                false,
                format!("{addr} 连接失败: {e}（去「端口转发」建 tcp:{port} → tcp:{port}）"),
            ),
            Err(_) => (
                false,
                format!("{addr} 连接超时（去「端口转发」建 tcp:{port} → tcp:{port}）"),
            ),
        }
    }

    // ===== 会话启动（§6.1/§6.2） =====

    /// 组 CommandSpec 起 runner 任务，返回 taskId（kind="frida"，输出/回放/取消
    /// 全部复用 TaskService 链路）。停止 = task_cancel，杀 runner 即 detach。
    pub async fn start_session(&self, args: HookSessionStartArgs) -> CoreResult<String> {
        let runner = self
            .runner_path()
            .ok_or_else(|| CoreError::Internal("未找到 frida runner 脚本".into()))?;
        let python = self
            .config
            .get(KEY_PYTHON_PATH, "")
            .map(|v| v.trim().to_string())
            .ok()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| CoreError::Internal("未配置 Python 解释器（设置 → 工具环境）".into()))?;
        if !Path::new(&python).is_file() {
            return Err(CoreError::Internal(format!(
                "Python 解释器不可用: {python}"
            )));
        }

        let script_name = args.script.trim();
        if !frida::is_safe_js_script_name(script_name) {
            return Err(CoreError::Internal(format!(
                "脚本文件名非法（仅字母数字与 _.-、不以 . 或 - 开头、.js 结尾）: {script_name}"
            )));
        }
        let workdir = self.workdir();
        if workdir.is_empty() {
            return Err(CoreError::InvalidInput(
                "还没选择脚本工作目录（设置 → Hook → 工作目录）".into(),
            ));
        }
        let workdir_abs = Path::new(&workdir)
            .canonicalize()
            .map_err(|_| CoreError::InvalidInput(format!("工作目录不可访问: {workdir}")))?;
        let script_abs = workdir_abs.join(script_name);
        let script_abs = script_abs.canonicalize().map_err(|_| {
            CoreError::InvalidInput(format!("脚本不存在: {}", script_abs.display()))
        })?;
        // canonicalize 解析符号链接后必须仍落在工作目录一层内（符号链接逃逸防线）
        if script_abs.parent() != Some(workdir_abs.as_path()) || !script_abs.is_file() {
            return Err(CoreError::InvalidInput(
                "脚本必须是所选工作目录下的普通 .js 文件（符号链接已拒绝）".into(),
            ));
        }

        let usb = args
            .serial
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let remote = args
            .remote
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if usb.is_none() && remote.is_none() {
            return Err(CoreError::InvalidInput(
                "还没选择设备或远程端点：USB 模式需要先在列表里选一台设备".into(),
            ));
        }
        if let Some(ep) = remote {
            frida::parse_remote_endpoint(ep)
                .map_err(|_| CoreError::Internal(format!("远程端点非法（host:port）: {ep}")))?;
        }
        let script_str = script_abs.to_string_lossy().into_owned();
        let runner_str = runner.to_string_lossy().into_owned();
        // attach + 目标留空 = 附加设备当前前台（frida -UF 的 -F）。这里不拦也不报错：
        // 前台是谁由 runner 在设备上问 frida-server，ready 行会把解析出的包名带回来。
        let target = frida::resolve_target(args.spawn, &args.target);
        let spec_args = frida::build_runner_args(&runner_str, usb, remote, &target, &script_str)?;

        let spec = CommandSpec {
            // frida 会话不留宿主临时文件，回收点恒为空
            cleanup_paths: Vec::new(),
            executable: python.clone(),
            args: spec_args,
            cwd: Some(workdir.clone()),
            timeout: None, // 会话由用户停止（task_cancel → SIGTERM → runner detach）
            env_extra: HashMap::new(),
            hide_window: true, // Windows 防黑窗（§6.2）
        };
        // 可读会话名（§5.4）：spawn com.x · hook.js / attach 1234 · hook.js
        // 可读会话名（§5.4）。前台模式在启动那一刻还不知道是谁，写 frontmost 而不是
        // 留个空位——任务列表里一个空目标看起来像出了错。
        let (mode, who) = match &target {
            frida::FridaTarget::Frontmost => ("attach", "frontmost".to_string()),
            frida::FridaTarget::Attach(t) => ("attach", t.clone()),
            frida::FridaTarget::Spawn(t) => ("spawn", t.clone()),
        };
        let name = format!("frida {mode} {who} · {script_name}");
        let task_id = self.tasks.start_with_kind_named("frida", &name, spec)?;
        tracing::info!(
            task_id = %task_id,
            mode,
            target = %args.target,
            script = %script_str,
            usb = ?usb,
            remote = ?remote,
            "frida 会话启动"
        );
        Ok(task_id)
    }

    /// runner 路径解析：① 随包资源（打包态）② workspace scripts/（dev 态）。
    /// 两处都不可得返回 None（preflight 标红）。
    fn runner_path(&self) -> Option<PathBuf> {
        if let Ok(resolved) = self
            .app
            .path()
            .resolve(RUNNER_RESOURCE, tauri::path::BaseDirectory::Resource)
        {
            if resolved.is_file() {
                return Some(resolved);
            }
        }
        let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .map(|root| root.join("scripts").join("frida_runner.py"));
        dev.filter(|p| p.is_file())
    }
}

/// 扫描规则（纯函数，单测覆盖）：一层目录、普通文件、.js 结尾、
/// 名字过白名单（隐藏/空格/元字符不进列表）、忽略符号链接、上限截断。
fn scan_js_dir(dir: &Path) -> CoreResult<Vec<JsFileDto>> {
    if !dir.is_dir() {
        return Err(CoreError::Internal(format!(
            "工作目录不存在或不是目录: {}",
            dir.display()
        )));
    }
    let read = std::fs::read_dir(dir)
        .map_err(|e| CoreError::Internal(format!("读取目录失败 {}: {e}", dir.display())))?;
    let mut out: Vec<JsFileDto> = Vec::new();
    for entry in read.flatten() {
        // 符号链接（含指向 .js 的）一律忽略：启动时 canonicalize 白名单同源防线
        let Ok(meta) = entry.file_type() else {
            continue;
        };
        if meta.is_symlink() || !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !frida::is_safe_js_script_name(&name) {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        out.push(JsFileDto {
            name: name.clone(),
            path: dir.join(&name).to_string_lossy().into_owned(),
            size: md.len() as i64,
            mtime,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(JS_LIST_CAP);
    Ok(out)
}

// ===== 用户主动导出的文本落本机盘（第七十四轮：控制台「保存日志」） =====
//
// 为什么写成模块级自由函数而不是 `HookService` 的方法：这条路径**不需要** AppHandle，
// 而它需要被单测（路径把关与"写完回读自证"两条都必须能脱离 tauri 跑）。
// 挂在方法上就得先造一个 AppHandle，那是把测试挡住，不是把逻辑藏好。

/// 导出体积上限。导出的正文是脚本输出，几 GB 的内存 dump 真有人导；
/// 那种文件既打不开也会把磁盘写满——**拒掉并把原因说清**比"写一半成功"诚实。
const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;

/// 扩展名白名单。选择器里用户可能手打成 `.sh` / `.js`：工具不该因为"用户点了保存"
/// 就往磁盘上放一个将来会被执行的东西。白名单只放文本类扩展名。
const EXPORT_EXTENSIONS: &[&str] = &["log", "txt", "json", "jsonl", "csv", "md"];

/// 纯函数：路径形状把关。判据只有三条，但每条都要能单测。
fn validate_export_path(path: &str) -> Result<PathBuf, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("没有选定保存路径".into());
    }
    if trimmed.contains('\0') {
        return Err("保存路径里含非法字符".into());
    }
    let candidate = Path::new(trimmed);
    if !candidate.is_absolute() {
        return Err(format!("保存路径必须是完整路径，收到：{trimmed}"));
    }
    let ext = candidate
        .extension()
        .and_then(|v| v.to_str())
        .map(|v| v.to_ascii_lowercase())
        .unwrap_or_default();
    if !EXPORT_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!(
            "只支持保存为 {}，你把文件名写成了 .{ext}（保存对话框里改回扩展名）",
            EXPORT_EXTENSIONS.join(" / ")
        ));
    }
    Ok(candidate.to_path_buf())
}

/// 纯函数：体积把关（上限做成参数，测试才能用小数字跑到那条分支）。
fn check_export_size(content_len: usize, cap: usize) -> Result<(), String> {
    if content_len > cap {
        return Err(format!(
            "这份日志 {} 字节，超过单次保存上限 {} 字节；先用过滤条缩小范围，或分几次保存",
            content_len, cap
        ));
    }
    Ok(())
}

/// 把用户主动导出的文本写到他选的**本机**路径。
///
/// 判据是磁盘上真的有这份文件：写完**回读**比对字节数，对不上就报错——
/// "我调用过 write"不是完成（UI-6 那条纪律的同一条口径）。
/// 只在用户点了「保存日志」并在选择器里确认路径之后才被调用，
/// 程序自己**不会**悄悄把脚本正文落到任何地方（见重构文档 §3.7）。
pub fn save_export(path: &str, content: &str) -> CoreResult<u64> {
    let target = validate_export_path(path).map_err(CoreError::InvalidInput)?;
    check_export_size(content.len(), MAX_EXPORT_BYTES).map_err(CoreError::InvalidInput)?;
    if let Some(parent) = target.parent() {
        if !parent.exists() {
            return Err(CoreError::InvalidInput(format!(
                "目标目录已经不在了：{}",
                parent.display()
            )));
        }
    }
    std::fs::write(&target, content.as_bytes())
        .map_err(|error| CoreError::Internal(format!("写入 {} 失败：{error}", target.display())))?;
    let written = std::fs::metadata(&target)
        .map_err(|error| {
            CoreError::Internal(format!(
                "写完读不回来，不能确认文件真的存在（{}）：{error}",
                target.display()
            ))
        })?
        .len();
    if written as usize != content.len() {
        return Err(CoreError::Internal(format!(
            "文件大小对不上：界面交给我的是 {} 字节，磁盘上是 {written} 字节",
            content.len()
        )));
    }
    tracing::info!(
        path = %target.display(),
        bytes = written,
        "用户主动导出日志到本机（内容不进应用日志）"
    );
    Ok(written)
}

#[cfg(test)]
mod tests {

    fn frida_env(python_ready: bool, binding: Option<&str>, tools: Option<&str>) -> FridaEnv {
        FridaEnv {
            python_ready,
            installed: binding.is_some() || tools.is_some(),
            frida_version: binding.map(str::to_string),
            frida_tools_version: tools.map(str::to_string),
            hint: match (binding, tools) {
                (Some(_), _) => None,
                (None, Some(_)) => None,
                (None, None) => Some(
                    "该 Python 环境未安装 frida / frida-tools（pip install frida-tools）。".into(),
                ),
            },
        }
    }

    /// 四种组合都钉住。**只有绑定包在**才放行结构化会话：今天那句"任一即通过"
    /// 会在这台机器上显示绿灯，然后 runner 报 `ModuleNotFoundError: frida`。
    #[test]
    fn frida_requirement_for_structured_session_is_the_python_binding() {
        // ① 绑定包在（不管 tools 在不在）⇒ 放行，且不给原因
        assert!(frida_binding_requirement(&frida_env(true, Some("16.5.1"), None)).0);
        let both = frida_binding_requirement(&frida_env(true, Some("16.5.1"), Some("12.5.3")));
        assert_eq!((both.0, both.1.as_deref()), (true, None));

        // ② 只有 frida-tools ⇒ 不许放行，并且要说清"缺的是绑定包"
        let only_tools = frida_binding_requirement(&frida_env(true, None, Some("12.5.3")));
        assert!(!only_tools.0, "只有 frida-tools 时结构化会话必须判不通过");
        let reason = only_tools.1.expect("得给出原因，不能只亮红灯");
        assert!(reason.contains("frida-tools"), "{reason}");
        assert!(reason.contains("绑定包"), "{reason}");
        // 顺带点明终端那条路没坏——否则她会把整个 frida 能力当成坏了
        assert!(reason.contains("REPL"), "{reason}");

        // ③ 两个都没有 ⇒ 沿用 EnvService 那句原文，不在这里改写它
        let none = frida_binding_requirement(&frida_env(true, None, None));
        assert!(!none.0);
        assert_eq!(
            none.1.as_deref(),
            Some("该 Python 环境未安装 frida / frida-tools（pip install frida-tools）。")
        );

        // ④ Python 压根没就绪 ⇒ 也沿用原文那句（此时谈装哪个包是误导）
        let no_python = frida_binding_requirement(&FridaEnv {
            python_ready: false,
            installed: false,
            frida_version: None,
            frida_tools_version: None,
            hint: Some("Python 环境未就绪，无法检测 Frida。".into()),
        });
        assert!(!no_python.0);
        assert_eq!(
            no_python.1.as_deref(),
            Some("Python 环境未就绪，无法检测 Frida。")
        );
    }

    /// 这条扫描钉的是**调用点**：结构体的 ok 必须来自那个分开判断，
    /// 一旦被改回 `frida_env.installed`（任一即通过），这里就会红。
    #[test]
    fn preflight_no_longer_uses_the_any_one_passes_aggregate() {
        let source = include_str!("hook_service.rs");
        let at = source
            .find("frida_ok:")
            .expect("preflight 里必须还在报这一项");
        let line = source[at..].lines().next().unwrap_or_default();
        assert!(
            !line.contains("frida_env.installed"),
            "frida_ok 又用回聚合位了：{line}"
        );
    }

    /// 路径把关：这三条判据都得能脱离文件系统被钉住。
    #[test]
    fn export_path_rules_reject_relative_empty_nul_and_executable_extensions() {
        assert!(validate_export_path("").is_err());
        assert!(validate_export_path("   ").is_err());
        assert!(validate_export_path("out/weird\u{0}.log").is_err());
        // 相对路径：选择器不会给，但手拼/降级路径会给，必须拒
        assert!(validate_export_path("logs/frida.log").is_err());
        // 可执行扩展名：保存日志不该顺手往磁盘放一个将来会被执行的东西
        assert!(validate_export_path("/tmp/frida.sh").is_err());
        assert!(validate_export_path("/tmp/frida").is_err());
        for ok in ["/tmp/a.log", "/tmp/a.TXT", "/tmp/a.jsonl", "/tmp/a.csv"] {
            assert!(validate_export_path(ok).is_ok(), "{ok} 应当被接受");
        }
    }

    #[test]
    fn export_size_check_rejects_oversized_payload_before_touching_disk() {
        // 上限做成参数就是为了这条腿能用小数字跑到拒绝分支
        assert!(check_export_size(8, 8).is_ok());
        let error = check_export_size(9, 8).unwrap_err();
        assert!(
            error.contains("9 字节") && error.contains("8 字节"),
            "{error}"
        );
        assert!(
            error.contains("过滤"),
            "拒绝必须给出出路，不只是说太大了：{error}"
        );
    }

    /// 写成功要有磁盘上的证据：回读到的字节数必须等于交过来的字节数。
    #[test]
    fn export_write_roundtrip_is_proved_by_reading_the_file_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("frida.log");
        let content = "00000000  48 65 6c 6c 6f\n";
        let written = save_export(&file.display().to_string(), content).unwrap();
        assert_eq!(written as usize, content.len());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), content);
    }

    /// 目标目录没了（用户在别的窗口删了文件夹）：要说清是哪条路径，不能只回一个 os error。
    #[test]
    fn export_write_failure_names_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("already-gone").join("frida.log");
        let error = save_export(&gone.display().to_string(), "x").unwrap_err();
        assert!(
            error.to_string().contains("already-gone"),
            "错误里必须带上那条路径：{error}"
        );
        assert!(!gone.exists(), "失败时不该留下半个文件");
    }

    use super::*;

    fn touch(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn scan_js_filters_non_js_dirs_hidden_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        touch(dir, "hook-ssl.js", "// x");
        touch(dir, "agent2.js", "// y");
        touch(dir, "notes.txt", "no");
        touch(dir, ".hidden.js", "no");
        touch(dir, "bad name.js", "no");
        std::fs::create_dir(dir.join("sub.js")).unwrap();
        let subdir = dir.join("sub.js");
        touch(&subdir, "deep.js", "// z");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("hook-ssl.js"), dir.join("link.js")).unwrap();

        let files = scan_js_dir(dir).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["agent2.js", "hook-ssl.js"],
            "只收普通 .js，按名排序"
        );
        assert!(files[0].size > 0);
        assert!(files[0].mtime > 0);
    }

    #[test]
    fn scan_js_missing_dir_is_error() {
        assert!(scan_js_dir(Path::new("/definitely/not/a/dir-xyz")).is_err());
    }

    #[test]
    fn scan_js_cap_500() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..505 {
            touch(tmp.path(), &format!("s{i:03}.js"), "//");
        }
        let files = scan_js_dir(tmp.path()).unwrap();
        assert_eq!(files.len(), JS_LIST_CAP);
    }
    #[test]
    fn channel_probe_only_trusts_its_own_output_shapes() {
        // 成功：脚本问出了进程数
        let (ok, hint) = parse_channel_probe("OK 128 USB 18271FDF\n", Some("16.5.7"));
        assert!(ok);
        assert_eq!(hint, None);
        // 失败：把 frida 的原话与"工具用的版本"一起带出来，用户不必自己猜不匹配
        let (ok, hint) = parse_channel_probe(
            "ERR ProtocolError unable to communicate with remote frida-server\n",
            Some("16.5.7"),
        );
        assert!(!ok);
        let hint = hint.expect("失败必须给原因");
        assert!(hint.contains("unable to communicate"), "{hint}");
        assert!(hint.contains("16.5.7"), "要带出工具用的 frida 版本：{hint}");
        // 没探成（空输出/ python 崩了）不算成功，也不算"端口没开"
        let (ok, hint) = parse_channel_probe("", None);
        assert!(!ok);
        let hint = hint.expect("探不出结果也要说清为什么");
        assert!(hint.contains("任何输出"), "{hint}");
        // 乱码输出同样落回"没探成"，绝不猜成成功
        let (ok, _) = parse_channel_probe("Traceback (most recent call last):", None);
        assert!(!ok);
    }

    #[test]
    fn channel_probe_is_skipped_rather_than_faked_when_python_is_not_ready() {
        // checked=false → DTO 里是 None → 界面上根本不出现这一项
        let skipped = ChannelProbe::skipped();
        assert!(!skipped.checked);
        let as_dto: Option<bool> = skipped.checked.then_some(skipped.ok);
        assert_eq!(as_dto, None);
        // 探过但失败 → Some(false)（画红），与"没探"分得开
        let probed = ChannelProbe {
            checked: true,
            ok: false,
            hint: Some("x".into()),
        };
        assert_eq!(probed.checked.then_some(probed.ok), Some(false));
    }
}

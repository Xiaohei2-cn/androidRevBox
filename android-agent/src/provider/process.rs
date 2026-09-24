//! ProcessesProvider（AR6.2）：端口/进程互查在设备端一次快照完成。
//!
//! 迁移前 Desktop 需要 `cat /proc/net/tcp{,6}` 拉全文 + 宿主侧 hex 解析，端口反查进程
//! 还要分批 `ls -l /proc/<pid>/fd`（一次查询几十条 shell）。这里改为：直接读
//! `/proc/net/*` 与 `/proc/<pid>/fd` 符号链接，在 Agent 内完成 inode→pid 归属匹配。
//! 读不到的项一律如实上报（`unreadable` / `skipped` / `truncated`），
//! 不把「没权限看」说成「没有端口」。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use agent_protocol::method::{PROCESS_BY_PORT, PROCESS_KILL, PROCESS_PORTS, PROCESS_PROC_READ};
use agent_protocol::{
    AgentError, ErrorCode, KillOutcome, KillSignal, ListeningPort, PortHoldingProcess, ProcFile,
    ProcessByPortParams, ProcessByPortResult, ProcessKillParams, ProcessKillResult,
    ProcessPortsParams, ProcessPortsResult, ProcessProcReadParams, ProcessProcReadResult,
    ProviderHealth, ProviderInfo, SocketFamily,
};
use serde_json::Value;

use super::privileged;
use super::{Provider, ProviderFuture, RequestContext};

const PROCESS_METHODS: &[&str] = &[
    PROCESS_PORTS,
    PROCESS_BY_PORT,
    PROCESS_KILL,
    PROCESS_PROC_READ,
];
/// 终止后的确认轮询：有界，不做无界等待（卡住的写操作比慢的更危险）。
const KILL_VERIFY_TIMEOUT: Duration = Duration::from_millis(1_500);
const KILL_VERIFY_POLL: Duration = Duration::from_millis(50);
const NET_FILES: [(&str, SocketFamily); 2] = [
    ("/proc/net/tcp", SocketFamily::Ipv4),
    ("/proc/net/tcp6", SocketFamily::Ipv6),
];
const PROC: &str = "/proc";
/// 按需读取的默认/上限行数与字节数：宁可标 `truncated`，也不把一次界面点击变成
/// 无界的大文件传输（maps 上万行很常见）。
const DEFAULT_PROC_LINES: u32 = 400;
const MAX_PROC_LINES: u32 = 5_000;
const MAX_PROC_BYTES: u64 = 4 * 1024 * 1024;
/// 行数与扫描进程数上限：极端设备上宁可标 truncated，也不无界扫描。
const MAX_SOCKET_ROWS: usize = 200_000;
const MAX_SCANNED_PIDS: usize = 4_000;

pub struct ProcessesProvider;

impl Provider for ProcessesProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "process".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: vec!["shell".into()],
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        PROCESS_METHODS
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                PROCESS_PORTS => self.ports(params).await,
                PROCESS_BY_PORT => self.by_port(params).await,
                PROCESS_KILL => self.kill(params).await,
                PROCESS_PROC_READ => self.proc_read(params).await,
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported process method: {method}"),
                )),
            }
        })
    }
}

#[derive(Debug, Clone)]
struct NetEntry {
    inode: u64,
    local_port: u16,
    local_address: String,
    remote_port: u16,
    state: String,
    uid: u32,
    family: SocketFamily,
}

#[derive(Debug, Default)]
struct NetSnapshot {
    entries: Vec<NetEntry>,
    truncated: bool,
    unreadable: Vec<String>,
}

impl ProcessesProvider {
    /// PID -> 监听端口：该进程 fd 持有的 socket inode 与 /proc/net 快照求交。
    async fn ports(&self, params: Value) -> Result<Value, AgentError> {
        let params: ProcessPortsParams = parse_params(params)?;
        let snapshot = load_snapshot().await;
        let mut unreadable = snapshot.unreadable.clone();
        let owned: HashSet<u64> = match socket_inodes_of(params.pid).await {
            Ok(inodes) => inodes,
            Err(reason) => {
                unreadable.push(format!("/proc/{}/fd: {reason}", params.pid));
                HashSet::new()
            }
        };
        let ports: Vec<ListeningPort> = snapshot
            .entries
            .iter()
            .filter(|entry| owned.contains(&entry.inode))
            .map(|entry| ListeningPort {
                port: entry.local_port,
                address: entry.local_address.clone(),
                family: entry.family,
                state: entry.state.clone(),
                inode: entry.inode,
                uid: entry.uid,
            })
            .collect();
        serialize(ProcessPortsResult {
            pid: params.pid,
            comm: read_trimmed(&format!("{PROC}/{}/comm", params.pid)).await,
            cmdline: read_cmdline(&format!("{PROC}/{}/cmdline", params.pid)).await,
            ports,
            unreadable,
            truncated: snapshot.truncated,
        })
    }

    /// 端口 -> 持有进程：先筛本地端口的监听 socket，再用 fd→inode 反查属主。
    async fn by_port(&self, params: Value) -> Result<Value, AgentError> {
        let params: ProcessByPortParams = parse_params(params)?;
        if params.port == 0 {
            return Err(AgentError::new(ErrorCode::InvalidRequest, "port 不能为 0"));
        }
        let snapshot = load_snapshot().await;
        let matching: Vec<&NetEntry> = snapshot
            .entries
            .iter()
            .filter(|entry| entry.local_port == params.port && entry.remote_port == 0)
            .collect();
        let wanted: HashSet<u64> = matching
            .iter()
            .map(|entry| entry.inode)
            .filter(|inode| *inode != 0)
            .collect();
        let owners = if wanted.is_empty() {
            OwnerScan::default()
        } else {
            scan_socket_owners(&wanted).await
        };
        let mut seen: HashSet<u64> = HashSet::new();
        let mut sockets = Vec::new();
        let mut unowned = Vec::new();
        for entry in &matching {
            match owners.owners.get(&entry.inode) {
                Some(pid) => {
                    if !seen.insert(entry.inode) {
                        continue;
                    }
                    sockets.push(PortHoldingProcess {
                        pid: *pid,
                        uid: entry.uid,
                        family: entry.family,
                        address: entry.local_address.clone(),
                        state: entry.state.clone(),
                        inode: entry.inode,
                        comm: read_trimmed(&format!("{PROC}/{pid}/comm")).await,
                    });
                }
                None => {
                    if !seen.insert(entry.inode) {
                        continue;
                    }
                    unowned.push(PortHoldingProcess {
                        // pid=0 表示 socket 存在但确定不了属主（权限或已退出），
                        // 必须配合 skipped 一起判断，不能当「没人监听」
                        pid: 0,
                        uid: entry.uid,
                        family: entry.family,
                        address: entry.local_address.clone(),
                        state: entry.state.clone(),
                        inode: entry.inode,
                        comm: None,
                    });
                }
            }
        }
        let mut skipped: Vec<String> = snapshot.unreadable.clone();
        if owners.unreadable > 0 {
            skipped.push(format!(
                "fd_unreadable_processes={}（非 root 时属主可能不全）",
                owners.unreadable
            ));
        }
        if owners.truncated {
            skipped.push(format!("fd_scan_truncated_at={MAX_SCANNED_PIDS}"));
        }
        serialize(ProcessByPortResult {
            port: params.port,
            sockets,
            unowned,
            truncated: snapshot.truncated || owners.truncated,
            skipped,
        })
    }

    /// PID -> 终止信号（AR6.3，写操作）。
    ///
    /// 三条硬规则：
    /// 1. **执行前重读身份**：Desktop 传来的 PID 只是意图，先读 `/proc/<pid>/comm`
    ///    与 cmdline，和 `expected_comm` 对不上就以 `precondition_failed` 拒止——
    ///    PID 复用窗口里盲发信号等于随机杀进程；
    /// 2. **不越权**：`require_root=true` 而 Agent 不是 root 时直接 `permission_denied`，
    ///    不去 `kill` 一下看运气（EPERM 的错误信息会让 UI 把权限问题说成进程问题）；
    /// 3. **幂等**：目标本来就不存在算 `already_gone` 成功返回，重复点击不报错。
    ///
    /// 信号用 `libc::kill` 直发，不孵化 `sh -c "kill -9 x"`；errno 映射成结构化错误码。
    /// 发完信号后有界轮询 `/proc/<pid>` 确认，确认不到只说「未确认」，不谎报已死。
    /// 按需读取 `/proc/<pid>/<file>` 的详情（设备信息页点箭头之后才会走到这里）。
    ///
    /// **先按 shell 身份试读，读不到才提权**：`cmdline`/`status` 以 shell 就读得到
    /// （真机实测），没必要为它们弹一次 su；`maps` 属于另一个用户的进程，Android 14 上
    /// shell 必然 `EACCES`，这时才走 `su`。结果里的 `read_via` 把这个选择如实带回去，
    /// 因为"这次是靠提权看到的"本身就是要显示给用户的信息。
    async fn proc_read(&self, params: Value) -> Result<Value, AgentError> {
        let params: ProcessProcReadParams = parse_params(params)?;
        let path = proc_path(params.pid, params.file)?;
        let max_lines = params
            .max_lines
            .unwrap_or(DEFAULT_PROC_LINES)
            .clamp(1, MAX_PROC_LINES);

        match read_proc_head(&path, params.file, max_lines).await {
            // `Readable` 直接用；`Empty` 只在 maps 上继续往上问（见 read_proc_head 的注释）
            ShellRead::Readable(read) => {
                return serialize(&ProcessProcReadResult {
                    path,
                    file: params.file,
                    total_lines: read.total_lines,
                    returned_lines: read.returned_lines,
                    truncated: read.truncated,
                    text: read.text,
                    read_via: "shell".to_owned(),
                });
            }
            ShellRead::Empty(read) if params.file != ProcFile::Maps => {
                return serialize(&ProcessProcReadResult {
                    path,
                    file: params.file,
                    total_lines: read.total_lines,
                    returned_lines: read.returned_lines,
                    truncated: read.truncated,
                    text: read.text,
                    read_via: "shell".to_owned(),
                });
            }
            ShellRead::Empty(_) | ShellRead::Unreadable => {}
        }
        // shell 读不到才提权。脚本里没有一个是外部给的路径：pid 校验过是数字、
        // 文件名来自协议枚举、行数是夹紧过的整数（D038）。
        let script = privileged::proc_read_script(params.pid, params.file.as_str(), max_lines);
        let output =
            privileged::run_privileged("读取 /proc 详情", &script, privileged::PROC_READ_SENTINEL)
                .await?;
        let read = parse_proc_read_output(&output)?;
        let truncated = read.total_lines > u64::from(max_lines) || read.body_truncated;
        serialize(&ProcessProcReadResult {
            path,
            file: params.file,
            total_lines: read.total_lines,
            returned_lines: if read.body.is_empty() {
                0
            } else {
                read.body.lines().count() as u32
            },
            truncated,
            text: read.body,
            read_via: "root".to_owned(),
        })
    }

    async fn kill(&self, params: Value) -> Result<Value, AgentError> {
        let params: ProcessKillParams = parse_params(params)?;
        // 先拒掉「一发信号打死一大片」的输入：pid=0 在 kill(2) 里是整进程组，
        // pid=1 是 init，pid=自身会把会话杀掉——三者都不允许经 Agent 发起。
        if params.pid == 0 {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                "pid=0 会命中整个进程组，禁止通过 Agent 终止",
            ));
        }
        if params.pid == 1 {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                "禁止终止 init（pid=1）",
            ));
        }
        if params.pid == std::process::id() {
            return Err(AgentError::new(
                ErrorCode::InvalidRequest,
                "禁止终止 Agent 自身，请使用 Agent 管理入口停止会话",
            ));
        }
        let ran_as_root = effective_uid() == Some(0);
        if params.require_root && !ran_as_root {
            return Err(AgentError::new(
                ErrorCode::PermissionDenied,
                "终止该进程需要 root，Agent 当前以 shell 身份运行",
            )
            .with_details(serde_json::json!({
                "reason": "root_required",
                "agent_uid": effective_uid().unwrap_or(u32::MAX),
            })));
        }
        let identity = read_process_identity(params.pid).await;
        guard_identity(params.pid, &params, identity.as_ref())?;
        let comm = identity
            .as_ref()
            .and_then(|value| value.comm.clone())
            .unwrap_or_default();
        let uid = identity.as_ref().and_then(|value| value.uid);
        if identity.is_none() {
            // 目标本来就不在：幂等成功，重复点击不报错
            audit(&params, "already_gone", None, "-", ran_as_root);
        }
        let outcome = match send_signal(params.pid, params.signal) {
            // 竞态窗口里进程刚好退出：与「本来就没了」同一处理，保持幂等
            SignalResult::Gone => KillOutcome::AlreadyGone,
            SignalResult::Denied => {
                audit(
                    &params,
                    "permission_denied",
                    uid,
                    comm.as_str(),
                    ran_as_root,
                );
                return Err(AgentError::new(
                    ErrorCode::PermissionDenied,
                    format!("无权限终止 pid={}", params.pid),
                )
                .with_details(serde_json::json!({
                    "reason": "not_owner",
                    "target_uid": uid,
                    "agent_uid": effective_uid().unwrap_or(u32::MAX),
                })));
            }
            SignalResult::Failed(errno) => {
                audit(
                    &params,
                    &format!("errno={errno}"),
                    uid,
                    comm.as_str(),
                    ran_as_root,
                );
                return Err(AgentError::new(
                    ErrorCode::Internal,
                    format!("kill 失败 pid={} errno={}", params.pid, errno),
                )
                .with_details(serde_json::json!({ "comm": comm, "uid": uid })));
            }
            SignalResult::Sent => {
                audit(&params, "signaled", uid, comm.as_str(), ran_as_root);
                KillOutcome::Signaled
            }
        };
        let verified_dead = match outcome {
            KillOutcome::AlreadyGone => true,
            KillOutcome::Signaled => confirm_dead(params.pid).await,
        };
        // 身份证据取「发信号之前」那一次：进程一死就读不到了，而审计要的正是
        // 「我们到底杀了谁」，不是事后空值。
        serialize(ProcessKillResult {
            pid: params.pid,
            signal: params.signal,
            outcome,
            comm: identity.as_ref().and_then(|value| value.comm.clone()),
            cmdline: identity.as_ref().and_then(|value| value.cmdline.clone()),
            uid: identity.as_ref().and_then(|value| value.uid),
            ran_as_root,
            verified_dead,
            detail: match (outcome, verified_dead) {
                (KillOutcome::Signaled, false) => Some("signal_sent_not_confirmed".to_string()),
                (KillOutcome::Signaled, true) => Some("signal_sent_confirmed".to_string()),
                (KillOutcome::AlreadyGone, _) => Some("process_not_running".to_string()),
            },
        })
    }
}

#[derive(Debug, Clone)]
struct ProcessIdentity {
    comm: Option<String>,
    cmdline: Option<String>,
    uid: Option<u32>,
}

/// 执行前重读 `/proc/<pid>`；`None` 表示进程不存在（幂等分支）。
async fn read_process_identity(pid: u32) -> Option<ProcessIdentity> {
    let comm = read_trimmed(&format!("{PROC}/{pid}/comm")).await;
    let cmdline = read_cmdline(&format!("{PROC}/{pid}/cmdline")).await;
    let status = tokio::fs::read_to_string(format!("{PROC}/{pid}/status"))
        .await
        .ok();
    let uid = status.as_deref().and_then(parse_status_uid);
    if comm.is_none() && cmdline.is_none() && uid.is_none() {
        return None;
    }
    Some(ProcessIdentity { comm, cmdline, uid })
}

fn parse_status_uid(status: &str) -> Option<u32> {
    status.lines().find_map(|line| {
        line.strip_prefix("Uid:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.trim().parse::<u32>().ok())
    })
}

/// 身份校验：只有调用方给了 `expected_comm` 才比对；不匹配即拒止（PID 复用防护）。
fn guard_identity(
    pid: u32,
    params: &ProcessKillParams,
    identity: Option<&ProcessIdentity>,
) -> Result<(), AgentError> {
    let Some(raw) = params.expected_comm.as_deref() else {
        return Ok(());
    };
    let expected = raw.trim();
    if expected.is_empty() {
        return Ok(());
    }
    let Some(identity) = identity else {
        // 进程不存在：无需比对，交给幂等分支
        return Ok(());
    };
    let actual = identity.comm.clone().unwrap_or_default();
    let program = identity
        .cmdline
        .as_deref()
        .and_then(|line| line.split_whitespace().next())
        .map(|path| path.rsplit('/').next().unwrap_or(path).to_owned())
        .unwrap_or_default();
    if actual == expected || program == expected {
        return Ok(());
    }
    Err(AgentError::new(
        ErrorCode::PreconditionFailed,
        format!("pid={pid} 的身份与预期不符，已拒绝终止"),
    )
    .with_details(serde_json::json!({
        "reason": "identity_mismatch",
        "expected": expected,
        "actual_comm": actual,
        "actual_program": program,
    })))
}

pub(crate) enum SignalResult {
    Sent,
    Gone,
    Denied,
    Failed(i32),
}

pub(crate) fn send_signal(pid: u32, signal: KillSignal) -> SignalResult {
    let raw = pid.try_into().map_err(|_| ()).unwrap_or(0_i32);
    if raw <= 0 {
        return SignalResult::Failed(libc::EINVAL);
    }
    let code = unsafe { libc::kill(raw, signal.number()) };
    if code == 0 {
        return SignalResult::Sent;
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => SignalResult::Gone,
        Some(libc::EPERM) | Some(libc::EACCES) => SignalResult::Denied,
        Some(errno) => SignalResult::Failed(errno),
        None => SignalResult::Failed(-1),
    }
}

/// 有界确认：`/proc/<pid>` 消失或只剩僵尸态（父进程未收割）算确认死亡。
pub(crate) async fn confirm_dead(pid: u32) -> bool {
    let started = std::time::Instant::now();
    loop {
        if !std::path::Path::new(&format!("{PROC}/{pid}")).exists() {
            return true;
        }
        if let Ok(status) = tokio::fs::read_to_string(format!("{PROC}/{pid}/status")).await
            && status.lines().any(|line| line.starts_with("State:	Z"))
        {
            return true;
        }
        if started.elapsed() >= KILL_VERIFY_TIMEOUT {
            return false;
        }
        tokio::time::sleep(KILL_VERIFY_POLL).await;
    }
}

/// `/proc/<pid>/<file>` 只由**校验过的数字 + 协议枚举**拼出来：这条路径会进提权脚本，
/// 所以不接受任何外部字符串。
fn proc_path(pid: u32, file: ProcFile) -> Result<String, AgentError> {
    if pid == 0 {
        return Err(AgentError::new(
            ErrorCode::InvalidRequest,
            "pid 不能是 0（0 在 /proc 里是 swapper，没有可读的 maps）",
        ));
    }
    Ok(format!("{PROC}/{pid}/{}", file.as_str()))
}

/// `cmdline` 是 NUL 分隔的一行；其余按文本读，非法字节替换而不是整条失败。
fn decode_proc_bytes(file: ProcFile, raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    match file {
        ProcFile::Cmdline => text.replace('\0', " ").trim().to_owned(),
        ProcFile::Maps | ProcFile::Status => text.into_owned(),
    }
}

fn take_lines(body: &str, max_lines: u32) -> (u64, String, bool) {
    let total = body.lines().count() as u64;
    let kept: Vec<&str> = body.lines().take(max_lines as usize).collect();
    let truncated = total > kept.len() as u64;
    (total, kept.join("\n"), truncated)
}

/// 以 shell 身份读 `/proc/<pid>/<file>` 的前若干行。
///
/// 返回 `None` = 读不到（`EACCES`/进程已退），调用方据此决定去提权；**不要**把它当成
/// "文件是空的"。读取带字节上限：极端 maps 能到十几 MB，一次界面点击不该无界吃内存，
/// 撞到上限时按已读部分给行数并如实标 `truncated`。
/// shell 身份试读 `/proc/<pid>/<file>` 的三种结果，**必须分开**：
/// `Readable` 正常返回；`Unreadable` 是"读不出"（EACCES、EISDIR…）；
/// `Empty` 是"真的一个字节都没有"。
///
/// 为什么要三态：Android 14 上 `/proc/<pid>/maps` 属于别的用户，shell **能 open、
/// 读的时候才失败**，有些 ROM/内核甚至直接给一个 EOF 当"读完了"。只看 `open` 或只看
/// "读完不报错"，就会把"我们没权限"渲染成"这个进程有 0 条映射"——那是凭缺失的证据编一个
/// 结论。maps 的正常值不可能是空（活的进程至少几十条），所以 maps 的 `Empty` 也当
/// "没读到"处理，交给提权路径去确认真相。
#[derive(Debug)]
enum ShellRead {
    Readable(ProcHeadRead),
    Empty(ProcHeadRead),
    Unreadable,
}

async fn read_proc_head(path: &str, file: ProcFile, max_lines: u32) -> ShellRead {
    use tokio::io::AsyncReadExt;
    let Ok(handle) = tokio::fs::File::open(path).await else {
        return ShellRead::Unreadable;
    };
    let mut raw = Vec::new();
    if handle
        .take(MAX_PROC_BYTES + 1)
        .read_to_end(&mut raw)
        .await
        .is_err()
    {
        return ShellRead::Unreadable;
    }
    let capped = raw.len() as u64 > MAX_PROC_BYTES;
    raw.truncate(usize::try_from(MAX_PROC_BYTES).unwrap_or(usize::MAX));
    let body = decode_proc_bytes(file, &raw);
    let (total, text, truncated_by_lines) = take_lines(&body, max_lines);
    let read = ProcHeadRead {
        total_lines: total,
        returned_lines: if text.is_empty() {
            0
        } else {
            text.lines().count() as u32
        },
        truncated: truncated_by_lines || capped,
        text,
    };
    if raw.is_empty() {
        ShellRead::Empty(read)
    } else {
        ShellRead::Readable(read)
    }
}

#[derive(Debug)]
struct ProcHeadRead {
    total_lines: u64,
    returned_lines: u32,
    truncated: bool,
    text: String,
}

#[derive(Debug)]
struct ProcReadOutput {
    total_lines: u64,
    body: String,
    body_truncated: bool,
}

/// 解析 `proc_read_script` 的输出。形状错了要报出来，不能悄悄给个空内容当"没有映射"。
fn parse_proc_read_output(output: &str) -> Result<ProcReadOutput, AgentError> {
    let lines: Vec<&str> = output.lines().map(str::trim_end).collect();
    let total_index = lines
        .iter()
        .position(|line| *line == "ARTPROC_TOTAL")
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::Internal,
                format!("提权读取的应答形状不对: {output}"),
            )
        })?;
    // GONE/MISSING 都只看 TOTAL 后面那一个标记：正文里万一出现同名字符串也不会误判
    match lines.get(total_index + 1).copied() {
        Some("ARTPROC_GONE") => {
            return Err(AgentError::new(
                ErrorCode::NotFound,
                "进程已经不在了（/proc 下没有这个 pid），请刷新后重试",
            )
            .with_details(serde_json::json!({ "reason": "proc_gone" })));
        }
        Some("ARTPROC_MISSING") => {
            return Err(AgentError::new(
                ErrorCode::NotFound,
                "进程在，但 /proc 下没有这个文件（可能是该进程类型没有这项内容）",
            )
            .with_details(serde_json::json!({ "reason": "proc_file_missing" })));
        }
        _ => {}
    }
    let raw_total = lines.get(total_index + 1).copied().unwrap_or("").trim();
    let total_lines: i64 = raw_total.parse().map_err(|error| {
        AgentError::new(
            ErrorCode::Internal,
            format!("行数无法解析 {raw_total:?}: {error}"),
        )
    })?;
    let body_index = lines
        .iter()
        .position(|line| *line == "ARTPROC_BODY")
        .ok_or_else(|| AgentError::new(ErrorCode::Internal, "提权读取没有正文标记".to_owned()))?;
    let end_index = lines
        .iter()
        .rposition(|line| *line == "ARTPROC_END")
        .ok_or_else(|| {
            AgentError::new(
                ErrorCode::Internal,
                "提权读取没有结束标记（可能被截断）".to_owned(),
            )
        })?;
    if end_index < body_index {
        return Err(AgentError::new(
            ErrorCode::Internal,
            "提权读取的标记顺序不对".to_owned(),
        ));
    }
    // 脚本在正文后面多打了一个空行（`cmdline` 结尾没有换行，不补就会把 `ARTPROC_END`
    // 顶到正文同一行，见 proc_read_script）；这里吃掉它，但不改 total 的判断。
    let raw_body = lines[body_index + 1..end_index].join("\n");
    let body = raw_body.trim_end_matches('\n').to_owned();
    Ok(ProcReadOutput {
        // awk 数不出来（读不到）时脚本给的是 -1：当成 0 行，但正文也一定是空的
        total_lines: u64::try_from(total_lines).unwrap_or(0),
        body_truncated: body.is_empty() && raw_body.len() != body.len(),
        body,
    })
}

fn effective_uid() -> Option<u32> {
    let uid = std::fs::read_to_string(format!("{PROC}/self/status")).ok();
    uid.as_deref().and_then(parse_status_uid)
}

/// 设备侧审计行：只写身份与结论，不含令牌、路径正文或大块数据。
fn audit(params: &ProcessKillParams, outcome: &str, uid: Option<u32>, comm: &str, root: bool) {
    eprintln!(
        "audit method={PROCESS_KILL} pid={} signal={} expected_comm={:?} target_uid={:?} comm={:?} outcome={outcome} ran_as_root={root}",
        params.pid,
        params.signal.number(),
        params.expected_comm.as_deref().unwrap_or("-"),
        uid,
        if comm.is_empty() { "-" } else { comm },
    );
}

#[derive(Debug, Default)]
struct OwnerScan {
    owners: HashMap<u64, u32>,
    unreadable: usize,
    truncated: bool,
}

async fn load_snapshot() -> NetSnapshot {
    let mut snapshot = NetSnapshot::default();
    for (path, family) in NET_FILES {
        let text = match tokio::fs::read_to_string(path).await {
            Ok(text) => text,
            Err(error) => {
                snapshot
                    .unreadable
                    .push(format!("{path}: {}", error.kind()));
                continue;
            }
        };
        for line in text.lines().skip(1) {
            if snapshot.entries.len() >= MAX_SOCKET_ROWS {
                snapshot.truncated = true;
                break;
            }
            if let Some(entry) = parse_net_line(line, family) {
                snapshot.entries.push(entry);
            }
        }
    }
    snapshot
}

/// 解析 `/proc/net/tcp{,6}` 一行：
/// `sl local_address rem_address st tx:rx tr:tm->when retrnsmt uid timeout inode`
fn parse_net_line(line: &str, family: SocketFamily) -> Option<NetEntry> {
    let mut columns = line.split_whitespace();
    columns.next()?; // sl
    let local = columns.next()?;
    let remote = columns.next()?;
    let state = u8::from_str_radix(columns.next()?, 16).ok()?;
    columns.next()?; // tx:rx
    columns.next()?; // tr:tm->when
    columns.next()?; // retrnsmt
    let uid: u32 = columns.next()?.parse().ok()?;
    columns.next()?; // timeout
    let inode: u64 = columns.next()?.parse().ok()?;
    let (local_address, local_port) = parse_endpoint(local, family)?;
    let remote_port = remote
        .rsplit_once(':')
        .and_then(|(_, port)| u16::from_str_radix(port, 16).ok())
        .unwrap_or(0);
    Some(NetEntry {
        inode,
        local_port,
        local_address,
        remote_port,
        state: state_name(state).to_owned(),
        uid,
        family,
    })
}

fn parse_endpoint(value: &str, family: SocketFamily) -> Option<(String, u16)> {
    let (raw, port) = value.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    match family {
        SocketFamily::Ipv4 => {
            let words = u32::from_str_radix(raw, 16).ok()?;
            let bytes = words.to_be_bytes();
            Some((
                format!("{}.{}.{}.{}", bytes[3], bytes[2], bytes[1], bytes[0]),
                port,
            ))
        }
        SocketFamily::Ipv6 => Some((format_ipv6(raw)?, port)),
    }
}

/// IPv6 地址在 /proc/net/tcp6 里是 4 个 32 位小端字，需逐字翻转字节序。
/// 输出用 `Ipv6Addr::to_string()` 的标准压缩记法（`::1`、`::`），与 Desktop
/// Legacy 解析器 `adb::hex_ipv6` 完全一致，shadow 对照才不会把记法差异当成结果差异。
fn format_ipv6(raw: &str) -> Option<String> {
    if raw.len() != 32 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (group_index, group) in raw.as_bytes().chunks(8).enumerate() {
        let value = u32::from_str_radix(std::str::from_utf8(group).ok()?, 16).ok()?;
        bytes[group_index * 4..group_index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    Some(std::net::Ipv6Addr::from(bytes).to_string())
}

fn state_name(value: u8) -> &'static str {
    match value {
        0x01 => "established",
        0x02 => "syn_sent",
        0x03 => "syn_recv",
        0x04 => "fin_wait1",
        0x05 => "fin_wait2",
        0x06 => "time_wait",
        0x07 => "close_wait",
        0x08 => "last_ack",
        0x09 => "closing",
        0x0A => "listen",
        0x0B => "closing",
        0x0C => "closed",
        // 未收录的 st 值兜底成 unknown，绝不复用 listen/established 误导上层
        _ => "unknown",
    }
}

async fn socket_inodes_of(pid: u32) -> Result<HashSet<u64>, String> {
    read_socket_inodes(&format!("{PROC}/{pid}/fd")).await
}

async fn read_socket_inodes(dir: &str) -> Result<HashSet<u64>, String> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|error| format!("{}", error.kind()))?;
    let mut inodes = HashSet::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Some(inode) = socket_inode_of_link(&entry.path()) {
            inodes.insert(inode);
        }
    }
    Ok(inodes)
}

/// 直接 readlink fd，不再解析 `ls -l` 文本。
fn socket_inode_of_link(path: &std::path::Path) -> Option<u64> {
    let target = std::fs::read_link(path).ok()?;
    let text = target.to_string_lossy();
    text.strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse::<u64>()
        .ok()
}

/// 扫描 `/proc/<pid>/fd` 建 inode→pid 索引；非 root 下大量进程不可读，如实计数。
async fn scan_socket_owners(wanted: &HashSet<u64>) -> OwnerScan {
    let mut scan = OwnerScan::default();
    let mut dir = match tokio::fs::read_dir(PROC).await {
        Ok(dir) => dir,
        Err(error) => {
            scan.unreadable += 1;
            let _ = error;
            return scan;
        }
    };
    let mut scanned = 0_usize;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == 0 {
            continue;
        }
        if scanned >= MAX_SCANNED_PIDS {
            scan.truncated = true;
            break;
        }
        scanned += 1;
        match socket_inodes_of(pid).await {
            Ok(inodes) => {
                for inode in inodes {
                    if wanted.contains(&inode) {
                        scan.owners.entry(inode).or_insert(pid);
                    }
                }
                if scan.owners.len() >= wanted.len() {
                    break;
                }
            }
            Err(_) => scan.unreadable += 1,
        }
    }
    scan
}

async fn read_trimmed(path: &str) -> Option<String> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

async fn read_cmdline(path: &str) -> Option<String> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let text = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AgentError> {
    serde_json::from_value(params).map_err(|error| {
        AgentError::new(ErrorCode::InvalidRequest, "invalid process parameters")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|error| {
        AgentError::new(
            ErrorCode::Internal,
            format!("failed to serialize process result: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    #[cfg(test)]
    #[test]
    fn proc_path_is_built_from_validated_parts_only() {
        // 这条方法会走提权脚本，所以"能读哪个文件"必须由代码决定，不能由参数决定。
        assert_eq!(proc_path(4321, ProcFile::Maps).unwrap(), "/proc/4321/maps");
        assert_eq!(proc_path(1, ProcFile::Cmdline).unwrap(), "/proc/1/cmdline");
        // pid=0 在 /proc 里是 swapper，没有 maps；放到脚本里就是一句让人找不着北的失败
        assert!(proc_path(0, ProcFile::Maps).is_err());
    }

    #[test]
    fn proc_read_script_shape_has_the_markers_the_parser_expects() {
        let script = privileged::proc_read_script(4321, "maps", 400);
        // 数字与白名单值拼出来的路径，脚本里不存在任何外部字符串
        assert!(script.contains("/proc/4321/maps"), "{script}");
        assert!(script.contains("sed -n '1,400p'"), "{script}");
        assert!(script.contains("ARTPROC_TOTAL") && script.contains("ARTPROC_BODY"));
        assert!(script.contains("ARTPROC_GONE") && script.contains("ARTPROC_MISSING"));
        // 早退分支也必须以哨兵收尾，否则 run_privileged 会把"进程不在了"报成 Internal
        assert_eq!(
            script.matches(privileged::PROC_READ_SENTINEL).count(),
            3,
            "{script}"
        );
        assert!(script.contains(privileged::PROC_READ_SENTINEL));
        // 行数统计用 awk 而不是 wc -l：cmdline 结尾没有换行，wc -l 会数成 0
        assert!(
            script.contains("END{{print NR}}") || script.contains("END{print NR}"),
            "{script}"
        );
        assert!(
            !script.contains("wc -l"),
            "cmdline 用 wc -l 会得到 0 行: {script}"
        );
    }

    #[tokio::test]
    async fn shell_read_separates_unreadable_empty_and_readable() {
        // ① "能打开、读不出"：拿目录模拟（open 成功、read 直接 EISDIR）。
        //    这类必须判成 Unreadable，否则界面会把"我们没权限"说成"这文件是空的"。
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_string_lossy().to_string();
        assert!(matches!(
            read_proc_head(&dir_path, ProcFile::Maps, 10).await,
            ShellRead::Unreadable
        ));
        // ② 真·空文件：Maps 依然要交给提权去确认（活的进程不可能 0 条映射），
        //    其他文件（如内核线程的 cmdline）就照实返回空。
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        let as_str = empty.to_string_lossy().to_string();
        assert!(matches!(
            read_proc_head(&as_str, ProcFile::Maps, 10).await,
            ShellRead::Empty(_)
        ));
        assert!(matches!(
            read_proc_head(&as_str, ProcFile::Cmdline, 10).await,
            ShellRead::Empty(_)
        ));
        // ③ 有内容才叫 Readable
        let full = dir.path().join("full");
        std::fs::write(&full, b"12c00000 rw-p x\n32c00000 rw-p y\n").unwrap();
        let read = read_proc_head(&full.to_string_lossy(), ProcFile::Maps, 10).await;
        match read {
            ShellRead::Readable(value) => {
                assert_eq!(value.total_lines, 2);
                assert!(!value.truncated);
            }
            other => panic!("应当读到内容: {other:?}"),
        }
    }

    #[test]
    fn proc_read_output_distinguishes_gone_missing_and_ok() {
        let ok = parse_proc_read_output(
            "ARTPROC_TOTAL\n2566\nARTPROC_BODY\nline1\nline2\n\nARTPROC_END",
        )
        .unwrap();
        assert_eq!(ok.total_lines, 2566);
        // 脚本为了不把 ARTPROC_END 顶到正文同一行会多打一个空行，解析时要吃掉
        assert_eq!(ok.body, "line1\nline2");

        // 与 proc_read_script 的真实输出一致：早退分支也要先打 TOTAL、并以哨兵收尾
        let gone = parse_proc_read_output("ARTPROC_TOTAL\nARTPROC_GONE\nARTPROC_END");
        assert_eq!(
            gone.err().map(|error| error.code),
            Some(ErrorCode::NotFound),
            "进程已退要说成 NotFound，不能给一个空正文"
        );
        let missing = parse_proc_read_output("ARTPROC_TOTAL\nARTPROC_MISSING\nARTPROC_END");
        assert_eq!(
            missing.err().map(|error| error.code),
            Some(ErrorCode::NotFound)
        );
        // 应答形状不对时必须报错：把它当"没有映射"就是又一次用空值伪装成功
        let junk = parse_proc_read_output("something else");
        assert!(junk.is_err(), "{junk:?}");
    }

    #[test]
    fn cmdline_nuls_become_spaces_and_line_cap_marks_truncation() {
        // cmdline 在 /proc 里是 NUL 分隔的一行；原样带出去界面就是一串方块
        let raw = b"com.google.android.apps.nexuslauncher\0\0\0";
        assert_eq!(
            decode_proc_bytes(ProcFile::Cmdline, raw),
            "com.google.android.apps.nexuslauncher"
        );
        let (total, text, truncated) = take_lines("a\nb\nc", 2);
        assert_eq!((total, text.as_str(), truncated), (3, "a\nb", true));
        assert!(!take_lines("a\nb", 5).2);
    }

    use super::*;

    const TCP4_SAMPLE: &str = concat!(
        "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n",
        "   0: 0100007F:2CEC 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 62728 1 0000000000000000 100 0 0 10 0\n",
        "   1: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1046        0 51406 1 0000000000000000 100 0 0 10 0\n",
        "   2: B165B40A:9F2C 0100007F:1F94 01 00000000:00000000 00:00000000 00000000 10107        0 98765 1 0000000000000000 100 0 0 10 0\n",
    );

    #[test]
    fn parses_ipv4_rows_with_state_and_owner() {
        let rows: Vec<NetEntry> = TCP4_SAMPLE
            .lines()
            .skip(1)
            .filter_map(|line| parse_net_line(line, SocketFamily::Ipv4))
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].local_address, "127.0.0.1");
        assert_eq!(rows[0].local_port, 11500);
        assert_eq!(rows[0].state, "listen");
        assert_eq!(rows[0].inode, 62728);
        assert_eq!(rows[1].local_port, 8080);
        assert_eq!(rows[1].uid, 1046);
        assert_eq!(rows[2].state, "established");
        // 远端端口非 0 的行由调用方按 remote_port 过滤，这里保留原值
        assert_eq!(rows[2].remote_port, 8084);
    }

    #[test]
    fn decodes_ipv6_loopback_and_rejects_short_address() {
        let row = "   0: 00000000000000000000000001000000:1F91 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 4242 1 0000000000000000 100 0 0 10 0";
        let entry = parse_net_line(row, SocketFamily::Ipv6).expect("should parse");
        assert_eq!(entry.local_port, 8081);
        // 四段 32 位字按小端还原后就是 ::1，且必须用与 Legacy 相同的压缩记法
        assert_eq!(entry.local_address, "::1");
        assert_eq!(entry.state, "listen");
        assert!(format_ipv6("0000").is_none());
    }

    #[test]
    fn unknown_states_never_masquerade_as_listen() {
        assert_eq!(state_name(0x0A), "listen");
        assert_eq!(state_name(0x01), "established");
        assert_eq!(state_name(0xEE), "unknown");
    }

    #[test]
    fn malformed_rows_are_skipped() {
        assert!(parse_net_line("   0: 0100007F", SocketFamily::Ipv4).is_none());
        assert!(parse_net_line("", SocketFamily::Ipv4).is_none());
    }

    /// 只有 Linux（含设备）有 `/proc`；macOS 宿主上跳过，不拿"读不到"当"没端口"。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn scans_own_fd_inodes_without_shell() {
        let pid = std::process::id();
        // 测试进程没有 socket fd：应为空集合而不是错误，证明「没端口」与「读不到」可区分
        let inodes = socket_inodes_of(pid)
            .await
            .expect("own fd dir must be readable");
        assert!(inodes.is_empty());
    }

    fn kill_params(expected: Option<&str>, require_root: bool) -> ProcessKillParams {
        ProcessKillParams {
            pid: 4321,
            expected_comm: expected.map(str::to_string),
            signal: KillSignal::Kill,
            require_root,
        }
    }

    fn identity(comm: &str, cmdline: &str) -> ProcessIdentity {
        ProcessIdentity {
            comm: Some(comm.to_string()),
            cmdline: Some(cmdline.to_string()),
            uid: Some(2000),
        }
    }

    /// PID 复用防护：身份对不上时必须在发信号之前就拒止，且错误里带上两侧证据。
    #[test]
    fn identity_guard_refuses_mismatch_before_signalling() {
        let error = guard_identity(
            4321,
            &kill_params(Some("my-hosted-tool"), false),
            Some(&identity("mediaserver", "/system/bin/mediaserver")),
        )
        .expect_err("身份不匹配必须拒止");
        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        let details = error.details.expect("必须带回身份证据");
        assert_eq!(details["reason"], "identity_mismatch");
        assert_eq!(details["expected"], "my-hosted-tool");
        assert_eq!(details["actual_comm"], "mediaserver");
    }

    /// comm 会被内核截断到 15 字符，所以 cmdline 的程序名也要能匹配上。
    #[test]
    fn identity_guard_accepts_comm_or_program_match() {
        guard_identity(
            4321,
            &kill_params(Some("toybox"), false),
            Some(&identity("toybox", "/system/bin/toybox nc -L -p 24567")),
        )
        .expect("comm 命中应放行");
        guard_identity(
            4321,
            &kill_params(Some("long-hosted-binary-name"), false),
            Some(&identity(
                "long-hosted-bina",
                "/data/local/tmp/long-hosted-binary-name --serve",
            )),
        )
        .expect("comm 被截断时应按 cmdline 程序名放行");
        guard_identity(4321, &kill_params(None, false), None).expect("不给预期名时不阻断幂等分支");
    }

    #[test]
    fn signals_use_fixed_numbers_and_pid_zero_never_reaches_kill() {
        assert_eq!(KillSignal::Term.number(), 15);
        assert_eq!(KillSignal::Kill.number(), 9);
        // pid=0 在 kill(2) 里是「发给整个进程组」，绝不能透传下去
        assert!(matches!(
            send_signal(0, KillSignal::Term),
            SignalResult::Failed(_)
        ));
    }

    #[test]
    fn status_uid_reads_the_real_uid_column() {
        let status = "Name:\ttask\nState:\tS (sleeping)\nTgid:\t4321\nUid:\t2000\t2000\t2000\t2000\nGid:\t2000\t2000\t2000\t2000\n";
        assert_eq!(parse_status_uid(status), Some(2000));
        assert_eq!(parse_status_uid("Uid:\t\t"), None);
    }

    #[tokio::test]
    async fn missing_pid_reports_unreadable_instead_of_empty() {
        let missing = std::process::id() + 900_000;
        let error = socket_inodes_of(missing).await.unwrap_err();
        assert!(!error.is_empty(), "读不到必须给出原因: {error}");
    }
}

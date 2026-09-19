//! ProcessesProvider（AR6.2）：端口/进程互查在设备端一次快照完成。
//!
//! 迁移前 Desktop 需要 `cat /proc/net/tcp{,6}` 拉全文 + 宿主侧 hex 解析，端口反查进程
//! 还要分批 `ls -l /proc/<pid>/fd`（一次查询几十条 shell）。这里改为：直接读
//! `/proc/net/*` 与 `/proc/<pid>/fd` 符号链接，在 Agent 内完成 inode→pid 归属匹配。
//! 读不到的项一律如实上报（`unreadable` / `skipped` / `truncated`），
//! 不把「没权限看」说成「没有端口」。

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use agent_protocol::method::{PROCESS_BY_PORT, PROCESS_KILL, PROCESS_PORTS};
use agent_protocol::{
    AgentError, ErrorCode, KillOutcome, KillSignal, ListeningPort, PortHoldingProcess,
    ProcessByPortParams, ProcessByPortResult, ProcessKillParams, ProcessKillResult,
    ProcessPortsParams, ProcessPortsResult, ProviderHealth, ProviderInfo, SocketFamily,
};
use serde_json::Value;

use super::{Provider, ProviderFuture, RequestContext};

const PROCESS_METHODS: &[&str] = &[PROCESS_PORTS, PROCESS_BY_PORT, PROCESS_KILL];
/// 终止后的确认轮询：有界，不做无界等待（卡住的写操作比慢的更危险）。
const KILL_VERIFY_TIMEOUT: Duration = Duration::from_millis(1_500);
const KILL_VERIFY_POLL: Duration = Duration::from_millis(50);
const NET_FILES: [(&str, SocketFamily); 2] = [
    ("/proc/net/tcp", SocketFamily::Ipv4),
    ("/proc/net/tcp6", SocketFamily::Ipv6),
];
const PROC: &str = "/proc";
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

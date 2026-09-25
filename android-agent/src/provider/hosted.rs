//! HostedProvider（AR7.2）：托管二进制的列目录 / 赋权 / 启动 / 状态由 Agent 管理。
//!
//! 迁移前 Desktop 用三串 shell 文本拼出这件事：`ls -l` 判权限、`file` 判 ELF、
//! `nohup ./x >log 2>&1 & echo $!` 取 PID，运行状态再靠文件名反查进程。于是 PID 只是
//! 一个没有身份的数、Desktop 或 Agent 一重启就对不上号、文件名与参数都要进 shell 解析上下文。
//! 本 Provider 改成：
//! - ELF 用文件头 magic 判定（不依赖设备端有没有 `file` 命令）；
//! - 启动即生成稳定 `handle`，身份是 `pid + /proc/<pid>/stat` 的 start time ticks：
//!   PID 被复用时两者必然不一致，对账只会认不出来（标 `exited`/`pid_reused`），不会认错进程；
//! - 记录落盘（状态目录 0700、记录文件 0600），Agent 重启后按同一规则对账；
//! - 自己启动的子进程由本 Provider 回收，能给出真实退出码；重启后对账出来的记录
//!   不再是自己的子进程，只报存活状态，绝不假装知道退出码；
//! - 参数数组直接 exec，文件名与参数都不进任何 shell 解析上下文。

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent_protocol::method::{HOSTED_CHMOD, HOSTED_LIST, HOSTED_START, HOSTED_STATUS, HOSTED_STOP};
use agent_protocol::{
    AgentError, ErrorCode, ExternalProc, FileKind, HostedBinaryInfo, HostedChmodParams,
    HostedChmodResult, HostedListParams, HostedListResult, HostedRunRecord, HostedRunState,
    HostedStartParams, HostedStartResult, HostedStatusParams, HostedStatusResult, HostedStopParams,
    HostedStopResult, KillOutcome, KillSignal, PERMISSION_BITS, ProviderHealth, ProviderInfo,
    render_mode_text,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::process::{SignalResult, send_signal};
use super::{Provider, ProviderFuture, RequestContext};

const HOSTED_METHODS: &[&str] = &[
    HOSTED_LIST,
    HOSTED_CHMOD,
    HOSTED_START,
    HOSTED_STATUS,
    HOSTED_STOP,
];
/// 托管目录固定路径（与 Desktop 的 `adb::HOSTED_DIR` 一致，用户指定的默认目录）。
const HOSTED_DIR: &str = "/data/local/tmp";
/// 运行记录落盘目录：Agent 重启后靠它对账，不靠内存表。
const STATE_DIR: &str = "/data/local/tmp/app-reverse-tools-hosted";
const MAX_BINARIES: usize = 2_000;
/// 记录文件保留上限（按 mtime 留最近这些），防止长期堆积。
const MAX_RUN_RECORDS: usize = 200;
/// 发信号后确认进程消失的有界等待：超时只说「未确认」，不谎报已停止。
const STOP_CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1_500);
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

pub struct HostedProvider {
    state: Mutex<Table>,
}

#[derive(Default)]
struct Table {
    /// handle -> 运行记录；`child` 仅在 Agent 亲自启动时存在（才能回收退出码）
    runs: HashMap<String, ManagedRun>,
    /// 是否已从磁盘对账过（首次调用时懒加载，构造期不做 IO）
    loaded: bool,
}

struct ManagedRun {
    record: HostedRunRecord,
    child: Option<tokio::process::Child>,
    source: RunSource,
}

/// 记录来源：决定能不能给出退出码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunSource {
    /// 本 Agent 亲自启动，持有子进程句柄
    Spawned,
    /// 从磁盘对账恢复（已不是自己的子进程）
    Reconciled,
}

/// 落盘格式：记录本体 + 来源标记。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRun {
    record: HostedRunRecord,
}

#[derive(Debug, Default)]
struct BinaryScan {
    binaries: Vec<HostedBinaryInfo>,
    truncated: bool,
    unreadable: Vec<String>,
}

impl HostedProvider {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(Table::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Table> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Default for HostedProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for HostedProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "hosted".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: vec!["shell".into()],
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        HOSTED_METHODS
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                HOSTED_LIST => self.list(params).await,
                HOSTED_CHMOD => self.chmod(params),
                HOSTED_START => self.start(params),
                HOSTED_STATUS => self.status(params),
                HOSTED_STOP => self.stop(params),
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported hosted method: {method}"),
                )),
            }
        })
    }
}

impl HostedProvider {
    /// 托管目录里的 ELF 可执行文件 + 当前运行表。
    async fn list(&self, params: Value) -> Result<Value, AgentError> {
        let _params: HostedListParams = parse_params(params)?;
        self.ensure_loaded()?;
        // 上千个文件的 open+读头不能占住 async 上下文；扫描期间不持锁
        let scan = tokio::task::spawn_blocking(scan_binaries)
            .await
            .map_err(|error| {
                AgentError::new(
                    ErrorCode::Internal,
                    format!("托管目录扫描任务失败: {error}"),
                )
            })?;
        let mut table = self.lock();
        let runs = refresh(&mut table);
        let runs: Vec<HostedRunRecord> = runs.into_iter().map(|(_, record)| record).collect();
        let ours: std::collections::HashSet<u32> = runs
            .iter()
            .filter(|r| r.state == HostedRunState::Running)
            .map(|r| r.pid)
            .collect();
        // 设备上有同名进程在跑、但不是我们启动的：工具比目标进程晚启动时必然出现这种
        // "它在跑，可我这边显示未运行"。把 pid 一起带回去，界面才说得出"其实已经在跑"。
        let external = running_processes_by_comm();
        let mut binaries = scan.binaries;
        for info in &mut binaries {
            info.external_procs = external_procs_for(&info.name, &external, &ours);
        }
        serialize(HostedListResult {
            dir: HOSTED_DIR.to_owned(),
            binaries,
            runs,
            truncated: scan.truncated,
            unreadable: scan.unreadable,
        })
    }

    /// 赋可执行位：`mode | 0o111`。保留原有读位与其它位，不粗暴设 0755。
    fn chmod(&self, params: Value) -> Result<Value, AgentError> {
        let params: HostedChmodParams = parse_params(params)?;
        let path = hosted_path(&params.name)?;
        let metadata = std::fs::metadata(&path).map_err(|error| io_error("stat", &path, error))?;
        let mode = metadata.mode() & PERMISSION_BITS;
        if mode & 0o100 != 0 {
            // 已可执行：幂等返回当前状态，不再写一次权限（免得无意义改 ctime）
            return serialize(HostedChmodResult {
                name: params.name.clone(),
                path: display(&path),
                mode,
                mode_text: render_mode_text(FileKind::File, mode),
                has_exec: true,
            });
        }
        let next = mode | 0o111;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(next))
            .map_err(|error| io_error("chmod", &path, error))?;
        let actual = std::fs::metadata(&path)
            .map(|metadata| metadata.mode() & PERMISSION_BITS)
            .unwrap_or(next);
        eprintln!(
            "audit method={HOSTED_CHMOD} name={} path={} mode={:o} prev={:o}",
            params.name,
            display(&path),
            actual,
            mode
        );
        serialize(HostedChmodResult {
            name: params.name.clone(),
            path: display(&path),
            mode: actual,
            mode_text: render_mode_text(FileKind::File, actual),
            has_exec: actual & 0o100 != 0,
        })
    }

    /// 启动托管进程：参数数组直接 exec，不进 shell；记录身份并落盘。
    fn start(&self, params: Value) -> Result<Value, AgentError> {
        let params: HostedStartParams = parse_params(params)?;
        self.ensure_loaded()?;
        if params.root {
            // 与 AR6.3 同一条规则：不「试一下看运气」，也不冒充满足 root 要求
            return Err(AgentError::new(
                ErrorCode::PermissionDenied,
                "以 root 启动需要 root 通道，Agent 当前以 shell 身份运行",
            )
            .with_details(serde_json::json!({
                "reason": "root_required",
                "agent_uid": effective_uid(),
            })));
        }
        let path = hosted_path(&params.name)?;
        let metadata = std::fs::metadata(&path).map_err(|error| io_error("stat", &path, error))?;
        if !metadata.is_file() {
            return Err(invalid(
                "not_a_regular_file",
                format!("托管目标不是普通文件: {}", display(&path)),
            ));
        }
        if !is_elf(&path).unwrap_or(false) {
            return Err(invalid(
                "not_an_elf",
                format!("托管目标不是 ELF 可执行文件: {}", display(&path)),
            ));
        }
        if metadata.mode() & 0o100 == 0 {
            return Err(invalid(
                "not_executable",
                format!("缺少 owner 执行位，请先调 hosted.chmod: {}", display(&path)),
            ));
        }
        let log_path = format!("{HOSTED_DIR}/.{}.run.log", params.name);
        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|error| io_error("open_log", Path::new(&log_path), error))?;
        // 日志可能已存在且是别人建的（umask 各版本不同）：能收就收，收不动也不阻断启动
        let _ = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600));
        let stderr = stdout.try_clone().map_err(|error| {
            AgentError::new(ErrorCode::Internal, format!("dup 日志句柄失败: {error}"))
        })?;
        let mut command = tokio::process::Command::new(&path);
        command
            .current_dir(HOSTED_DIR)
            .args(&params.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(stdout))
            .stderr(std::process::Stdio::from(stderr))
            // 独立进程组：Agent 停止时不把托管进程一起带走（对齐原 nohup 语义）
            .process_group(0)
            .kill_on_drop(false);
        let child = command.spawn().map_err(|error| {
            AgentError::new(
                ErrorCode::Internal,
                format!("启动 {} 失败: {error}", params.name),
            )
            .with_details(serde_json::json!({
                "reason": "spawn_failed",
                "errno": error.raw_os_error(),
            }))
        })?;
        let pid = child.id().unwrap_or(0);
        let start_time_ticks = read_start_time_ticks(pid).unwrap_or(0);
        let handle = random_handle()?;
        let record = HostedRunRecord {
            handle: handle.clone(),
            name: params.name.clone(),
            pid,
            start_time_ticks,
            started_at_unix: unix_now(),
            log_path: log_path.clone(),
            root: false,
            state: HostedRunState::Running,
            exit_code: None,
            detail: (start_time_ticks == 0).then(|| "start_time_unreadable".to_string()),
        };
        self.persist(&record);
        self.lock().runs.insert(
            handle.clone(),
            ManagedRun {
                record: record.clone(),
                child: Some(child),
                source: RunSource::Spawned,
            },
        );
        eprintln!(
            "audit method={HOSTED_START} handle={handle} name={} pid={} args={:?} log={log_path}",
            params.name, record.pid, params.args
        );
        serialize(HostedStartResult { record })
    }

    /// 单条运行记录：能回收的子进程顺手回收，拿到真实退出码。
    fn status(&self, params: Value) -> Result<Value, AgentError> {
        let params: HostedStatusParams = parse_params(params)?;
        self.ensure_loaded()?;
        let mut table = self.lock();
        let runs = refresh(&mut table);
        let Some((source, record)) = runs
            .into_iter()
            .find(|(_, record)| record.handle == params.handle)
        else {
            return Err(AgentError::new(
                ErrorCode::NotFound,
                format!("没有句柄 {} 的运行记录", params.handle),
            )
            .with_details(serde_json::json!({ "reason": "unknown_handle" })));
        };
        serialize(HostedStatusResult {
            reconciled: source == RunSource::Reconciled,
            record,
        })
    }

    /// 停止托管进程（AR7.3，写操作）。
    ///
    /// 与 `process.kill` 的区别在寻址方式：这里按 `handle` 找记录，发信号前用落盘的
    /// start time 复核身份。PID 复用窗口里（老进程已退、数字被别人拿走）判
    /// `already_gone` + `pid_reused_*`，**绝不向新租户补一刀**；既不是自己持有的
    /// 子进程、又核不出身份时直接拒止，而不是「先杀了看看」。自己启动且未回收的
    /// 子进程可以放行：它还是僵尸时 PID 被内核保留，不存在复用问题。
    /// 确认退出后删除持久化记录——状态目录只服务「重启后可能还活着」的对账，
    /// 已确认死亡的不该在下次重启里复活成 running。
    fn stop(&self, params: Value) -> Result<Value, AgentError> {
        let params: HostedStopParams = parse_params(params)?;
        self.ensure_loaded()?;
        let signal = params.signal;
        let mut table = self.lock();
        let Some(run) = table.runs.get_mut(&params.handle) else {
            return Err(AgentError::new(
                ErrorCode::NotFound,
                format!("没有句柄 {} 的运行记录", params.handle),
            )
            .with_details(serde_json::json!({ "reason": "unknown_handle" })));
        };
        let pid = run.record.pid;
        if let Some(expected) = params.expected_pid {
            if expected != pid {
                return Err(AgentError::new(
                    ErrorCode::PreconditionFailed,
                    format!(
                        "句柄 {} 的 PID 与调用方看到的不一致，已拒绝终止",
                        params.handle
                    ),
                )
                .with_details(serde_json::json!({
                    "reason": "pid_mismatch",
                    "expected_pid": expected,
                    "actual_pid": pid,
                })));
            }
        }
        let owned = run.child.is_some();
        let observed = read_start_time_ticks(pid);
        let mut identity_verified = false;
        match observed {
            Some(ticks) if ticks == run.record.start_time_ticks => identity_verified = true,
            Some(ticks) => {
                run.record.state = HostedRunState::Exited;
                run.record.detail = Some(format!("pid_reused_new_start_ticks={ticks}"));
                run.child = None;
                drop_persisted(&run.record.handle);
                audit_stop(
                    &params.handle,
                    pid,
                    "already_gone_pid_reused",
                    signal,
                    false,
                );
                return serialize(HostedStopResult {
                    record: run.record.clone(),
                    outcome: KillOutcome::AlreadyGone,
                    identity_verified: false,
                    record_dropped: true,
                });
            }
            None if !owned && !std::path::Path::new(&format!("/proc/{pid}")).exists() => {
                run.record.state = HostedRunState::Exited;
                run.record.detail = Some("process_gone".to_string());
                drop_persisted(&run.record.handle);
                audit_stop(&params.handle, pid, "already_gone", signal, false);
                return serialize(HostedStopResult {
                    record: run.record.clone(),
                    outcome: KillOutcome::AlreadyGone,
                    identity_verified: false,
                    record_dropped: true,
                });
            }
            None if !owned => {
                return Err(AgentError::new(
                    ErrorCode::PreconditionFailed,
                    format!("无法核对 pid={pid} 的身份，已拒绝终止"),
                )
                .with_details(serde_json::json!({
                    "reason": "identity_unverifiable",
                    "hint": "该进程可能属于其它 uid；root 支路请走 Legacy su -c kill",
                })));
            }
            None => {}
        }
        match send_signal(pid, signal) {
            SignalResult::Gone => {
                run.record.state = HostedRunState::Exited;
                run.record.detail = Some("process_gone".to_string());
                run.child = None;
                drop_persisted(&run.record.handle);
                audit_stop(
                    &params.handle,
                    pid,
                    "already_gone",
                    signal,
                    identity_verified,
                );
                serialize(HostedStopResult {
                    record: run.record.clone(),
                    outcome: KillOutcome::AlreadyGone,
                    identity_verified,
                    record_dropped: true,
                })
            }
            SignalResult::Denied => {
                audit_stop(
                    &params.handle,
                    pid,
                    "permission_denied",
                    signal,
                    identity_verified,
                );
                Err(
                    AgentError::new(ErrorCode::PermissionDenied, format!("无权限终止 pid={pid}"))
                        .with_details(serde_json::json!({
                            "reason": "not_owner",
                            "handle": params.handle,
                            "agent_uid": effective_uid(),
                        })),
                )
            }
            SignalResult::Failed(errno) => {
                audit_stop(
                    &params.handle,
                    pid,
                    &format!("errno={errno}"),
                    signal,
                    identity_verified,
                );
                Err(AgentError::new(
                    ErrorCode::Internal,
                    format!("kill 失败 pid={pid} errno={errno}"),
                ))
            }
            SignalResult::Sent => {
                audit_stop(&params.handle, pid, "signaled", signal, identity_verified);
                let gone = poll_dead(pid);
                if gone {
                    reap(run);
                    run.record.state = HostedRunState::Exited;
                    if run.record.detail.is_none() {
                        run.record.detail = Some("stopped".to_string());
                    }
                    drop_persisted(&run.record.handle);
                } else {
                    // SIGTERM 命中忙进程可能几秒后才退：没确认到只说「未确认」
                    run.record.detail = Some("signal_sent_not_confirmed".to_string());
                }
                serialize(HostedStopResult {
                    record: run.record.clone(),
                    outcome: KillOutcome::Signaled,
                    identity_verified,
                    record_dropped: gone,
                })
            }
        }
    }

    /// 首次调用时从磁盘对账（构造期不做 IO）。
    fn ensure_loaded(&self) -> Result<(), AgentError> {
        let mut table = self.lock();
        if table.loaded {
            return Ok(());
        }
        let mut runs = HashMap::new();
        match std::fs::read_dir(STATE_DIR) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|value| value.to_str()) != Some("json") {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let Ok(stored) = serde_json::from_str::<StoredRun>(&text) else {
                        // 解析不了的文件留着只会每次重复报错；先留日志再删
                        eprintln!("hosted: 忽略无法解析的运行记录 {}", path.display());
                        let _ = std::fs::remove_file(&path);
                        continue;
                    };
                    let mut record = stored.record;
                    let (state, detail) = reconcile(record.start_time_ticks, record.pid);
                    record.state = state;
                    // 已不是自己的子进程，没有退出码可 claim
                    record.exit_code = None;
                    if let Some(detail) = detail {
                        record.detail = Some(detail);
                    }
                    runs.insert(
                        record.handle.clone(),
                        ManagedRun {
                            record,
                            child: None,
                            source: RunSource::Reconciled,
                        },
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("read_state_dir", Path::new(STATE_DIR), error)),
        }
        table.runs = runs;
        table.loaded = true;
        Ok(())
    }

    fn persist(&self, record: &HostedRunRecord) {
        if let Err(error) = create_state_dir() {
            eprintln!("hosted: 状态目录创建失败，运行记录不落盘: {error}");
            return;
        }
        let path = PathBuf::from(STATE_DIR).join(format!("{}.json", record.handle));
        let Ok(text) = serde_json::to_vec_pretty(&StoredRun {
            record: record.clone(),
        }) else {
            return;
        };
        match std::fs::write(&path, text) {
            Ok(()) => {
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                prune_state_dir();
            }
            Err(error) => eprintln!("hosted: 运行记录写入失败 {}: {error}", path.display()),
        }
    }
}

/// 刷新所有记录状态并回收已退出的子进程（不跨 await，纯同步）。
fn refresh(table: &mut Table) -> Vec<(RunSource, HostedRunRecord)> {
    for run in table.runs.values_mut() {
        if let Some(child) = run.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    run.record.state = HostedRunState::Exited;
                    run.record.exit_code = status.code();
                    run.record.detail = Some(match status.signal() {
                        Some(signal) => format!("exited_signal_{signal}"),
                        None => "exited".to_string(),
                    });
                    run.child = None;
                }
                Ok(None) => run.record.state = HostedRunState::Running,
                Err(error) => {
                    run.record.state = HostedRunState::Unknown;
                    run.record.detail = Some(format!("wait_failed: {error}"));
                }
            }
            continue;
        }
        let (state, detail) = reconcile(run.record.start_time_ticks, run.record.pid);
        run.record.state = state;
        if let Some(detail) = detail {
            run.record.detail = Some(detail);
        }
    }
    let mut runs: Vec<(RunSource, HostedRunRecord)> = table
        .runs
        .values()
        .map(|run| (run.source, run.record.clone()))
        .collect();
    runs.sort_by_key(|run| run.1.started_at_unix);
    runs
}

/// pid + start time 对账：读到的 ticks 与记录的 ticks 一致才算同一个进程。
fn reconcile(recorded_ticks: u64, pid: u32) -> (HostedRunState, Option<String>) {
    if pid == 0 {
        return (HostedRunState::Unknown, Some("pid_zero".to_string()));
    }
    match read_start_time_ticks(pid) {
        Some(ticks) if ticks == recorded_ticks => (HostedRunState::Running, None),
        Some(ticks) => (
            HostedRunState::Exited,
            // PID 已被复用：老进程不在了，这个数字现在属于别人
            Some(format!("pid_reused_new_start_ticks={ticks}")),
        ),
        None if !Path::new(&format!("/proc/{pid}")).exists() => {
            (HostedRunState::Exited, Some("process_gone".to_string()))
        }
        None => (
            HostedRunState::Unknown,
            Some("start_time_unreadable".to_string()),
        ),
    }
}

fn read_start_time_ticks(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_start_time_ticks(&text)
}

/// `/proc/<pid>/stat` 第 22 字段是 starttime（时钟滴答）。
/// comm 可能含空格与括号，必须从**最后一个** `)` 之后重新数列：`)` 后第一个字段是
/// state（第 3 字段），所以第 22 字段 = 其后第 19 个 token。
fn parse_start_time_ticks(stat: &str) -> Option<u64> {
    let closer = stat.rfind(')')?;
    let tail = &stat[closer + 1..];
    tail.split_whitespace()
        .nth(19)
        .and_then(|value| value.parse::<u64>().ok())
}

/// 文件名白名单 + 目录固定：Agent 端二次校验，不信任 Desktop（§3.7）。
fn hosted_path(name: &str) -> Result<PathBuf, AgentError> {
    let valid = !name.is_empty()
        && !name.starts_with('.')
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !valid {
        return Err(invalid(
            "invalid_name",
            "托管文件名只允许字母数字与 _ . -，且不得以 . 开头",
        ));
    }
    Ok(PathBuf::from(HOSTED_DIR).join(name))
}

/// 读文件头判 ELF：部分 ROM 没有 `file` 命令，magic 判定不依赖外部命令。
fn is_elf(path: &Path) -> Result<bool, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut head = [0_u8; 4];
    match file.read_exact(&mut head) {
        Ok(()) => Ok(head == ELF_MAGIC),
        // 空文件或短文件：不是 ELF，不是错误
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => Err(error),
    }
}

fn create_state_dir() -> std::io::Result<()> {
    match std::fs::metadata(STATE_DIR) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::fs::create_dir(STATE_DIR)?;
    // umask 可能放过 0755，这里显式收到 0700：记录里有 pid 与路径，不给别人读
    std::fs::set_permissions(STATE_DIR, std::fs::Permissions::from_mode(0o700))
}

/// 只保留最近 `MAX_RUN_RECORDS` 个记录文件（按 mtime）。
fn prune_state_dir() {
    let Ok(entries) = std::fs::read_dir(STATE_DIR) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let mtime = entry.metadata().and_then(|meta| meta.modified()).ok()?;
            Some((mtime, path))
        })
        .collect();
    if files.len() <= MAX_RUN_RECORDS {
        return;
    }
    files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, path) in files.iter().skip(MAX_RUN_RECORDS) {
        let _ = std::fs::remove_file(path);
    }
}

/// 扫托管目录：普通文件 + ELF 头 + 权限。整块在 `spawn_blocking` 里跑。
fn scan_binaries() -> BinaryScan {
    let mut scan = BinaryScan::default();
    let entries = match std::fs::read_dir(HOSTED_DIR) {
        Ok(entries) => entries,
        Err(error) => {
            scan.unreadable
                .push(format!("{HOSTED_DIR}: {}", error.kind()));
            return scan;
        }
    };
    for item in entries.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        // 隐藏项是运行日志（`.xxx.run.log`），不进列表
        if name.starts_with('.') {
            continue;
        }
        let path = item.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            scan.unreadable.push(format!("{name}: 元数据读不到"));
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if scan.binaries.len() >= MAX_BINARIES {
            scan.truncated = true;
            break;
        }
        match is_elf(&path) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                scan.unreadable.push(format!("{name}: {error}"));
                continue;
            }
        }
        let mode = metadata.mode() & PERMISSION_BITS;
        scan.binaries.push(HostedBinaryInfo {
            name,
            path: display(&path),
            size: metadata.size(),
            mode,
            mode_text: render_mode_text(FileKind::File, mode),
            has_exec: mode & 0o100 != 0,
            uid: metadata.uid(),
            mtime_unix: metadata.mtime().max(0) as u64,
            // 这里先留空：外部同名进程要扫一遍 /proc，整份清单一次扫完再回填（见 list）
            external_procs: Vec::new(),
        });
    }
    scan.binaries.sort_by(|a, b| a.name.cmp(&b.name));
    scan
}

/// 扫一遍 `/proc/<pid>/comm`，得到"进程名 → 正在跑的 pid"。
///
/// 用 comm 而不是 cmdline：cmdline 属于别的 uid 时 shell 读不到（真机实测），
/// 而 comm 可读。注意内核把 comm 截到 15 字符，所以匹配时按截断后的名字比，
/// 长名字会一起命中——这比"显示成未运行"更接近事实，误命中的代价由界面上
/// "确认再执行"这一步兜住（不会静默替用户做决定）。
fn running_processes_by_comm() -> HashMap<String, Vec<(u32, u32)>> {
    let mut map: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return map;
    };
    let me = std::process::id();
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_string_lossy().parse::<u32>().ok() else {
            continue;
        };
        if pid == me {
            continue;
        }
        // 一次读 stat 就够：comm 与状态都在里面（stat 里同样被截到 15 字符，与
        // /proc/<pid>/comm 一致）。**僵尸必须剔掉**：它既有 pid 也还有 comm，按 comm
        // 匹配就会把"等着被回收的尸体"报成"外部在跑"——那又是拿缺失的证据编一个结论。
        // 真机上就是这个形状：auth-server 活着是 9727(S)，11049 是 Z。
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some((comm, ppid, state)) = parse_comm_ppid_state(&stat) else {
            continue;
        };
        if matches!(state, 'Z' | 'X' | 'x') || comm.is_empty() {
            continue;
        }
        map.entry(comm.to_owned()).or_default().push((pid, ppid));
    }
    map
}

/// 一次拿 (comm, ppid, state)。
///
/// 只能从**最后一个** `)` 之后切：comm 自己可以含括号（内核线程名常是 `(devw` 这类，
/// 应用也会改进程名），从第一个 `)` 切会把状态读成名字中间的一个字符。
/// ppid 同理取最后一个 `)` 之后的第二段。同一口径在本模块 `parse_start_time_ticks`
/// 已经用过，别再各写一份。
fn parse_comm_ppid_state(stat: &str) -> Option<(String, u32, char)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = stat[open + 1..close].to_owned();
    let mut fields = stat[close + 1..].split_whitespace();
    let state = fields.next()?.chars().next()?;
    let ppid = fields.next()?.parse().unwrap_or(0);
    Some((comm, ppid, state))
}

/// 某个托管文件当前有没有"别人启动的同名进程"在跑：按 comm 匹配，并剔掉我们自己
/// 托管表里已在跑的 pid（那些有句柄、界面本来就显示成运行中，不该再报"外部"）。
fn external_procs_for(
    name: &str,
    by_comm: &HashMap<String, Vec<(u32, u32)>>,
    ours: &HashSet<u32>,
) -> Vec<ExternalProc> {
    let mut found: Vec<ExternalProc> = by_comm
        .iter()
        .filter(|(comm, _)| comm_matches(name, comm))
        .flat_map(|(_, pids)| pids.iter().copied())
        .filter(|(pid, _)| !ours.contains(pid))
        .map(|(pid, ppid)| ExternalProc {
            pid,
            ppid,
            // uid 只对少数候选读，别为 /proc 下几百个 pid 都开一次文件
            uid: real_uid(pid).unwrap_or(u32::MAX),
        })
        .collect();
    found.sort_unstable_by_key(|proc| proc.pid);
    found.dedup();
    found
}

/// 真实 uid（`/proc/<pid>/status` 的 `Uid:` 第一段）。读不到返回 `None`——
/// 用 `u32::MAX` 占位是"不知道"，不会像 0 那样被误读成"root 起的"。
fn real_uid(pid: u32) -> Option<u32> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|line| line.starts_with("Uid:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// 托管文件名与 comm 的对应：完全相等，或名字长到被内核截断后相等。
fn comm_matches(name: &str, comm: &str) -> bool {
    name == comm || (name.len() > 15 && name.starts_with(comm) && comm.len() == 15)
}

/// 确认死亡时顺手回收自己持有的子进程，把真实退出码/信号留在记录里。
fn reap(run: &mut ManagedRun) {
    let Some(child) = run.child.as_mut() else {
        return;
    };
    match child.try_wait() {
        Ok(Some(status)) => {
            run.record.exit_code = status.code();
            run.record.detail = Some(match status.signal() {
                Some(signal) => format!("exited_signal_{signal}"),
                None => "exited".to_string(),
            });
        }
        Ok(None) => run.record.detail = Some("still_running".to_string()),
        Err(error) => run.record.detail = Some(format!("wait_failed: {error}")),
    }
    run.child = None;
}

/// 停止成功后删除持久化记录：状态目录只保留「可能还在跑」的进程。
fn drop_persisted(handle: &str) {
    let path = PathBuf::from(STATE_DIR).join(format!("{handle}.json"));
    let _ = std::fs::remove_file(path);
}

/// 有界轮询确认进程消失。僵尸态算已消失（它只是没被回收，不再运行）。
fn poll_dead(pid: u32) -> bool {
    let started = std::time::Instant::now();
    loop {
        let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return true;
        };
        let state = status
            .rsplit_once(')')
            .and_then(|(_, tail)| tail.trim_start().chars().next());
        if state == Some('Z') {
            return true;
        }
        if started.elapsed() >= STOP_CONFIRM_TIMEOUT {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn audit_stop(handle: &str, pid: u32, outcome: &str, signal: KillSignal, verified: bool) {
    eprintln!(
        "audit method={HOSTED_STOP} handle={handle} pid={} signal={} outcome={outcome} identity_verified={verified}",
        pid,
        signal.number()
    );
}

fn random_handle() -> Result<String, AgentError> {
    let mut bytes = [0_u8; 8];
    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|error| io_error("open_urandom", Path::new("/dev/urandom"), error))?;
    file.read_exact(&mut bytes)
        .map_err(|error| io_error("read_urandom", Path::new("/dev/urandom"), error))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or_default()
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid 无前置条件，也不改进程状态。
    unsafe { libc::geteuid() }
}

fn invalid(reason: &str, message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::InvalidRequest, message)
        .with_details(serde_json::json!({ "reason": reason }))
}

fn io_error(op: &str, path: &Path, error: std::io::Error) -> AgentError {
    let (code, reason) = match error.kind() {
        std::io::ErrorKind::NotFound => (ErrorCode::NotFound, "not_found"),
        std::io::ErrorKind::PermissionDenied => (ErrorCode::PermissionDenied, "permission_denied"),
        _ => (ErrorCode::Internal, "io_error"),
    };
    AgentError::new(code, format!("{op} 失败 {}: {error}", path.display()))
        .with_details(serde_json::json!({ "reason": reason, "op": op, "path": display(path) }))
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AgentError> {
    serde_json::from_value(params).map_err(|error| {
        AgentError::new(ErrorCode::InvalidRequest, "invalid hosted parameters")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, AgentError> {
    serde_json::to_value(value).map_err(|error| {
        AgentError::new(
            ErrorCode::Internal,
            format!("failed to encode hosted result: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn zombies_are_not_reported_as_running_elsewhere() {
        // 真机原样：9727 在跑（S），11049 是僵尸（Z）
        let (comm, _, state) = parse_comm_ppid_state("9727 (auth-server) S 1 9727 0 0\n").unwrap();
        // 真机形状：ppid=1（父进程已退出）
        let (_, ppid, _) = parse_comm_ppid_state("3571 (auth-server) S 1 3568 3568\n").unwrap();
        assert_eq!(ppid, 1);
        // comm 里有空格时也不能错位：整行 split 会把 ppid 读成 comm 中间那个词
        let (_, ppid, _) = parse_comm_ppid_state("42 (top - 12:00) S 41 42 42\n").unwrap();
        assert_eq!(ppid, 41, "comm 含空格时 ppid 必须从最后一个 ) 之后数");
        assert_eq!((comm.as_str(), state), ("auth-server", 'S'));
        let (_, _, state) = parse_comm_ppid_state("11049 (auth-server) Z 1 11049 0 0\n").unwrap();
        assert_eq!(state, 'Z', "僵尸要能被认出来，否则又变成「它在跑」的假警报");
        // comm 内含括号时必须从最后一个 ) 切
        let (comm, _, state) = parse_comm_ppid_state("7 ((weird (x)) name) S 1 7\n").unwrap();
        assert_eq!((comm.as_str(), state), ("(weird (x)) name", 'S'));
        assert!(parse_comm_ppid_state("garbage without parens").is_none());
    }

    #[test]
    fn comm_matching_survives_the_kernels_15_char_truncation() {
        assert!(comm_matches("auth-server", "auth-server"));
        // 内核把 comm 截到 15 字符：长名字必须还能认出来，否则又会显示成"没在跑"
        // TASK_COMM_LEN=16 → 内核只留 15 个字符
        assert_eq!("my-very-long-dumper-name".chars().count().min(15), 15);
        assert!(comm_matches("my-very-long-dumper-name", "my-very-long-du"));
        // 短名字不能被前缀误伤（comm 必须一字不差）
        assert!(!comm_matches("frida-server", "frida"));
        assert!(!comm_matches("server", "auth-server"));
    }

    #[test]
    fn externals_exclude_what_we_started_and_carry_the_shape() {
        let mut by_comm = HashMap::new();
        // (pid, ppid)：9727 是自己 fork 出去后被 init 收养的子进程；9728 是我们仍在管的
        // 那个进程本身（在运行表里，界面已经显示"运行中"，不该再算"表外"）
        by_comm.insert(
            "auth-server".to_string(),
            vec![(9727_u32, 1u32), (9728, 4321)],
        );
        let ours: HashSet<u32> = [9728].into_iter().collect();
        let found = external_procs_for("auth-server", &by_comm, &ours);
        assert_eq!(found.len(), 1, "只该留下表外那条: {found:?}");
        assert_eq!(found[0].pid, 9727);
        assert_eq!(
            found[0].ppid, 1,
            "ppid 必须带出来，界面靠它说\"父进程已退出\""
        );
        assert!(external_procs_for("frida-server", &by_comm, &ours).is_empty());
    }

    #[test]
    fn ppid_is_parsed_after_the_last_paren_not_by_splitting_the_line() {
        // comm 里可以有空格（`top - 12:00` 这类），整行 split 会把 ppid 读成 comm 中间一个词
        let stat = "42 (top - 12:00) S 41 42 42 0";
        let (_, ppid, _) = parse_comm_ppid_state(stat).unwrap();
        assert_eq!(ppid, 41, "ppid 必须从最后一个 ) 之后数");
        // uid 读不到时返回 None：界面宁可不写，也不要填个 0 假装"是 root 起的"
        assert_eq!(real_uid(0), None);
    }

    use super::*;

    fn record(handle: &str, pid: u32, ticks: u64) -> HostedRunRecord {
        HostedRunRecord {
            handle: handle.to_string(),
            name: "toybox".into(),
            pid,
            start_time_ticks: ticks,
            started_at_unix: 1_760_000_000,
            log_path: "/data/local/tmp/.toybox.run.log".into(),
            root: false,
            state: HostedRunState::Running,
            exit_code: None,
            detail: None,
        }
    }

    #[test]
    fn hosted_path_rejects_names_that_could_escape_or_hide() {
        for bad in [
            "",
            ".hidden",
            "../etc/passwd",
            "sub/dir",
            "with space",
            "quote'.so",
            "$(id)",
            "x".repeat(200).as_str(),
        ] {
            let error = hosted_path(bad).expect_err(&format!("{bad:?} 必须被拒"));
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{bad:?}");
            assert_eq!(error.details.unwrap()["reason"], "invalid_name");
        }
        assert_eq!(
            hosted_path("frida-server").unwrap(),
            PathBuf::from("/data/local/tmp/frida-server")
        );
    }

    /// comm 里带 `)` 与空格是常态（`/proc/<pid>/comm` 截到 15 字符，但 stat 里可能更怪）：
    /// 必须从最后一个 `)` 之后数列，否则 starttime 会读成别人的字段。
    #[test]
    fn parse_start_time_ticks_counts_fields_from_last_paren() {
        let mut tail = vec!["S".to_string()];
        tail.extend((1..=18_u64).map(|value| value.to_string()));
        tail.push("987654".to_string());
        let line = format!("4321 (weird) name) {}", tail.join(" "));
        assert_eq!(parse_start_time_ticks(&line), Some(987654));
        assert_eq!(parse_start_time_ticks("no paren here"), None);
        assert_eq!(
            parse_start_time_ticks("1 (a) S 1"),
            None,
            "字段不足时必须给不出而不是猜 0"
        );
    }

    #[test]
    fn elf_magic_detects_real_binaries_and_rejects_junk() {
        let dir = tempfile::tempdir().unwrap();
        let elf = dir.path().join("bin");
        std::fs::write(&elf, [0x7f, b'E', b'L', b'F', 2, 1, 1, 0]).unwrap();
        let script = dir.path().join("script.sh");
        std::fs::write(&script, b"#!/system/bin/sh\necho hi\n").unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        assert!(is_elf(&elf).unwrap());
        assert!(!is_elf(&script).unwrap());
        assert!(!is_elf(&empty).unwrap(), "空文件不是 ELF 也不该报错");
        // 读不到的文件要给错误而不是静默当成「不是 ELF」
        assert!(is_elf(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn reconcile_never_confuses_reused_pid_with_the_original_process() {
        // 一个真实存在、且 start time 必然与记录不符的 pid：本进程的 pid
        let pid = std::process::id();
        let actual = read_start_time_ticks(pid);
        let Some(actual) = actual else {
            // macOS 宿主没有 /proc：该分支只在设备上有效，跳过而不是假绿
            return;
        };
        let (state, detail) = reconcile(actual + 1, pid);
        assert_eq!(state, HostedRunState::Exited);
        assert!(detail.unwrap().starts_with("pid_reused_new_start_ticks="));
        let (state, detail) = reconcile(actual, pid);
        assert_eq!(
            state,
            HostedRunState::Running,
            "同一个 pid 且 ticks 相同才算还活着"
        );
        assert_eq!(detail, None);
    }

    #[test]
    fn reconcile_marks_missing_process_as_exited_and_pid_zero_as_unknown() {
        let (state, detail) = reconcile(7, 0);
        assert_eq!(state, HostedRunState::Unknown);
        assert_eq!(detail.unwrap(), "pid_zero");
        // 一个不可能存在的 pid：/proc 下没有它 → exited（不是 unknown）
        let (state, detail) = reconcile(7, u32::MAX);
        assert_eq!(state, HostedRunState::Exited);
        assert_eq!(detail.unwrap(), "process_gone");
    }

    #[test]
    fn refresh_reaps_finished_children_and_keeps_exit_code_only_for_spawned() {
        let mut table = Table::default();
        table.runs.insert(
            "aabb".into(),
            ManagedRun {
                record: record("aabb", 4242, 11),
                child: None,
                source: RunSource::Reconciled,
            },
        );
        let runs = refresh(&mut table);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, RunSource::Reconciled);
        assert_eq!(runs[0].1.exit_code, None, "对账来的记录不能有退出码");
        // u32::MAX 这个 pid 一定不存在 → 必须被刷成 exited
        assert_eq!(runs[0].1.state, HostedRunState::Exited);

        table.runs.insert(
            "ccdd".into(),
            ManagedRun {
                record: record("ccdd", 9999, 7),
                child: None,
                source: RunSource::Spawned,
            },
        );
        let runs = refresh(&mut table);
        assert_eq!(runs.len(), 2);
        assert!(
            runs[0].1.started_at_unix <= runs[1].1.started_at_unix,
            "按启动时间稳定排序"
        );
    }

    #[test]
    fn stored_records_round_trip_and_reject_unknown_shapes() {
        let payload = StoredRun {
            record: record("ff00", 1, 2),
        };
        let text = serde_json::to_vec(&payload).unwrap();
        let back: StoredRun = serde_json::from_slice(&text).unwrap();
        assert_eq!(back.record.handle, "ff00");
        assert!(serde_json::from_str::<StoredRun>("{}").is_err());
    }

    /// 写操作的守卫字段名必须钉住：`expected_pid` 拼错时不是「报错」，
    /// 而是守卫静默失效（Agent 照原 PID 发信号），所以这里锁死协议里的字段名。
    #[test]
    fn stop_guard_field_name_is_snake_case_and_optional() {
        let params: HostedStopParams =
            serde_json::from_value(serde_json::json!({ "handle": "aa", "expected_pid": 7 }))
                .unwrap();
        assert_eq!(params.expected_pid, Some(7));
        assert_eq!(params.signal, KillSignal::Term, "缺省必须是温和的 SIGTERM");
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value["expected_pid"], serde_json::json!(7));
        // 驼峰写法不被识别：字段留空，说明拼错会静默放宽守卫，调用方必须用 DTO
        let loose: HostedStopParams =
            serde_json::from_value(serde_json::json!({ "handle": "aa", "expectedPid": 7 }))
                .unwrap();
        assert_eq!(loose.expected_pid, None);
    }

    #[test]
    fn hosted_provider_info_is_stable_and_methods_unique() {
        let provider = HostedProvider::new();
        assert_eq!(provider.info().name, "hosted");
        assert_eq!(provider.methods(), HOSTED_METHODS);
        assert_eq!(HOSTED_METHODS.len(), 5);
    }

    #[test]
    fn start_requires_exec_bit_and_rejects_non_elf_before_spawning() {
        let provider = HostedProvider::new();
        // 非法名先在白名单处被挡下，不会走到 spawn
        let error = provider
            .start(serde_json::json!({ "name": "../escape" }))
            .expect_err("非法名必须拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        // root=true：Agent 是 shell 身份，直接 permission_denied 且不去试启动
        let error = provider
            .start(serde_json::json!({ "name": "toybox", "root": true }))
            .expect_err("root 启动要求必须显式拒绝");
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert_eq!(error.details.unwrap()["reason"], "root_required");
        // 不存在的文件：not_found（而不是启动一个空气进程）
        let error = provider
            .start(serde_json::json!({ "name": "definitely-absent-bin" }))
            .expect_err("缺文件必须报错");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    fn table_with(record: HostedRunRecord, source: RunSource) -> HostedProvider {
        let provider = HostedProvider::new();
        {
            let mut table = provider.lock();
            table.loaded = true;
            table.runs.insert(
                record.handle.clone(),
                ManagedRun {
                    record,
                    child: None,
                    source,
                },
            );
        }
        provider
    }

    #[test]
    fn stop_refuses_stale_pid_before_touching_the_process() {
        // 调用方拿着旧界面里的 PID 来停：必须先拒止，绝不能按数字发信号
        let provider = table_with(record("aabbccddeeff0011", 4242, 11), RunSource::Spawned);
        let error = provider
            .stop(serde_json::json!({ "handle": "aabbccddeeff0011", "expected_pid": 999 }))
            .expect_err("PID 与调用方看到的不一致时必须拒止");
        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        let details = error.details.unwrap();
        assert_eq!(details["reason"], "pid_mismatch");
        assert_eq!(details["expected_pid"], serde_json::json!(999));
        assert_eq!(details["actual_pid"], serde_json::json!(4242));
    }

    #[test]
    fn stop_of_unknown_handle_is_not_found() {
        let provider = HostedProvider::new();
        let error = provider
            .stop(serde_json::json!({ "handle": "1122334455667788" }))
            .expect_err("未知句柄必须 not_found");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.details.unwrap()["reason"], "unknown_handle");
    }

    /// 目标早就不在了：幂等成功 + 记录出表，不报错（重复点「停止」不该红字）。
    #[test]
    fn stop_of_gone_process_is_idempotent_and_drops_the_record() {
        let provider = table_with(
            record("1122334455667788", u32::MAX, 7),
            RunSource::Reconciled,
        );
        let value = provider
            .stop(serde_json::json!({ "handle": "1122334455667788" }))
            .expect("目标已消失应作为幂等成功返回");
        let result: HostedStopResult = serde_json::from_value(value).unwrap();
        assert_eq!(result.outcome, KillOutcome::AlreadyGone);
        assert_eq!(result.record.state, HostedRunState::Exited);
        assert!(result.record_dropped, "已确认死亡的记录不该留在对账表里");
        assert!(!result.identity_verified, "没核出身份就不能声称核过");
        assert_eq!(
            result.record.detail.as_deref(),
            Some("process_gone"),
            "要能区分『核出 PID 易主』与『进程本来就不在』"
        );
    }

    /// Linux 上才有 /proc：PID 易主时必须判 already_gone 而不是补一刀，
    /// 且调用方给的 expected_pid 不符时一律先拒止。
    #[cfg(target_os = "linux")]
    #[test]
    fn stop_never_signals_a_reused_pid() {
        let self_pid = std::process::id();
        let actual = read_start_time_ticks(self_pid).expect("本进程 start time 应可读");
        let provider = table_with(
            record("cafe000000000001", self_pid, actual + 1),
            RunSource::Reconciled,
        );
        let value = provider
            .stop(serde_json::json!({ "handle": "cafe000000000001" }))
            .expect("复用场景应作为幂等成功返回而不是报错");
        let result: HostedStopResult = serde_json::from_value(value).unwrap();
        assert_eq!(result.outcome, KillOutcome::AlreadyGone);
        assert!(
            result.record.detail.unwrap().starts_with("pid_reused"),
            "必须说明为什么不动手"
        );
        assert!(
            read_start_time_ticks(self_pid).is_some(),
            "本进程必须还活着：绝不能向新租户发信号"
        );
    }

    #[test]
    fn poll_dead_returns_immediately_for_absent_process() {
        assert!(poll_dead(u32::MAX), "不存在的 pid 必须立刻判已消失");
    }

    #[test]
    fn hosted_provider_declares_all_five_hosted_methods() {
        let provider = HostedProvider::new();
        assert_eq!(provider.methods().len(), 5);
        assert!(provider.methods().contains(&HOSTED_STOP));
    }

    #[test]
    fn status_of_unknown_handle_is_not_found_with_reason() {
        let provider = HostedProvider::new();
        let error = provider
            .status(serde_json::json!({ "handle": "ffffffffffffffff" }))
            .expect_err("未知句柄必须 not_found");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.details.unwrap()["reason"], "unknown_handle");
    }
}

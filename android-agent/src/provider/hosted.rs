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
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use agent_protocol::method::{
    HOSTED_ADOPT, HOSTED_CHMOD, HOSTED_LIST, HOSTED_PROBE, HOSTED_START, HOSTED_STATUS,
    HOSTED_STOP, HOSTED_WRITE,
};
use agent_protocol::{
    AgentError, ErrorCode, ExternalProc, FileKind, HostedAdoptParams, HostedAdoptResult,
    HostedBinaryInfo, HostedChmodParams, HostedChmodResult, HostedListParams, HostedListResult,
    HostedProbeParams, HostedProbeResult, HostedRunRecord, HostedRunState, HostedStartParams,
    HostedStartResult, HostedStatusParams, HostedStatusResult, HostedStdinMode, HostedStopParams,
    HostedStopResult, HostedWriteParams, HostedWriteResult, KillOutcome, KillSignal,
    PERMISSION_BITS, ProviderHealth, ProviderInfo, render_mode_text,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::process::{SignalResult, send_signal};
use super::{Provider, ProviderFuture, RequestContext};

const HOSTED_METHODS: &[&str] = &[
    HOSTED_LIST,
    HOSTED_ADOPT,
    HOSTED_CHMOD,
    HOSTED_START,
    HOSTED_STATUS,
    HOSTED_STOP,
    HOSTED_PROBE,
    HOSTED_WRITE,
];
/// 托管目录固定路径（与 Desktop 的 `adb::HOSTED_DIR` 一致，用户指定的默认目录）。
const HOSTED_DIR: &str = "/data/local/tmp";
/// 运行记录落盘目录：Agent 重启后靠它对账，不靠内存表。
const STATE_DIR: &str = "/data/local/tmp/app-reverse-tools-hosted";
const MAX_BINARIES: usize = 2_000;
/// 记录文件保留上限（按 mtime 留最近这些），防止长期堆积。
const MAX_RUN_RECORDS: usize = 200;
/// 探测结束后最多再等输出流这么久：等不到就按已读到的收口，并在结果里说明
const STREAM_GRACE: std::time::Duration = std::time::Duration::from_millis(400);
/// stdin 上限来自协议层（两条支路共用同一个数，见 `MAX_HOSTED_STDIN_BYTES`）
const MAX_STDIN_BYTES: usize = agent_protocol::MAX_HOSTED_STDIN_BYTES;
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
    /// 持续输入的写入端。
    ///
    /// 用 `tokio::sync::Mutex` 而不是直接把 `ChildStdin` 放着：写入是 async 的，
    /// 绝不能带着 `std::sync::Mutex` 的守卫去 await（future 就不 Send 了）。
    /// 只有 Agent 亲自起、且启动时接管了 stdin 的记录才会有；重启/认领/Root 支路都是 None，
    /// 界面上的「已失去输入通道」就是从这里的实况来的，不是靠记录里的声称。
    stdin: Option<Arc<tokio::sync::Mutex<tokio::process::ChildStdin>>>,
    source: RunSource,
}

/// 记录来源：决定能不能给出退出码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunSource {
    /// 本 Agent 亲自启动，持有子进程句柄
    Spawned,
    /// 从磁盘对账恢复（已不是自己的子进程）
    Reconciled,
    /// AR7.7：桌面侧（Legacy/root 支路）起的进程事后认领进来，同样没有子进程句柄
    Adopted,
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
                HOSTED_ADOPT => self.adopt(params),
                HOSTED_CHMOD => self.chmod(params),
                HOSTED_START => self.start(params),
                HOSTED_STATUS => self.status(params),
                HOSTED_STOP => self.stop(params),
                HOSTED_PROBE => self.probe(params).await,
                HOSTED_WRITE => self.write(params).await,
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
        // 路径/普通文件/ELF/执行位这四道检查与 probe 共用（同一个判据不能两份实现）
        let path = validated_executable(&params.name)?;
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
        // 一次性 stdin：有内容才接管道，没有就照旧给 /dev/null
        // （给 /dev/null 是有意的：不接管 stdin 的程序读到 EOF 会自己往下走）
        let stdin_data = take_one_shot_stdin(params.stdin_data.as_deref())?;
        let mut command = tokio::process::Command::new(&path);
        command
            .current_dir(HOSTED_DIR)
            .args(&params.args)
            .stdin(if stdin_data.is_some() || params.interactive_stdin {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            })
            .stdout(std::process::Stdio::from(stdout))
            .stderr(std::process::Stdio::from(stderr))
            // 独立进程组：Agent 停止时不把托管进程一起带走（对齐原 nohup 语义）
            .process_group(0)
            .kill_on_drop(false);
        let mut child = command.spawn().map_err(|error| {
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
        // 写入端先取出来。两种用法**必须走两条路**：
        // - 一次性输入：写完后由这个任务**持有并 drop** 管道 —— 管道的 EOF 是"写端全关了"
        //   才出现的，把它存进运行表就永远不关，等 stdin 的程序会一直卡着
        //   （真机腿抓到过：cat 收完内容不退，状态写着 once 却还 Running）；
        // - 持续输入：句柄留在运行表里（界面上的输入框只在设备说 open 时才长出来）。
        let interactive = params.interactive_stdin;
        let mut pipe = (stdin_data.is_some() || interactive)
            .then(|| child.stdin.take())
            .flatten();
        // 状态按**实际拿到什么**记，不按调用方要求记：要了持续输入而管道没接上却记成
        // Open，界面上就会长出一个吞字的假输入框。
        let stdin_mode = match (&pipe, interactive) {
            (Some(_), true) => HostedStdinMode::Open,
            (Some(_), false) => HostedStdinMode::Once,
            (None, _) => HostedStdinMode::None,
        };
        let stdin_arc = if interactive {
            pipe.take().map(|p| Arc::new(tokio::sync::Mutex::new(p)))
        } else {
            None
        };
        if let Some(arc) = stdin_arc.clone() {
            // 持续输入：首段内容（如果有）写进同一个管道，但**不关**
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt as _;
                let mut pipe = arc.lock().await;
                if let Some(text) = stdin_data {
                    if let Err(error) = pipe.write_all(text.as_bytes()).await {
                        eprintln!("hosted: 写启动输入失败 pid={pid}: {error}");
                    }
                }
            });
        } else if let Some(pipe) = pipe.take() {
            // 一次性：写完就让它随任务结束一起关掉，这就是 EOF。
            // 子进程不读 stdin 时写入会堵在管道里 —— 所以不能在主路径上同步写。
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt as _;
                let mut pipe = pipe;
                if let Some(text) = stdin_data {
                    if let Err(error) = pipe.write_all(text.as_bytes()).await {
                        eprintln!("hosted: 写启动输入失败 pid={pid}: {error}");
                    }
                }
                let _ = pipe.shutdown().await;
                drop(pipe); // 管道的 EOF 发生在写端全部关闭时
            });
        }
        let handle = random_handle()?;
        let record = HostedRunRecord {
            handle: handle.clone(),
            name: params.name.clone(),
            pid,
            start_time_ticks,
            started_at_unix: unix_now(),
            log_path: log_path.clone(),
            args: params.args.clone(),
            stdin_mode: Some(stdin_mode),
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
                stdin: stdin_arc,
                source: RunSource::Spawned,
            },
        );
        eprintln!(
            "audit method={HOSTED_START} handle={handle} name={} pid={} args={:?} log={log_path}",
            params.name, record.pid, params.args
        );
        serialize(HostedStartResult { record })
    }

    /// AR7.7：把「桌面侧代跑、我们没句柄」的进程认领进运行表。
    ///
    /// 存在的理由不是"让界面好看"，而是**root 支路的启动从来不经 Agent**：
    /// `hosted.start{root:true}` 在设备侧是 `PermissionDenied`（Agent 跑 shell 身份），
    /// 于是 `device_binary_run(root=true)` 走 Legacy `su -c "cd D; nohup ./x >log 2>&1 & echo $!"`。
    /// 那条路不进运行表 ⇒ 软件重启/刷新页面之后，设备上没有任何凭据说"这是本工具起的"，
    /// 进程就显示成表外（用户的原话：明明是我启动的，怎么说不是我启动的）。
    ///
    /// 认领必须**拿设备上的实证**换，不能拿桌面的声明换：这条记录之后就是「终止」按钮的
    /// 依据，认错一个 pid 等于拿别人的进程当自己的杀。四道证据缺一不可（见 `adopt_refusal`）：
    /// 活着不是僵尸、`comm` 与托管文件名相符、`cmdline` 的 argv0 相符、进程启动时刻在窗口内。
    /// `exe` 能读就读（同 uid 可读，读得到就是最强证据）；跨 uid 读不到时**如实记为读不到**。
    fn adopt(&self, params: Value) -> Result<Value, AgentError> {
        let params: HostedAdoptParams = parse_params(params)?;
        self.ensure_loaded()?;
        let path = hosted_path(&params.name)?;
        let pid = params.pid;
        if pid == 0 {
            return Err(invalid("pid_zero", "pid 为 0 不能认领为运行记录"));
        }
        // 先把"托管目录里真的有这个文件"钉住：认领不能凭空调出一个不存在的二进制
        let metadata = std::fs::metadata(&path).map_err(|error| io_error("stat", &path, error))?;
        if !metadata.is_file() {
            return Err(invalid(
                "not_a_regular_file",
                format!("托管目标不是普通文件: {}", display(&path)),
            ));
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|error| {
            AgentError::new(
                ErrorCode::PreconditionFailed,
                format!("pid={pid} 读不到 /proc/{pid}/stat，拒绝认领: {error}"),
            )
            .with_details(serde_json::json!({ "reason": "stat_unreadable", "pid": pid }))
        })?;
        let Some((comm, _ppid, state)) = parse_comm_ppid_state(&stat) else {
            return Err(AgentError::new(
                ErrorCode::PreconditionFailed,
                format!("/proc/{pid}/stat 解析不出 comm/state，拒绝认领"),
            )
            .with_details(serde_json::json!({ "reason": "stat_unparsable", "pid": pid })));
        };
        let Some(ticks) = parse_start_time_ticks(&stat) else {
            return Err(AgentError::new(
                ErrorCode::PreconditionFailed,
                format!("/proc/{pid}/stat 读不到启动时刻，拒绝认领"),
            )
            .with_details(serde_json::json!({ "reason": "start_time_unreadable", "pid": pid })));
        };
        let argv0 = cmdline_argv0(pid);
        let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
            .ok()
            .map(|value| value.to_string_lossy().into_owned());
        let uid = real_uid(pid);
        let started_unix = process_start_unix(ticks);
        if let Some((reason, message)) = adopt_refusal(
            &params.name,
            &path,
            &comm,
            state,
            argv0.as_deref(),
            exe.as_deref(),
            uid,
            started_unix,
            unix_now(),
        ) {
            return Err(AgentError::new(ErrorCode::PreconditionFailed, message)
                .with_details(serde_json::json!({ "reason": reason, "pid": pid })));
        }
        // 属主以实测为准（不采信调用方声称的 root）：界面要靠它决定走哪条终止链路
        let root = uid == Some(0);
        let mut proofs = vec![
            format!("comm={comm}"),
            format!("argv0={}", argv0.clone().unwrap_or_default()),
            match uid {
                Some(value) => format!("uid={value}"),
                None => "uid_unreadable".to_string(),
            },
            match &exe {
                Some(value) => format!("exe={value}"),
                None => "exe_unreadable(跨 uid)".to_string(),
            },
            match (started_unix, uid) {
                (Some(at), _) => format!("started_at={at}"),
                (None, _) => "start_time_unreadable".to_string(),
            },
        ];
        if params.root != root {
            proofs.push(format!(
                "root_claim_discrepant(claimed={},measured={root})",
                params.root
            ));
        }
        let mut table = self.lock();
        // 幂等：同一个 pid 已经活着记在表里就回原来那条，不能发第二个句柄
        if let Some(existing) = table
            .runs
            .values()
            .find(|run| run.record.pid == pid && run.record.state == HostedRunState::Running)
        {
            let record = existing.record.clone();
            return serialize(HostedAdoptResult {
                record,
                verified_by: proofs,
                already: true,
            });
        }
        let handle = random_handle()?;
        let record = HostedRunRecord {
            handle: handle.clone(),
            name: params.name.clone(),
            pid,
            start_time_ticks: ticks,
            started_at_unix: started_unix.unwrap_or_else(unix_now),
            log_path: format!("{HOSTED_DIR}/.{}.run.log", params.name),
            // 认领进来的记录也要带参数：root 支路 Agent 没见过那次 exec，
            // 不带上就只剩 pid，界面上看不出"当初用什么参数跑的"
            args: params.args.clone(),
            // 认领来的进程 Agent 没见过那次 exec，stdin 在谁手里都不知道：只能记"没有通道"
            stdin_mode: Some(HostedStdinMode::None),
            root,
            state: HostedRunState::Running,
            exit_code: None,
            detail: Some("adopted".to_string()),
        };
        self.persist(&record);
        table.runs.insert(
            handle.clone(),
            ManagedRun {
                record: record.clone(),
                child: None,
                // root/Legacy 支路起的进程：Agent 没见过那次 exec，没有 stdin 句柄
                stdin: None,
                source: RunSource::Adopted,
            },
        );
        eprintln!(
            "audit method={HOSTED_ADOPT} handle={handle} name={} pid={} root={root} claimed_root={} proofs={:?}",
            params.name, pid, params.root, proofs
        );
        serialize(HostedAdoptResult {
            record,
            verified_by: proofs,
            already: false,
        })
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
            reconciled: !matches!(source, RunSource::Spawned),
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
    /// 输入通道写坏了：收回句柄并把记录降级，别让界面继续显示"可输入"。
    fn drop_stdin(&self, handle: &str, mode: HostedStdinMode) {
        let mut table = self.lock();
        if let Some(run) = table.runs.get_mut(handle) {
            run.stdin = None;
            run.record.stdin_mode = Some(mode);
            let record = run.record.clone();
            self.persist(&record);
        }
    }

    /// 第二层：拿一组参数把托管二进制起来一次，只回报事实。
    ///
    /// 与 `start` 的三条差别，都是为了"探测连试九个候选也不该在手机上留下东西"：
    /// ① 不进运行表、不落记录、不追加 `.name.run.log`；
    /// ② 到点没退就**连整个进程组**一起杀（只杀父进程会留下占端口的孤儿），
    ///    杀完回读 `/proc` 确认；确认不了就写 `still_running=true`，不装作清干净了；
    /// ③ 探测一律不给 stdin（`/dev/null`）：否则"它在等输入"会被误报成"它进了服务模式"。
    ///
    /// Agent 只有 shell 身份，探测也就以 shell 跑，不假装能提权（同 D026）；
    /// 必须 root 才能起的东西会给出退出码与报错，那本身也是有用的事实。
    ///
    /// 这里**不判断"这段输出算不算帮助"**：每个程序触发 help 后的反应差别太大，
    /// 任何模板化识别都会把误判当结论。stdout/stderr 原文带回去给人看，
    /// 分类只按客观事实（有没有输出、有没有退、多久、什么码）。
    async fn probe(&self, params: Value) -> Result<Value, AgentError> {
        let params: HostedProbeParams = parse_params(params)?;
        if params.args.len() > agent_protocol::MAX_HOSTED_ARGS {
            return Err(invalid(
                "too_many_args",
                format!(
                    "探测参数 {} 个，超过上限 {}",
                    params.args.len(),
                    agent_protocol::MAX_HOSTED_ARGS
                ),
            ));
        }
        for arg in &params.args {
            if arg.len() > agent_protocol::MAX_HOSTED_ARG_LEN {
                return Err(invalid(
                    "arg_too_long",
                    format!(
                        "单个参数最长 {} 字节，这里有一个 {} 字节",
                        agent_protocol::MAX_HOSTED_ARG_LEN,
                        arg.len()
                    ),
                ));
            }
        }
        let timeout = std::time::Duration::from_millis(
            params
                .timeout_ms
                .unwrap_or(agent_protocol::HOSTED_PROBE_DEFAULT_TIMEOUT_MS)
                .clamp(200, agent_protocol::HOSTED_PROBE_MAX_TIMEOUT_MS),
        );
        // 与 start 共用同一套目标检查：不能借探测去"顺便执行"托管目录外的东西
        // 非法文件名是调用方的 bug：这种照旧响亮报错，不混进"这个文件跑不了"
        let path = hosted_path(&params.name)?;
        let path = match check_executable_target(&path).map(|()| path.clone()) {
            Ok(path) => path,
            Err(error) => {
                // 探测的意义就是"这样跑会怎样"。目标本身不可执行是一类**结论**
                // （界面归到"无法执行"），不能报成"我没连上设备"那种失败。
                return serialize(HostedProbeResult {
                    args: params.args.clone(),
                    started: false,
                    pid: 0,
                    exit_code: None,
                    signal: None,
                    timed_out: false,
                    killed: false,
                    still_running: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                    elapsed_ms: 0,
                    detail: Some(describe_refusal(&error)),
                });
            }
        };

        let mut command = tokio::process::Command::new(&path);
        command
            .current_dir(HOSTED_DIR)
            .args(&params.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                // "起不来"是一种结论，不是一次 RPC 失败：报成错误的话，界面就分不出
                // "这个文件无法执行"和"我没连上设备"这两件完全不同的事
                return serialize(HostedProbeResult {
                    args: params.args.clone(),
                    started: false,
                    pid: 0,
                    exit_code: None,
                    signal: None,
                    timed_out: false,
                    killed: false,
                    still_running: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    stdout_bytes: 0,
                    stderr_bytes: 0,
                    truncated: false,
                    elapsed_ms: 0,
                    detail: Some(spawn_failure_hint(&error)),
                });
            }
        };
        let pid = child.id().unwrap_or(0);
        // 刚起来时的启动时刻：后面判断"还在 /proc 里的是不是同一个进程"全靠它
        let born_ticks = read_start_time_ticks(pid);
        let out_sink = Arc::new(Sink::default());
        let err_sink = Arc::new(Sink::default());
        let stop = Arc::new(AtomicBool::new(false));
        let mut pumps = Vec::new();
        if let Some(pipe) = child.stdout.take() {
            pumps.push(tokio::spawn(pump_read(
                pipe,
                out_sink.clone(),
                stop.clone(),
            )));
        }
        if let Some(pipe) = child.stderr.take() {
            pumps.push(tokio::spawn(pump_read(
                pipe,
                err_sink.clone(),
                stop.clone(),
            )));
        }

        let began = std::time::Instant::now();
        let mut timed_out = false;
        let mut killed = false;
        let mut status: Option<std::process::ExitStatus> = None;
        let mut detail: Option<String> = None;
        match tokio::time::timeout(timeout, child.wait()).await {
            Ok(Ok(value)) => status = Some(value),
            Ok(Err(error)) => detail = Some(format!("等待退出失败: {error}")),
            Err(_) => {
                timed_out = true;
                kill_process_group(pid);
                let _ = child.start_kill();
                killed = true;
                // 杀完再收一次：拿到信号退出码就照实记，收不到就留 None（不猜）
                if let Ok(Ok(value)) =
                    tokio::time::timeout(std::time::Duration::from_millis(1_000), child.wait())
                        .await
                {
                    status = Some(value);
                }
            }
        }
        let elapsed_ms = began.elapsed().as_millis() as u64;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for handle in pumps {
            let abort = handle.abort_handle();
            // 输出流可能被后代进程继续持有：只等有界时间，等不到就用已读到的部分并说明
            if tokio::time::timeout(STREAM_GRACE, handle).await.is_err() {
                abort.abort();
                detail =
                    Some("输出流没随进程关闭（可能有后代进程还持有它），已按有界等待收口".into());
            }
        }

        // 还在不在：按启动时刻比。光看 /proc 存在会把 PID 复用说成"它没退"
        let (still_running, alive_detail) = match (born_ticks, read_start_time_ticks(pid)) {
            (Some(born), Some(now)) => {
                (now == born, (now != born).then(|| "pid_reused".to_string()))
            }
            (Some(_), None) => (false, None),
            (None, _) => (
                std::path::Path::new(&format!("/proc/{pid}")).exists(),
                Some("start_time_unreadable".to_string()),
            ),
        };
        if detail.is_none() {
            detail = alive_detail;
        }
        let (stdout_text, stdout_kept) = out_sink.snapshot();
        let (stderr_text, stderr_kept) = err_sink.snapshot();
        let truncated = stdout_kept < out_sink.total() || stderr_kept < err_sink.total();
        eprintln!(
            "audit method={HOSTED_PROBE} name={} args={:?} exit={:?} signal={:?} timed_out={timed_out} elapsed_ms={elapsed_ms}",
            params.name,
            params.args,
            status.as_ref().and_then(|value| value.code()),
            status.as_ref().and_then(|value| value.signal()),
        );
        serialize(HostedProbeResult {
            args: params.args,
            started: true,
            pid,
            exit_code: status.as_ref().and_then(|value| value.code()),
            signal: status.as_ref().and_then(|value| value.signal()),
            timed_out,
            killed,
            still_running,
            stdout: stdout_text,
            stderr: stderr_text,
            stdout_bytes: out_sink.total(),
            stderr_bytes: err_sink.total(),
            truncated,
            elapsed_ms,
            detail,
        })
    }

    /// 第四层：向运行中的托管进程持续输入。
    ///
    /// 能不能写得看**设备上的句柄实况**，不看记录里的声称：Agent 重启后记录还在、
    /// 进程也可能还在，但 stdin 的写入端早断了。那种情况必须明确拒掉并说清原因，
    /// 界面才能显示"已失去输入通道"，而不是留一个看着能输、其实把字吞掉的框。
    async fn write(&self, params: Value) -> Result<Value, AgentError> {
        use tokio::io::AsyncWriteExt as _;
        let params: HostedWriteParams = parse_params(params)?;
        if params.text.len() > agent_protocol::MAX_HOSTED_WRITE_BYTES {
            return Err(invalid(
                "write_too_large",
                format!(
                    "单次输入 {} 字节，超过上限 {} 字节",
                    params.text.len(),
                    agent_protocol::MAX_HOSTED_WRITE_BYTES
                ),
            ));
        }
        self.ensure_loaded()?;
        // 取句柄这段不跨 await：带着表锁去写管道会把整张表钉住
        let pipe = {
            let mut table = self.lock();
            let Some(run) = table.runs.get_mut(&params.handle) else {
                return Err(AgentError::new(
                    ErrorCode::NotFound,
                    format!("没有句柄 {} 的运行记录", params.handle),
                )
                .with_details(serde_json::json!({ "reason": "unknown_handle" })));
            };
            // 先判"进程还在不在"，再判"通道在不在"：往一只已经退了的处理上写字，
            // 报"没通道"会把真原因（它已经死了）盖掉。
            // 但这句判断必须**拿得出证据**才说：自己起的孩子用 try_wait 实收
            // （收到状态顺手 reap，退出码就有了）；手里没孩子时只认记录里
            // refresh 已经判过的状态，不去猜——猜出来的"已退出"会遮掉
            // "根本没有输入通道"这个可操作的实话说。
            let gone = match run.child.as_mut() {
                Some(child) => {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        reap(run);
                        true
                    } else {
                        false
                    }
                }
                None => run.record.state == HostedRunState::Exited,
            };
            if gone {
                return Err(invalid(
                    "process_exited",
                    format!("进程已退出，这句输入没人收（{:?}）", run.record.exit_code),
                ));
            }
            match (run.stdin.clone(), run.record.stdin_mode) {
                (Some(pipe), Some(HostedStdinMode::Open)) => pipe,
                (_, mode) => {
                    // 原因按**记录声称的状态**判：声称能写却拿不出句柄，才是"通道丢了"；
                    // 本来就一次性用完/没有接管，那是"这条通道没开着"，两句话不一样。
                    let reason = if run.record.stdin_mode == Some(HostedStdinMode::Open) {
                        "stdin_handle_gone"
                    } else {
                        "stdin_closed"
                    };
                    return Err(invalid(
                        "stdin_unavailable",
                        format!(
                            "这条记录现在没有可用的输入通道（设备侧状态：{}）",
                            mode.map(HostedStdinMode::as_str)
                                .unwrap_or("老版本 Agent 未告知"),
                        ),
                    )
                    .with_details(serde_json::json!({
                        "reason": reason,
                        "stdin_mode": mode.map(HostedStdinMode::as_str),
                    })));
                }
            }
        };
        let mut guard = pipe.lock().await;
        let wrote = guard.write_all(params.text.as_bytes()).await;
        if let Err(error) = wrote {
            drop(guard);
            // 写不进去说明这条通道已经是死的了：把实况收回并降级记录，别留着骗界面
            self.drop_stdin(&params.handle, HostedStdinMode::Lost);
            return Err(
                AgentError::new(ErrorCode::Internal, format!("写入失败: {error}"))
                    .with_details(serde_json::json!({ "reason": "stdin_broken" })),
            );
        }
        let _ = guard.flush().await;
        let closing = params.close;
        if closing {
            // shutdown 之后就是 EOF：程序读到 EOF 自己往下走，这条通道到此为止
            let _ = guard.shutdown().await;
        }
        drop(guard);

        let mut table = self.lock();
        let run = match table.runs.get_mut(&params.handle) {
            Some(run) => run,
            None => {
                return Err(AgentError::new(
                    ErrorCode::NotFound,
                    format!("写入期间运行记录 {} 消失了", params.handle),
                )
                .with_details(serde_json::json!({ "reason": "unknown_handle" })));
            }
        };
        if closing {
            run.stdin = None;
            run.record.stdin_mode = Some(HostedStdinMode::Once);
        }
        let record = run.record.clone();
        self.persist(&record);
        let stdin_mode = record.stdin_mode.unwrap_or(HostedStdinMode::None);
        eprintln!(
            "audit method={HOSTED_WRITE} handle={} bytes={} close={closing} stdin_mode={}",
            params.handle,
            params.text.len(),
            stdin_mode.as_str()
        );
        serialize(HostedWriteResult {
            record,
            bytes_written: params.text.len() as u64,
            stdin_mode,
        })
    }

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
                    // 写入端跟着上一个 Agent 一起没了：进程可能还活着，但再也喂不进去
                    if record.stdin_mode == Some(HostedStdinMode::Open) {
                        record.stdin_mode = Some(HostedStdinMode::Lost);
                    }
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
                            stdin: None,
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

/// 认领窗口（秒）：只接受"刚刚起起来的"进程。
///
/// 为什么要有窗口而不是只看进程名：光凭 `comm`/`argv0` 相符就认领，等于承认
/// "这台机上任何一只叫 auth-server 的进程都是我们起的"——那正是"拿缺失的证据编结论"。
/// 加了时间窗，认领才真的意味着"这是刚才那次启动的产物"。
const ADOPT_WINDOW_SECS: u64 = 120;

/// `/proc/<pid>/stat` 第 22 字段的单位：Android userland 固定 100（`getconf CLK_TCK` 实测）。
const USER_HZ: u64 = 100;

/// 认领的否决判断（纯函数：每条守卫都能单独钉一条断言，不依赖设备上的 /proc）。
/// 返回 `Some((reason, message))` 就是拒绝；`None` 才是"可以写进运行表"。
#[allow(clippy::too_many_arguments)]
fn adopt_refusal(
    name: &str,
    path: &Path,
    comm: &str,
    state: char,
    argv0: Option<&str>,
    exe: Option<&str>,
    uid: Option<u32>,
    started_unix: Option<u64>,
    now: u64,
) -> Option<(&'static str, String)> {
    if matches!(state, 'Z' | 'X' | 'x') {
        return Some((
            "zombie",
            format!("pid 的进程状态是 {state}（已死待回收），不能认领成在跑"),
        ));
    }
    if !comm_matches(name, comm) {
        return Some((
            "comm_mismatch",
            format!("设备上的进程名是 {comm}，与托管文件 {name} 对不上，拒绝认领"),
        ));
    }
    let target = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    match argv0 {
        None => {
            return Some((
                "argv0_unreadable",
                format!("读不到 pid 的 cmdline，无法确认它跑的是 {name}，拒绝认领"),
            ));
        }
        Some(value) => {
            let base = value.rsplit('/').next().unwrap_or("");
            if base != target && value != path.to_string_lossy().as_ref() {
                return Some((
                    "argv0_mismatch",
                    format!("启动命令行是 {value}，不是托管文件 {name}，拒绝认领"),
                ));
            }
        }
    }
    if let Some(value) = exe {
        // 文件被覆盖时内核会在链接后面加 " (deleted)"：路径仍然指向我们托管的那个 inode
        let trimmed = value.strip_suffix(" (deleted)").unwrap_or(value);
        if trimmed != path.to_string_lossy().as_ref() {
            return Some((
                "exe_mismatch",
                format!("pid 实际运行的可执行文件是 {value}，不是 {name}，拒绝认领"),
            ));
        }
    }
    if uid.is_none() {
        return Some((
            "uid_unreadable",
            "读不到进程属主，界面上就定不了该走哪条终止链路，拒绝认领".to_string(),
        ));
    }
    match started_unix {
        None => Some((
            "start_time_unreadable",
            "算不出进程的启动时刻，无法判断它是不是刚才那次启动，拒绝认领".to_string(),
        )),
        Some(at) if at > now.saturating_add(5) => Some((
            "start_time_in_future",
            format!("进程启动时刻 {at} 晚于当前时间 {now}，设备时钟不可信，拒绝认领"),
        )),
        Some(at) if now.saturating_sub(at) > ADOPT_WINDOW_SECS => Some((
            "out_of_window",
            format!(
                "这个进程已经跑了 {}s，超过认领窗口 {ADOPT_WINDOW_SECS}s，不能算成本次启动的产物",
                now.saturating_sub(at)
            ),
        )),
        _ => None,
    }
}

/// `cmdline` 的第一个非空项（argv0）。空 cmdline 一般是内核线程，返回 `None`。
fn cmdline_argv0(pid: u32) -> Option<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    raw.split(|byte| *byte == 0)
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .find(|part| !part.is_empty())
}

/// 进程启动的墙上时间：`/proc/stat` 的 `btime` + `starttime_ticks / USER_HZ`。
/// 任何一环读不到都返回 `None`——宁可不认领，也不拿一个猜出来的时刻去过时间窗。
fn process_start_unix(ticks: u64) -> Option<u64> {
    let btime = boot_time_unix()?;
    btime.checked_add(ticks.checked_div(USER_HZ)?)
}

fn boot_time_unix() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().find(|line| line.starts_with("btime "))?;
    line.split_whitespace().nth(1)?.parse().ok()
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
/// 回收自己起的子进程并记下死因。
///
/// ⚠️ 只能在**已经确认它退了**之后调用：本函数会放掉 `child` 句柄，
/// 进程还在跑就叫它，等于把"以后还能收到退出码"这件事扔掉——
/// 界面上就只剩一个" exited 但不知道码"的记录。`refresh` 里那一步是
/// 先 `try_wait` 拿到状态才清句柄的，别照它抄成"无条件 reap"。
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

/// 把"没起来"的原因收成一句可判读的话：设备侧的 reason 码 + Agent 的中文说明。
///
/// 界面靠 reason 分类（不是靠正则匹配中文），所以这个串里必须两者都带上。
fn describe_refusal(error: &AgentError) -> String {
    let reason = error
        .details
        .as_ref()
        .and_then(|value| value.get("reason"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    format!("{reason}: {}", error.message)
}

/// start 与 probe 共用的目标检查：托管目录内、普通文件、ELF、有 owner 执行位。
///
/// 两处必须同一份实现：探测历史上就是"能不能跑"的第二条路，判据一旦分叉，
/// 就会出现 start 拒了而 probe 偷偷执行（或反过来）这种没人能解释的差别。
fn validated_executable(name: &str) -> Result<PathBuf, AgentError> {
    let path = hosted_path(name)?;
    check_executable_target(&path)?;
    Ok(path)
}

/// 目标本身能不能执行：普通文件 / ELF / owner 执行位。
///
/// 单独拆出来是给 probe 用的——"这个文件跑不了"在探测里是一类**结论**，
/// 而"文件名非法"是调用方的 bug，两者不能混成同一种回话。
fn check_executable_target(path: &Path) -> Result<(), AgentError> {
    let metadata = std::fs::metadata(path).map_err(|error| io_error("stat", path, error))?;
    if !metadata.is_file() {
        return Err(invalid(
            "not_a_regular_file",
            format!("托管目标不是普通文件: {}", display(path)),
        ));
    }
    if !is_elf(path).unwrap_or(false) {
        return Err(invalid(
            "not_an_elf",
            format!("托管目标不是 ELF 可执行文件: {}", display(path)),
        ));
    }
    if metadata.mode() & 0o100 == 0 {
        return Err(invalid(
            "not_executable",
            format!("缺少 owner 执行位，请先调 hosted.chmod: {}", display(path)),
        ));
    }
    Ok(())
}

/// 探测时一条输出流的去处：留住的字节 + 进程真实写出的字节。
///
/// 两个数都要有：只显示"留了多少"会把截断说成"它只输出了这些"。
#[derive(Default)]
struct Sink {
    kept: Mutex<Vec<u8>>,
    total: std::sync::atomic::AtomicU64,
}

impl Sink {
    fn snapshot(&self) -> (String, u64) {
        let kept = self
            .kept
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            String::from_utf8_lossy(&kept).into_owned(),
            kept.len() as u64,
        )
    }

    fn total(&self) -> u64 {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// 一直读到这一端关闭为止；超过上限的部分**继续读但不留**。
///
/// 为什么不能读满就停：管道塞满后进程会卡在 write 上不退，探测结论就变成
/// "它没退出"——那是我们自己的读法造成的假象。
async fn pump_read<R>(mut reader: R, sink: Arc<Sink>, stop: Arc<AtomicBool>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    use tokio::io::AsyncReadExt as _;
    let mut chunk = vec![0_u8; 8 * 1024];
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        match reader.read(&mut chunk).await {
            Ok(0) => return,
            Ok(n) => {
                sink.total
                    .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
                let mut kept = sink
                    .kept
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let room = agent_protocol::MAX_HOSTED_PROBE_OUTPUT_BYTES.saturating_sub(kept.len());
                let take = room.min(n);
                kept.extend_from_slice(&chunk[..take]);
            }
            // 读不动就当这条流没有输出：退出码与信号仍是事实，不额外编原因
            Err(_) => return,
        }
    }
}

/// 杀掉整个进程组（负 pid）。
///
/// 探测与启动都用 `process_group(0)` 起孩子，pgid 就等于那次进程的 pid。
/// 只 `child.kill()` 的话，它 fork 出去的那一半会留在设备上占着端口——
/// 这正是本工具"换个参数再起就 bind failed"的来历。
fn kill_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    let target = -(pid as libc::pid_t);
    // SIGKILL 而不是 TERM：探测超时的候选多半正在等输入或已经进了服务模式
    unsafe { libc::kill(target, libc::SIGKILL) };
}

/// 起不来的原因要说人话：errno 谁都不想看，但"为什么"必须留下。
fn spawn_failure_hint(error: &std::io::Error) -> String {
    let errno = error.raw_os_error().unwrap_or_default();
    let hint: String = match errno {
        13 => "没有执行权限（先给它加执行位，或确认它所在目录没有 noexec)".into(),
        2 => "文件不存在（列表可能已经过期，刷新一次再看)".into(),
        8 => "不是可执行格式（ELF 头对但架构或动态链接器不对)".into(),
        26 => "文件正忙（已被另一个进程占用)".into(),
        _ => error.to_string(),
    };
    format!("无法执行: {hint}（errno {errno}）")
}

/// 一次性启动输入的校验：超限就拒，**绝不截断**。
///
/// 截断等于让目标程序读到半句输入（少一个换行、少一段配置），现场比"起不来"难查得多。
/// 空字符串按"没填"处理：走 `/dev/null`，程序读到 EOF 自己往下走。
fn take_one_shot_stdin(text: Option<&str>) -> Result<Option<String>, AgentError> {
    let Some(text) = text.filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    if text.len() > MAX_STDIN_BYTES {
        return Err(invalid(
            "stdin_too_large",
            format!(
                "启动输入 {} 字节，超过上限 {MAX_STDIN_BYTES} 字节；这里不截断，请改小，或改用运行中持续输入",
                text.len()
            ),
        ));
    }
    Ok(Some(text.to_owned()))
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
    fn adopt_refusal_demands_every_piece_of_evidence() {
        use std::path::Path;
        let hosted = Path::new("/data/local/tmp/auth-server");
        let ok = |argv0: Option<&str>, exe: Option<&str>| {
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                argv0,
                exe,
                Some(0),
                Some(1_760_000_000),
                1_760_000_030,
            )
        };
        // 齐全的证据：放行
        assert_eq!(ok(Some("./auth-server"), None), None);
        assert_eq!(
            ok(
                Some("/data/local/tmp/auth-server"),
                Some("/data/local/tmp/auth-server")
            ),
            None
        );
        // 证据齐全的这一支本身必须走通，否则后面每一条反向断言都是空的
        assert_eq!(ok(Some("./auth-server"), None), None);
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "other-daemon",
                'S',
                Some("./other-daemon"),
                None,
                Some(0),
                Some(1_760_000_000),
                1_760_000_030,
            )
            .map(|pair| pair.0),
            Some("comm_mismatch")
        );
        // 僵尸：有 pid 也没在跑
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'Z',
                Some("./auth-server"),
                None,
                Some(0),
                Some(1_760_000_000),
                1_760_000_030,
            )
            .map(|pair| pair.0),
            Some("zombie")
        );
        // argv0 读不到 = 没法确认它跑的是哪个文件
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                None,
                None,
                Some(0),
                Some(1_760_000_000),
                1_760_000_030,
            )
            .map(|pair| pair.0),
            Some("argv0_unreadable")
        );
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("/system/bin/sh"),
                None,
                Some(0),
                Some(1_760_000_000),
                1_760_000_030,
            )
            .map(|pair| pair.0),
            Some("argv0_mismatch")
        );
        // exe 读得到却不指向托管文件：最强证据反了，必须拒
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("./auth-server"),
                Some("/data/local/tmp/other"),
                Some(0),
                Some(1_760_000_000),
                1_760_000_030,
            )
            .map(|pair| pair.0),
            Some("exe_mismatch")
        );
        // 文件被覆盖过：内核在链接后加 " (deleted)"，路径仍指向我们托管的那个 inode → 放行
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("./auth-server"),
                Some("/data/local/tmp/auth-server (deleted)"),
                Some(0),
                Some(1_760_000_000),
                1_760_000_030,
            ),
            None
        );
        // 属主读不到：界面定不了该走哪条终止链路
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("./auth-server"),
                None,
                None,
                Some(1_760_000_000),
                1_760_000_030,
            )
            .map(|pair| pair.0),
            Some("uid_unreadable")
        );
        // 启动时刻算不出来 → 不能拿猜出来的时间去判"是不是刚才那次启动"
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("./auth-server"),
                None,
                Some(0),
                None,
                1_760_000_030,
            )
            .map(|pair| pair.0),
            Some("start_time_unreadable")
        );
        // 跑了很久：那是别人的实例（或上一次会话起的），不能算本次启动的产物
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("./auth-server"),
                None,
                Some(0),
                Some(1_760_000_000),
                1_760_000_000 + 121,
            )
            .map(|pair| pair.0),
            Some("out_of_window")
        );
        // 窗口边界内仍放行（120s）
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("./auth-server"),
                None,
                Some(0),
                Some(1_760_000_000),
                1_760_000_120,
            ),
            None
        );
        // 设备时钟不可信（启动时刻在未来）
        assert_eq!(
            adopt_refusal(
                "auth-server",
                hosted,
                "auth-server",
                'S',
                Some("./auth-server"),
                None,
                Some(0),
                Some(1_760_000_100),
                1_760_000_000,
            )
            .map(|pair| pair.0),
            Some("start_time_in_future")
        );
    }

    #[test]
    fn adopted_records_have_no_child_handle_so_status_must_not_claim_exit_code() {
        // 认领来的记录没有子进程句柄：`reconciled` 必须为 true，否则界面会以为拿得到退出码
        assert!(!matches!(RunSource::Adopted, RunSource::Spawned));
        assert!(!matches!(RunSource::Reconciled, RunSource::Spawned));
        assert!(matches!(RunSource::Spawned, RunSource::Spawned));
    }

    #[test]
    fn process_start_unix_adds_ticks_to_boot_time() {
        // USER_HZ=100：20000 ticks = 200s
        let Some(btime) = boot_time_unix() else {
            // 非 Android/无 /proc/stat 的构建环境（CI 之外）跳过，但在这台机上不该发生
            return;
        };
        assert_eq!(process_start_unix(20_000), Some(btime + 200));
        assert_eq!(process_start_unix(0), Some(btime));
    }

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
            args: Vec::new(),
            stdin_mode: Some(HostedStdinMode::Open),
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
                stdin: None,
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
                stdin: None,
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

    /// 探测的入参检查必须**在看文件之前**：越界的 argv 不该让设备去 open 一次。
    #[tokio::test]
    async fn probe_rejects_oversized_argv_before_touching_the_filesystem() {
        let provider = HostedProvider::new();
        let many: Vec<String> = (0..agent_protocol::MAX_HOSTED_ARGS + 1)
            .map(|index| index.to_string())
            .collect();
        let error = provider
            .probe(serde_json::json!({ "name": "toybox", "args": many }))
            .await
            .expect_err("参数个数越界必须拒");
        assert_eq!(error.details.unwrap()["reason"], "too_many_args");
        let long = "x".repeat(agent_protocol::MAX_HOSTED_ARG_LEN + 1);
        let error = provider
            .probe(serde_json::json!({ "name": "toybox", "args": [long] }))
            .await
            .expect_err("单参数过长必须拒");
        assert_eq!(error.details.unwrap()["reason"], "arg_too_long");
    }

    /// 目标本身不可执行是一类**结论**（界面归"无法执行"），不是这次 RPC 失败：
    /// 报成失败的话，界面就分不出"这个文件跑不了"和"我没连上设备"。
    #[tokio::test]
    async fn probe_reports_unusable_target_as_a_result_not_an_error() {
        let provider = HostedProvider::new();
        let value = provider
            .probe(serde_json::json!({ "name": "definitely-absent-bin", "args": ["-h"] }))
            .await
            .expect("探测缺文件应当作为结论返回");
        let result: HostedProbeResult = serde_json::from_value(value).unwrap();
        assert!(!result.started, "没起来就得说没起来");
        assert!(!result.timed_out && !result.killed, "没起来与超时是两回事");
        let detail = result.detail.expect("没起来必须给出原因");
        assert!(
            detail.starts_with("not_found"),
            "detail 要带 reason 码，界面按它分类而不是匹配中文：{detail}"
        );
        // 非法文件名仍然是一次错误（白名单守卫必须响亮）
        assert!(
            provider
                .probe(serde_json::json!({ "name": "../escape", "args": ["-h"] }))
                .await
                .is_err()
        );
    }

    /// 超时的兜底：上限来自协议层，Agent 自己也要夹住，
    /// 否则一个 600s 的 timeout 会把"探测"变成"在设备上挂一个服务"。
    #[test]
    fn probe_timeout_is_clamped_to_the_protocol_bounds() {
        let params: HostedProbeParams = serde_json::from_value(serde_json::json!({
            "name": "toybox", "timeout_ms": 999_999_u64
        }))
        .unwrap();
        let clamped = params
            .timeout_ms
            .unwrap_or(agent_protocol::HOSTED_PROBE_DEFAULT_TIMEOUT_MS)
            .clamp(200, agent_protocol::HOSTED_PROBE_MAX_TIMEOUT_MS);
        assert_eq!(clamped, agent_protocol::HOSTED_PROBE_MAX_TIMEOUT_MS);
        let absent: HostedProbeParams =
            serde_json::from_value(serde_json::json!({ "name": "toybox" })).unwrap();
        assert_eq!(
            absent
                .timeout_ms
                .unwrap_or(agent_protocol::HOSTED_PROBE_DEFAULT_TIMEOUT_MS),
            4_000
        );
    }

    /// 持续输入的门槛看**句柄实况**：记录声称 Open 而句柄已经没了（Agent 重启过），
    /// 必须拒掉并把原因带回去——界面据此显示"已失去输入通道"，而不是留个吞字的框。
    #[tokio::test]
    async fn write_refuses_when_the_record_claims_open_but_no_handle_exists() {
        let provider = table_with(record("aabbccddeeff0011", 4242, 11), RunSource::Reconciled);
        let error = provider
            .write(serde_json::json!({ "handle": "aabbccddeeff0011", "text": "y\n" }))
            .await
            .expect_err("没有句柄就没有通道");
        assert_eq!(
            error.details.as_ref().unwrap()["reason"],
            "stdin_handle_gone"
        );
        // 一次性输入用完之后（once）同样写不进去：那条管道早就 EOF 了
        let once = HostedRunRecord {
            stdin_mode: Some(HostedStdinMode::Once),
            ..record("112233445566", 4242, 11)
        };
        let provider = table_with(once, RunSource::Spawned);
        let error = provider
            .write(serde_json::json!({ "handle": "112233445566", "text": "y\n" }))
            .await
            .expect_err("once 之后不该再写得进去");
        assert_eq!(error.details.as_ref().unwrap()["reason"], "stdin_closed");
    }

    /// 未知句柄与超限输入都要在碰到进程之前就拒掉。
    #[tokio::test]
    async fn write_rejects_unknown_handle_and_oversized_text() {
        let provider = HostedProvider::new();
        let error = provider
            .write(serde_json::json!({ "handle": "ffffffffffffffff", "text": "hi" }))
            .await
            .expect_err("没有这条记录");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.details.unwrap()["reason"], "unknown_handle");
        let big = "x".repeat(agent_protocol::MAX_HOSTED_WRITE_BYTES + 1);
        let error = provider
            .write(serde_json::json!({ "handle": "ffffffffffffffff", "text": big }))
            .await
            .expect_err("超限必须拒，不截断");
        assert_eq!(error.details.unwrap()["reason"], "write_too_large");
    }

    /// 输出流的截断只该发生在**回传**：真实写出量必须仍然报得出来，
    /// 否则界面会把"我只留了 64K"说成"它只输出了 64K"。
    #[tokio::test]
    async fn probe_output_sink_keeps_head_and_reports_true_total() {
        let sink = Arc::new(Sink::default());
        let stop = Arc::new(AtomicBool::new(false));
        let payload = vec![b'a'; agent_protocol::MAX_HOSTED_PROBE_OUTPUT_BYTES + 4_096];
        pump_read(std::io::Cursor::new(payload.clone()), sink.clone(), stop).await;
        let (text, kept) = sink.snapshot();
        assert_eq!(kept as usize, agent_protocol::MAX_HOSTED_PROBE_OUTPUT_BYTES);
        assert_eq!(sink.total() as usize, payload.len());
        assert_eq!(text.len(), agent_protocol::MAX_HOSTED_PROBE_OUTPUT_BYTES);
        assert!(text.starts_with('a'), "留的必须是开头");
    }

    /// 一次性 stdin：空串按"没填"处理（走 /dev/null），超限明确拒且不截断。
    #[test]
    fn one_shot_stdin_rejects_over_limit_instead_of_truncating() {
        assert!(take_one_shot_stdin(None).unwrap().is_none());
        assert!(
            take_one_shot_stdin(Some("")).unwrap().is_none(),
            "空串等于没填：不该因此把 stdin 从 /dev/null 改成管道"
        );
        let text = "x".repeat(agent_protocol::MAX_HOSTED_STDIN_BYTES);
        assert_eq!(
            take_one_shot_stdin(Some(&text)).unwrap().unwrap().len(),
            text.len()
        );
        let error = take_one_shot_stdin(Some(
            &"x".repeat(agent_protocol::MAX_HOSTED_STDIN_BYTES + 1),
        ))
        .expect_err("超限必须拒");
        assert_eq!(error.details.unwrap()["reason"], "stdin_too_large");
    }

    #[test]
    fn hosted_provider_info_is_stable_and_methods_unique() {
        let provider = HostedProvider::new();
        assert_eq!(provider.info().name, "hosted");
        assert_eq!(provider.methods(), HOSTED_METHODS);
        assert_eq!(HOSTED_METHODS.len(), 8);
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
                    stdin: None,
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
    fn hosted_provider_declares_all_eight_hosted_methods() {
        let provider = HostedProvider::new();
        assert_eq!(provider.methods().len(), 8);
        assert!(provider.methods().contains(&HOSTED_STOP));
        // AR7.7 新增：认领方法必须被宣告出来，否则 Desktop 的路由看不到它
        assert!(provider.methods().contains(&HOSTED_ADOPT));
    }

    #[test]
    fn adopt_refuses_before_touching_proc_when_the_file_is_not_hosted() {
        let provider = HostedProvider::new();
        // 非法名：白名单先挡下（不信任 Desktop，§3.7）
        let error = provider
            .adopt(serde_json::json!({ "name": "../escape", "pid": 1 }))
            .expect_err("非法名必须拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.details.unwrap()["reason"], "invalid_name");
        // 合法名但托管目录里没有这个文件：不能凭空调出一个不存在的二进制
        let error = provider
            .adopt(serde_json::json!({ "name": "definitely-absent-bin", "pid": 1 }))
            .expect_err("缺文件必须报错");
        assert_eq!(
            error.code,
            ErrorCode::NotFound,
            "文件不在托管目录：{:?} {}",
            error.code,
            error.message
        );
        // pid=0 连认领对象都没有
        let error = provider
            .adopt(serde_json::json!({ "name": "toybox", "pid": 0 }))
            .expect_err("pid 0 必须拒");
        assert_eq!(error.details.unwrap()["reason"], "pid_zero");
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

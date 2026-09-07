//! ProcessService：所有外部命令的统一执行入口（P2）。
//! 规则：
//! - 禁止 UI 直接 spawn；一切命令经 TaskService 进入任务上下文；
//! - 参数以结构化 CommandSpec（executable + args 数组）传递，不拼 shell 字符串；
//! - stdout/stderr 逐行实时泵出（回调），退出返回 Outcome（退出码/取消/超时）；
//! - 取消与超时均为「先 terminate 优雅退出，2s 宽限后强杀」；
//! - Unix 用 SIGTERM/SIGKILL；Windows 用 taskkill /T（tree）[/F]。

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};

use crate::core::error::{CoreError, CoreResult};

const GRACE_AFTER_TERM: Duration = Duration::from_secs(2);
const HARD_CAP_AFTER_TERM: Duration = Duration::from_secs(5);
const MAX_LINE_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Stdout,
    Stderr,
    System,
}

impl StreamKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamKind::Stdout => "stdout",
            StreamKind::Stderr => "stderr",
            StreamKind::System => "system",
        }
    }
}

/// 结构化命令规格（CommandSpec）。
#[derive(Debug, Clone)]
pub struct CommandSpec {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub timeout: Option<Duration>,
    pub env_extra: HashMap<String, String>,
}

/// 取消令牌：watch 通道实现，可 await 取消信号（无竞态丢通知问题）。
#[derive(Clone)]
pub struct CancellationToken {
    tx: watch::Sender<bool>,
    rx: watch::Receiver<bool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        let (tx, rx) = watch::channel(false);
        Self { tx, rx }
    }
}

impl CancellationToken {
    pub fn cancel(&self) {
        let _ = self.tx.send(true);
    }

    /// 等待取消发生（已取消则立即返回）。
    pub async fn cancelled(&self) {
        let mut rx = self.rx.clone();
        // changed() 首次会等待下一次值变化；初值 false 时等到 true。
        while !*rx.borrow_and_update() {
            if rx.changed().await.is_err() {
                return; // sender dropped 视为终止信号
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 进程自然退出，携带退出码
    Exited(Option<i32>),
    /// 用户取消（已 terminate + 必要时强杀）
    Cancelled,
    /// 超时触发的终止
    TimedOut,
}

/// 向 UI/DB 泵出一行输出。回调内禁止长阻塞。
pub type Sink = Box<dyn Fn(StreamKind, &str) + Send + Sync + 'static>;

pub async fn execute(
    spec: CommandSpec,
    sink: Sink,
    cancel: CancellationToken,
) -> CoreResult<Outcome> {
    let mut cmd = Command::new(&spec.executable);
    cmd.args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in &spec.env_extra {
        cmd.env(k, v);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| CoreError::Internal(format!("启动进程失败 {}: {e}", spec.executable)))?;
    let pid = child.id().unwrap_or(0);
    sink(StreamKind::System, &format!("pid={pid}"));

    let (tx, mut rx) = mpsc::unbounded_channel::<(StreamKind, String)>();
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    // 两个泵任务：逐行读取 → 通道；EOF 时 drop sender（ChildStdout/ChildStderr 类型不同，分开 spawn）
    spawn_pump(BufReader::new(stdout), StreamKind::Stdout, tx.clone());
    spawn_pump(BufReader::new(stderr), StreamKind::Stderr, tx);

    // 退出码等待器
    let (exit_tx, exit_rx) = oneshot::channel();
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                let _ = exit_tx.send(status.code());
            }
            Err(_) => {
                let _ = exit_tx.send(None);
            }
        }
    });
    let mut exit_rx = exit_rx;

    let mut exit_code: Option<Option<i32>> = None; // 外层 None=未退出，内层 None=无码
    let mut rx_open = true;
    let mut terminate_sent_at: Option<tokio::time::Instant> = None;
    let mut force_sent = false;
    let mut reason: Option<Outcome> = None;

    // 截止时间一次性锚定：避免每轮 select 重建 sleep 导致持续输出的进程永不超时
    let deadline: Option<tokio::time::Instant> =
        spec.timeout.map(|d| tokio::time::Instant::now() + d);

    loop {
        let timeout_fut = async {
            match deadline {
                Some(t) => tokio::time::sleep_until(t).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            biased;
            // 输出泵
            msg = rx.recv(), if rx_open => {
                match msg {
                    Some((kind, line)) => sink(kind, &line),
                    None => rx_open = false,
                }
            }
            // 进程退出
            code = &mut exit_rx, if exit_code.is_none() => {
                exit_code = Some(code.unwrap_or(None));
            }
            // 取消
            _ = cancel.cancelled(), if reason.is_none() => {
                terminate_tree(pid);
                terminate_sent_at = Some(tokio::time::Instant::now());
                reason = Some(Outcome::Cancelled);
                sink(StreamKind::System, "任务被取消，正在终止进程…");
            }
            // 超时
            _ = timeout_fut, if reason.is_none() && spec.timeout.is_some() => {
                terminate_tree(pid);
                terminate_sent_at = Some(tokio::time::Instant::now());
                reason = Some(Outcome::TimedOut);
                sink(StreamKind::System, "任务超时，正在终止进程…");
            }
        }

        // 取消/超时后升级：宽限期到后强杀一次（幂等标志防重复）
        if !force_sent {
            if let Some(at) = terminate_sent_at {
                if tokio::time::Instant::now() - at >= GRACE_AFTER_TERM {
                    force_kill(pid);
                    force_sent = true;
                }
            }
        }

        // 自然收束：已退出且输出读完
        if exit_code.is_some() && !rx_open {
            break;
        }
        // 终止收束：发出终止后，要么 EOF+退出，要么硬上限时间到
        if let Some(at) = terminate_sent_at {
            if tokio::time::Instant::now() - at >= HARD_CAP_AFTER_TERM {
                break;
            }
        }
    }

    match reason {
        Some(r) => Ok(r),
        None => Ok(Outcome::Exited(exit_code.flatten())),
    }
}

/// 逐行泵送 reader 输出到通道；EOF/sender 断开即结束并释放 sender。
fn spawn_pump<R>(
    reader: BufReader<R>,
    kind: StreamKind,
    tx: mpsc::UnboundedSender<(StreamKind, String)>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = truncate_bytes(&line, MAX_LINE_BYTES);
            if tx.send((kind, line)).is_err() {
                break;
            }
        }
        // sender drop → rx 端在两个泵都 EOF 后收到 None
    });
}

/// 按字节上限截断，回退到字符边界，避免切断多字节 UTF-8。
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…[truncated]", &s[..cut])
}

#[cfg(unix)]
fn terminate_tree(pid: u32) {
    if pid != 0 {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
}

#[cfg(unix)]
fn force_kill(pid: u32) {
    if pid != 0 {
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
fn terminate_tree(pid: u32) {
    if pid != 0 {
        taskkill(pid, false);
    }
}

#[cfg(windows)]
fn force_kill(pid: u32) {
    if pid != 0 {
        taskkill(pid, true);
    }
}

#[cfg(windows)]
fn taskkill(pid: u32, force: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = std::process::Command::new("taskkill");
    c.arg("/T").arg("/PID").arg(pid.to_string());
    if force {
        c.arg("/F");
    }
    c.creation_flags(CREATE_NO_WINDOW);
    let _ = c.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type SinkBuf = Arc<Mutex<Vec<(StreamKind, String)>>>;

    fn collect_sink() -> (Sink, SinkBuf) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let b2 = buf.clone();
        let sink: Sink = Box::new(move |k: StreamKind, s: &str| {
            b2.lock().unwrap().push((k, s.to_string()));
        });
        (sink, buf)
    }

    fn spec(exec: &str, args: &[&str], timeout: Option<Duration>) -> CommandSpec {
        CommandSpec {
            executable: exec.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            timeout,
            env_extra: HashMap::new(),
        }
    }

    #[cfg(unix)]
    fn echo_spec(msg: &str) -> CommandSpec {
        spec("/bin/echo", &[msg], None)
    }
    #[cfg(windows)]
    fn echo_spec(msg: &str) -> CommandSpec {
        spec("cmd.exe", &["/C", &format!("echo {msg}")], None)
    }

    #[cfg(unix)]
    fn slow_spec() -> CommandSpec {
        spec("/bin/sleep", &["30"], None)
    }
    #[cfg(windows)]
    fn slow_spec() -> CommandSpec {
        spec("cmd.exe", &["/C", "ping -n 30 127.0.0.1 > nul"], None)
    }

    #[cfg(unix)]
    fn exit_code_spec(code: i32) -> CommandSpec {
        spec("/bin/sh", &["-c", &format!("exit {code}")], None)
    }
    #[cfg(windows)]
    fn exit_code_spec(code: i32) -> CommandSpec {
        spec("cmd.exe", &["/C", &format!("exit {code}")], None)
    }

    #[tokio::test]
    async fn exited_task_streams_output() {
        let (sink, buf) = collect_sink();
        let outcome = execute(echo_spec("hello-p2"), sink, CancellationToken::default())
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::Exited(Some(0)));
        let lines = buf.lock().unwrap();
        assert!(
            lines
                .iter()
                .any(|(k, l)| *k == StreamKind::Stdout && l == "hello-p2"),
            "stdout 应被泵出: {lines:?}"
        );
        assert!(lines.iter().any(|(k, _)| *k == StreamKind::System));
    }

    #[tokio::test]
    async fn captures_nonzero_exit_code() {
        let (sink, _buf) = collect_sink();
        let outcome = execute(exit_code_spec(3), sink, CancellationToken::default())
            .await
            .unwrap();
        assert_eq!(outcome, Outcome::Exited(Some(3)));
    }

    #[tokio::test]
    async fn timeout_terminates_process() {
        let (sink, _buf) = collect_sink();
        let started = std::time::Instant::now();
        let outcome = execute(
            CommandSpec {
                timeout: Some(Duration::from_millis(300)),
                ..slow_spec()
            },
            sink,
            CancellationToken::default(),
        )
        .await
        .unwrap();
        assert_eq!(outcome, Outcome::TimedOut);
        // 终止应在 1s 内完成（300ms 超时 + 秒级 grace），不能等满 30s
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "耗时 {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn cancel_terminates_process() {
        let (sink, _buf) = collect_sink();
        let cancel = CancellationToken::default();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            c2.cancel();
        });
        let outcome = execute(slow_spec(), sink, cancel).await.unwrap();
        assert_eq!(outcome, Outcome::Cancelled);
    }

    #[tokio::test]
    async fn missing_executable_yields_error() {
        let (sink, _buf) = collect_sink();
        let res = execute(
            spec("/definitely/not/here-xyz", &[], None),
            sink,
            CancellationToken::default(),
        )
        .await;
        assert!(res.is_err());
    }
}

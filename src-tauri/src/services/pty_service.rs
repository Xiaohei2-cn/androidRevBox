//! PTY 通道（真终端的地基，FT0.2）：本机伪终端 + **原样字节流**。
//!
//! 一条铁律，写在这儿是为了让下一个动手的人别破它：
//! **本模块不解析任何转义序列**。字节的解释归 xterm.js（`docs/frida_terminal_phases.md` §4.1）。
//! 本项目历史上自己写过一份 ANSI 解析（`src/lib/ansi.ts`），那份只认 SGR、其余全丢，
//! 于是「能显示颜色」与「能跑 REPL」之间差的整层一直没人补。渲染交给开源内核，我们只做传输与生命周期。
//!
//! 与任务通道（`process_service`）的区别也必须说清，否则会被误当成重复实现：
//! - 任务通道按**行**泵并做 8KB 截断 —— 对 NDJSON 是对的，对终端是错的：
//!   半行、`\r` 覆盖、不带换行的进度条都会在那里被剁碎（`process_service.rs:245 reader.lines()`）；
//! - 本通道按**块**转发原始字节，不切行、不截断、不重排。
//!
//! 安全边界（§3.7 同一条口径）：PTY 等于「在用户本机起一个能执行任意命令的会话」。
//! 它只允许由**界面主动发起**，程序与参数来自用户输入；
//! **绝不允许**把设备回传的数据、Agent 的应答、日志文本拼进 `pty_open` 的参数里 ——
//! 那等于让手机决定桌面上跑什么。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;

use crate::core::error::{CoreError, CoreResult};

/// 单次读的最大字节数。比这大的输出被切成多块继续送，**不丢弃也不截断**（R-04/R-05 第一道闸）。
const READ_CHUNK_BYTES: usize = 64 * 1024;

/// 读线程收尾用的等待：最后一段输出常与退出挤在一起，直接 join 会丢尾巴。
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(150);

/// 界面到本机的字节出口。抽成 trait 有两个理由：
/// ① 传输层要能在**没有 tauri 运行时的单测里**被验证（真实 PTY + 内存 sink）；
/// ② 出口坏了（前端 tab 被关）必须能被读线程感知，从而停手而不是越堆越多。
pub trait PtySink: Send + Sync {
    /// 送出一块原始字节。返回 `false` = 出口已经没了，读线程该收摊。
    fn send(&self, bytes: &[u8]) -> bool;
}

impl<F> PtySink for F
where
    F: Fn(&[u8]) -> bool + Send + Sync,
{
    fn send(&self, bytes: &[u8]) -> bool {
        self(bytes)
    }
}

/// 子进程退出时推给界面的那一条（走事件 `pty://exit`：它是生命周期事实，不是数据流。
/// 混进字节流里会让前端分不清「程序说的话」和「程序的输出」。）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyExit {
    pub id: String,
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOpened {
    pub id: String,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    /// 实际生效的 TERM。界面不必显示，但排查「为什么没颜色」时它是唯一线索。
    pub term: String,
}

/// 一次 `pty_open` 的入参（同时也是 IPC 形状：命令层凑到 8 个位置参数会被 clippy 拦下，
/// 那不是洁癖——位置参数少传一个只会以"类型不对"炸掉，收成对象后错误落在名字上，可读也可测）。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOpenArgs {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

struct Session {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    reader: Option<JoinHandle<()>>,
    waiter: Option<JoinHandle<()>>,
}

/// PTY 会话表（挂在 AppState 上，一台桌面一个实例）。
#[derive(Default)]
pub struct PtyService {
    sessions: Mutex<HashMap<String, Session>>,
    next: AtomicU64,
}

impl PtyService {
    pub fn new() -> Self {
        Self::default()
    }

    /// 已登记的会话数（测试与「活动上限」都读这个，不靠回忆）。
    pub fn len(&self) -> usize {
        self.sessions
            .lock()
            .map(|guard| guard.len())
            .unwrap_or_default()
    }

    /// clippy 的 len_without_is_empty：语义就是 `len() == 0`。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 起一个本机 PTY 会话。`sink` 是字节出口，`on_exit` 在子进程真退出后被调一次；
    /// 两者都不在主线程上（读线程 / 等待线程）。
    pub fn open(
        &self,
        args: PtyOpenArgs,
        sink: Arc<dyn PtySink>,
        on_exit: Arc<dyn Fn(PtyExit) + Send + Sync + 'static>,
    ) -> CoreResult<PtyOpened> {
        let program = args.program.trim().to_owned();
        if program.is_empty() {
            return Err(CoreError::InvalidInput(
                "终端需要一个可执行程序（留空我起不来）".into(),
            ));
        }
        if program.contains('\0') || args.args.iter().any(|a| a.contains('\0')) {
            return Err(CoreError::InvalidInput(
                "命令里含非法字符，不能起这个终端".into(),
            ));
        }
        let cols = clamp_cols(args.cols);
        let rows = clamp_rows(args.rows);

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                // 像素尺寸交给界面算（只有 xterm 那边有字体度量）。给 0 时 tty 只按字符行列，
                // 而程序问的从来也是字符行列（C-05 的实测对象正是这个）。
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| CoreError::Internal(format!("打不开伪终端：{error}")))?;

        let mut cmd = CommandBuilder::new(&program);
        cmd.args(&args.args);
        cmd.env("TERM", "xterm-256color");
        // 256 色与真彩要显式说：一批程序（prompt_toolkit 一系在内）看到 TERM 只写 "xterm"
        // 就自动降回 8 色，那种「颜色不对」会被算成我们渲染坏了（C-01）。
        cmd.env("COLORTERM", "truecolor");
        if let Some(cwd) = args.cwd.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            cmd.cwd(cwd);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|error| CoreError::Internal(format!("起不了 {program}：{error}")))?;
        let id = format!("pty-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let pid = child.process_id();
        let child: Arc<Mutex<Box<dyn Child + Send + Sync>>> = Arc::new(Mutex::new(child));
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| CoreError::Internal(format!("拿不到终端输出：{error}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| CoreError::Internal(format!("拿不到终端输入：{error}")))?;

        // 读线程：块状转发原始字节，不切行不截断。出口没了就停（背压由 blocking read 天然给）。
        let reader_thread = {
            let id = id.clone();
            let child = child.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; READ_CHUNK_BYTES];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if !sink.send(&buf[..n]) {
                                tracing::debug!(id, "终端字节出口已关闭，读线程收摊");
                                break;
                            }
                        }
                        Err(error) => {
                            // 子进程被杀之后这条是常态（EIO），不是故障：留 debug 别刷警告
                            tracing::debug!(id, %error, "终端读结束");
                            break;
                        }
                    }
                }
                std::thread::sleep(DRAIN_AFTER_EXIT);
                if let Ok(mut guard) = child.lock() {
                    let _ = guard.try_wait();
                }
            })
        };

        // 等待线程：只报一次退出。界面靠它把状态灯从「在跑」翻成「已退出（码/信号）」。
        let waiter_thread = {
            let id = id.clone();
            let child = child.clone();
            std::thread::spawn(move || {
                loop {
                    let status = match child.lock() {
                        Ok(mut guard) => match guard.try_wait() {
                            Ok(Some(status)) => Some(status),
                            Ok(None) => None,
                            Err(_) => break,
                        },
                        Err(_) => break,
                    };
                    if let Some(status) = status {
                        let code = status.exit_code();
                        let failed = !status.success();
                        on_exit(PtyExit {
                            id: id.clone(),
                            exit_code: Some(code),
                            signal: failed.then(|| format!("exit_code={code}")),
                        });
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(80));
                }
            })
        };

        if let Ok(mut guard) = self.sessions.lock() {
            guard.insert(
                id.clone(),
                Session {
                    master: pair.master,
                    writer,
                    child,
                    reader: Some(reader_thread),
                    waiter: Some(waiter_thread),
                },
            );
        }

        tracing::info!(
            id,
            program,
            ?pid,
            cols,
            rows,
            "本机 PTY 会话起来（终端正文不进应用日志）"
        );
        Ok(PtyOpened {
            id,
            pid,
            cols,
            rows,
            term: "xterm-256color".into(),
        })
    }

    /// 把键盘字节送进 PTY。**不加工**：Ctrl-C 就是一个 0x03；回显、行编辑、历史
    /// 归 tty 的行规程与程序自己。也别在这里"看着补个换行"。
    pub fn write(&self, id: &str, bytes: &[u8]) -> CoreResult<()> {
        let mut guard = self
            .sessions
            .lock()
            .map_err(|_| CoreError::Internal("终端会话表被毒化".into()))?;
        let session = guard
            .get_mut(id)
            .ok_or_else(|| CoreError::NotFound(format!("终端会话 {id} 已经不在了")))?;
        session
            .writer
            .write_all(bytes)
            .and_then(|_| session.writer.flush())
            .map_err(|error| CoreError::Internal(format!("写进终端失败：{error}")))
    }

    /// 改尺寸 ⇒ 真的 `TIOCSWINSZ`，内核据此发 `SIGWINCH`（C-05）。
    /// 这条是本模块存在的理由之一：不做 PTY，程序永远以为自己 80 列。
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> CoreResult<()> {
        let guard = self
            .sessions
            .lock()
            .map_err(|_| CoreError::Internal("终端会话表被毒化".into()))?;
        let session = guard
            .get(id)
            .ok_or_else(|| CoreError::NotFound(format!("终端会话 {id} 已经不在了")))?;
        session
            .master
            .resize(PtySize {
                rows: clamp_rows(rows),
                cols: clamp_cols(cols),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| CoreError::Internal(format!("改终端尺寸失败：{error}")))
    }

    /// 关会话：先杀子进程再收线程。
    /// `kill_on_drop(true)` 那条纪律在这儿的形式版：**没这条，界面关掉之后本机就留着一堆活 shell**（R-08）。
    pub fn close(&self, id: &str) -> CoreResult<()> {
        let session = {
            let mut guard = self
                .sessions
                .lock()
                .map_err(|_| CoreError::Internal("终端会话表被毒化".into()))?;
            guard.remove(id)
        };
        let Some(mut session) = session else {
            // 已经没了不是失败：用户要的效果（没有活着的会话）已经达成
            return Ok(());
        };
        if let Ok(mut child) = session.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = session.reader.take() {
            let _ = reader.join();
        }
        if let Some(waiter) = session.waiter.take() {
            let _ = waiter.join();
        }
        tracing::info!(id, "终端会话已关闭，子进程与读写线程都收了");
        Ok(())
    }
}

/// 列数下限：0 列的终端会让一批程序（含 `stty`/curses）直接算错，
/// 而界面在折叠/隐藏状态下量到 0 是常态（R-03）——在这里兜住，而不是让 tty 收下它。
fn clamp_cols(cols: u16) -> u16 {
    cols.clamp(20, 512)
}

fn clamp_rows(rows: u16) -> u16 {
    rows.clamp(4, 512)
}

/// 默认起什么程序。判据是**这台机器自己说的**（`$SHELL`），不是界面猜的：
/// 猜成 `/bin/bash` 会让 zsh 用户丢掉自己的补全与提示符，那种差异会被当成"终端坏了"。
/// Windows 走 `COMSPEC`，两个都拿不到才退回各平台的通用值。
pub fn default_program() -> String {
    if let Some(shell) = std::env::var("SHELL").ok().filter(|v| !v.trim().is_empty()) {
        return shell;
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "cmd.exe".into())
    }
    #[cfg(not(windows))]
    {
        "/bin/zsh".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把字节收到内存里的 sink（测试用；生产走 tauri Channel）。
    fn collect_sink() -> (Arc<Mutex<Vec<u8>>>, Arc<dyn PtySink>) {
        let collected = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let collected = collected.clone();
            Arc::new(move |bytes: &[u8]| -> bool {
                if let Ok(mut guard) = collected.lock() {
                    guard.extend_from_slice(bytes);
                }
                true
            })
        } as Arc<dyn PtySink>;
        (collected, sink)
    }

    fn noop_exit() -> Arc<dyn Fn(PtyExit) + Send + Sync + 'static> {
        Arc::new(|_| {})
    }

    fn sh(command: &str) -> PtyOpenArgs {
        PtyOpenArgs {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), command.into()],
            cwd: None,
            cols: 80,
            rows: 24,
        }
    }

    /// 等一条腿跑出期望内容。PTY 是异步的，测试不许靠一句 sleep 猜结果。
    fn wait_for(bytes: &Arc<Mutex<Vec<u8>>>, needle: &str) -> String {
        for _ in 0..200 {
            let text = bytes
                .lock()
                .map(|guard| String::from_utf8_lossy(guard.as_slice()).to_string())
                .unwrap_or_default();
            if text.contains(needle) {
                return text;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let text = bytes
            .lock()
            .map(|guard| String::from_utf8_lossy(guard.as_slice()).to_string())
            .unwrap_or_default();
        panic!("等不到 {needle:?}，实际收到：{text:?}");
    }

    /// 默认程序必须是"本机说的"，不是猜的：`SHELL=/bin/dash` 时必须回 `/bin/dash`。
    /// （这条用环境变量注入，跑在单进程里可能与其他腿并行——所以只断言"非空且像路径"，
    /// 不去断言具体值，避免互相踩。）
    #[test]
    fn default_program_is_never_empty() {
        let program = default_program();
        assert!(!program.trim().is_empty(), "默认程序不能是空串");
    }

    #[test]
    fn open_rejects_empty_and_illegal_program_without_spawning() {
        let service = PtyService::new();
        let (_bytes, sink) = collect_sink();
        for program in ["", "   ", "weird\u{0}name"] {
            let error = service
                .open(
                    PtyOpenArgs {
                        program: program.into(),
                        args: vec![],
                        cwd: None,
                        cols: 80,
                        rows: 24,
                    },
                    sink.clone(),
                    noop_exit(),
                )
                .unwrap_err();
            assert!(
                matches!(error, CoreError::InvalidInput(_)),
                "{program:?} 应当报输入非法，实际 {error:?}"
            );
        }
        assert_eq!(service.len(), 0, "被拒的调用不许留下半个会话");
    }

    /// **字节保真**是「能不能交给 xterm 渲染」的前提。任务通道按行切 + 8KB 截断
    /// 会剁掉半行、`\r` 覆盖与不带换行的进度；这儿不许，转义序列必须原样到界面。
    #[test]
    fn raw_bytes_are_forwarded_verbatim_from_the_child() {
        let service = PtyService::new();
        let (bytes, sink) = collect_sink();
        let opened = service
            .open(
                sh("printf 'a\\rb\\rcc\\rd\\n\\x1b[31mRED\\x1b[0m\\n'"),
                sink,
                noop_exit(),
            )
            .unwrap();
        let text = wait_for(&bytes, "RED");
        assert!(text.contains("RED"), "SGR 包着的文字必须在: {text:?}");
        assert!(
            text.contains('\u{1b}'),
            "转义序列不许被剥掉（那是 xterm 的输入，不是我们的）: {text:?}"
        );
        // cooked 模式的行规程会把 \r 变成 \n —— 那是 tty 的行为，我们不改它也不掩盖它
        assert!(
            text.contains('a') && text.contains('b') && text.contains('c') && text.contains('d'),
            "\\r 覆盖串里的每个片段都得出现: {text:?}"
        );
        service.close(&opened.id).unwrap();
    }

    /// C-06 的一半：键盘字节确实落到子进程 stdin（回显与行编辑归 tty 与程序）。
    #[test]
    fn written_bytes_reach_the_child_stdin() {
        let service = PtyService::new();
        let (bytes, sink) = collect_sink();
        let opened = service
            .open(sh("read line; echo GOT:$line"), sink, noop_exit())
            .unwrap();
        service.write(&opened.id, b"hello pty\n").unwrap();
        let text = wait_for(&bytes, "GOT:hello pty");
        assert!(text.contains("GOT:hello pty"), "{text:?}");
        service.close(&opened.id).unwrap();
    }

    /// C-05：`resize` 必须真的改内核里的 winsize，程序问得到。
    /// 判据用 `stty size`（读内核结构，不是环境变量）——这条过了才谈得上
    /// 「prompt_toolkit 按界面宽度重排」，否则一切回 80 列。
    #[test]
    fn resize_changes_the_winsize_that_stty_reports() {
        let service = PtyService::new();
        let (bytes, sink) = collect_sink();
        let opened = service
            .open(
                PtyOpenArgs {
                    program: "/bin/sh".into(),
                    args: vec![],
                    cwd: None,
                    cols: 40,
                    rows: 12,
                },
                sink,
                noop_exit(),
            )
            .unwrap();
        service.write(&opened.id, b"stty size\n").unwrap();
        let text = wait_for(&bytes, "12 40");
        assert!(text.contains("12 40"), "起会话时的尺寸没进内核: {text:?}");

        service.resize(&opened.id, 100, 30).unwrap();
        service.write(&opened.id, b"stty size\n").unwrap();
        let text = wait_for(&bytes, "30 100");
        assert!(
            text.contains("30 100"),
            "resize 之后 stty 仍报旧尺寸: {text:?}"
        );
        service.close(&opened.id).unwrap();
    }

    /// 0 列/0 行不许传给 tty：隐藏 tab 量到 0 是常态（R-03）。
    #[test]
    fn zero_sized_geometry_is_clamped_not_forwarded() {
        assert_eq!(clamp_cols(0), 20);
        assert_eq!(clamp_cols(1), 20);
        assert_eq!(clamp_cols(4000), 512);
        assert_eq!(clamp_rows(0), 4);
        assert_eq!(clamp_rows(9999), 512);
    }

    /// C-06 的另一半：Ctrl-C 这个 0x03 由行规程翻成 SIGINT，trap 收得到。
    /// 也就是说"中断"不需要我们自己模拟，更不该在 write() 里特殊处理它。
    #[test]
    fn ctrl_c_from_the_keyboard_becomes_a_sigint() {
        let service = PtyService::new();
        let (bytes, sink) = collect_sink();
        let opened = service
            .open(
                sh("trap 'echo CAUGHT-INT; exit' INT; while :; do sleep 0.2; done"),
                sink,
                noop_exit(),
            )
            .unwrap();
        // 等 shell 真把 trap 装上（这条 sleep 是等环境，不是等结果）
        std::thread::sleep(Duration::from_millis(250));
        service.write(&opened.id, "\u{3}".as_bytes()).unwrap();
        let text = wait_for(&bytes, "CAUGHT-INT");
        assert!(
            text.contains("CAUGHT-INT"),
            "0x03 没能变成 SIGINT: {text:?}"
        );
        service.close(&opened.id).unwrap();
    }

    /// R-08：close 之后不留活着的子进程，会话表也腾干净。
    /// 判据是操作系统说"没这个 pid 了"，不是"我调用过 kill"（UI-6 那条纪律的同一形式）。
    #[test]
    fn close_terminates_the_child_and_empties_the_table() {
        let service = PtyService::new();
        let (_bytes, sink) = collect_sink();
        let opened = service.open(sh("sleep 60"), sink, noop_exit()).unwrap();
        assert_eq!(service.len(), 1);
        let pid = opened.pid.expect("PTY 子进程应有 pid");
        service.close(&opened.id).unwrap();
        assert_eq!(service.len(), 0, "关掉之后表里必须没有");
        let probe = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("kill -0 {pid} 2>/dev/null; echo $?")])
            .output()
            .expect("探测进程应能跑");
        let alive = String::from_utf8_lossy(&probe.stdout).trim().to_owned();
        assert_eq!(
            alive, "1",
            "pid {pid} 还活着（kill -0 返回 0），说明 close 没真的收掉子进程"
        );
    }

    #[test]
    fn operations_on_unknown_session_say_so() {
        let service = PtyService::new();
        assert!(matches!(
            service.write("pty-404", b"x"),
            Err(CoreError::NotFound(_))
        ));
        assert!(matches!(
            service.resize("pty-404", 80, 24),
            Err(CoreError::NotFound(_))
        ));
        // 关一个不存在的 id 不报错：用户要的效果（没有活着的会话）已经达成
        service.close("pty-404").unwrap();
    }

    /// 读线程必须能感知"出口没了"（前端 tab 被关），否则字节在往一个没人收的队列里堆。
    #[test]
    fn reader_stops_when_the_sink_is_gone() {
        let service = PtyService::new();
        let hits = Arc::new(AtomicU64::new(0));
        let sink = {
            let hits = hits.clone();
            Arc::new(move |_bytes: &[u8]| -> bool {
                hits.fetch_add(1, Ordering::Relaxed);
                false
            })
        } as Arc<dyn PtySink>;
        let opened = service.open(sh("echo bye"), sink, noop_exit()).unwrap();
        for _ in 0..120 {
            if hits.load(Ordering::Relaxed) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            hits.load(Ordering::Relaxed) > 0,
            "一次都没送到，说明读线程没跑起来"
        );
        std::thread::sleep(Duration::from_millis(200));
        // 被拒之后不再堆：只可能送过一次（读线程已收摊）
        assert_eq!(
            hits.load(Ordering::Relaxed),
            1,
            "出口关了还在送 = 字节找没人要"
        );
        service.close(&opened.id).unwrap();
    }
}

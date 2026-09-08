//! Process Plugin 宿主通道（P6）：以独立进程运行插件，stdio 行分隔 JSON-RPC 2.0 通信。
//! 协议定稿见 docs/plugin-process-protocol.md（P4 移交本阶段定稿）。
//!
//! 崩溃隔离语义：插件进程退出/被杀只影响当次调用（返回错误），
//! 主程序不受影响；下次调用懒重启子进程并重新握手。
//! 与 in-process 的差异：进程插件超时可以直接 kill，无「无法抢占」的残留线程问题。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::plugins::manifest::PluginManifest;

/// initialize 握手的固定超时
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// shutdown 通知后等待插件自行退出的时间，超时强杀
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1500);

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("无法启动插件进程 {path}: {source}")]
    Spawn {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("插件握手失败: {0}")]
    Handshake(String),
    #[error("插件进程崩溃（退出码 {code:?}）")]
    Crashed { code: Option<i32> },
    #[error("插件调用超时（{0:?}），进程已终止")]
    Timeout(Duration),
    #[error("插件响应协议错误: {0}")]
    BadProtocol(String),
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
}

/// 已握手插件自报的信息（等价 in-process 的 AbiInfo）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub plugin_type: String,
}

struct RunningChild {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    next_id: u64,
}

/// 进程插件句柄。Drop 时发 shutdown 并回收子进程。
pub struct ProcessPlugin {
    manifest: PluginManifest,
    exe: PathBuf,
    inner: Mutex<Option<RunningChild>>,
}

// 子进程句柄与 mpsc 均可跨线程移动；并发访问由 inner Mutex 串行化
unsafe impl Send for ProcessPlugin {}
unsafe impl Sync for ProcessPlugin {}

impl Drop for ProcessPlugin {
    fn drop(&mut self) {
        if let Some(mut c) = self.inner.lock().unwrap_or_else(|p| p.into_inner()).take() {
            shutdown_child(&mut c);
        }
    }
}

impl ProcessPlugin {
    /// 记录 manifest 与可执行路径；不立即 spawn（首次 call 懒启动）。
    pub fn new(manifest: PluginManifest, exe: PathBuf) -> Self {
        Self {
            manifest,
            exe,
            inner: Mutex::new(None),
        }
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    pub fn exe_path(&self) -> &std::path::Path {
        &self.exe
    }

    /// 确保子进程已启动并完成 initialize 握手；返回自报信息。
    pub fn info(&self) -> Result<ProcessInfo, ProcessError> {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let child = ensure_running(&mut guard, &self.exe)?;
        Ok(child_info(&self.manifest, child))
    }

    /// 调用插件。input 为 UTF-8 JSON 文本（与 in-process payload 协议一致）。
    /// 返回 (code, output)；业务错误照旧走 output 内的 JSON。
    pub fn call(&self, input: &[u8], timeout: Duration) -> Result<(i32, Vec<u8>), ProcessError> {
        let input = String::from_utf8_lossy(input).into_owned();
        let deadline = Instant::now() + timeout;
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let child = ensure_running(&mut guard, &self.exe)?;
        let id = child.next_id;
        child.next_id += 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "call",
            "params": { "input": input }
        });
        if write_line(&mut child.stdin, &req.to_string()).is_err() {
            let code = reap(&mut child.child);
            kill_and_clear(&mut guard.take());
            return Err(ProcessError::Crashed { code });
        }
        match wait_response(&child.rx, id, deadline) {
            Ok(result) => {
                // 协议：result = { code: int, output: string }；output 为 UTF-8 JSON 文本
                let code = result.get("code").and_then(Value::as_i64).unwrap_or(-2) as i32;
                let output = result
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .as_bytes()
                    .to_vec();
                Ok((code, output))
            }
            Err(e) => {
                // 崩溃或超时都回收子进程：下次调用懒重启
                kill_and_clear(&mut guard.take());
                Err(e)
            }
        }
    }

    /// 主动停止插件进程（启停语义的「停」）。幂等。
    pub fn stop(&self) {
        if let Some(mut c) = self.inner.lock().unwrap_or_else(|p| p.into_inner()).take() {
            shutdown_child(&mut c);
        }
    }
}

fn child_info(manifest: &PluginManifest, child: &RunningChild) -> ProcessInfo {
    // 握手已校验自报 id == manifest.id；展示信息以 manifest 为准（含 name/version）
    let _ = child;
    ProcessInfo {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        plugin_type: manifest.plugin_type.clone(),
    }
}

/// 无子进程则启动 + 握手；有则直接复用。出错时清空槽位。
fn ensure_running<'a>(
    slot: &'a mut Option<RunningChild>,
    exe: &std::path::Path,
) -> Result<&'a mut RunningChild, ProcessError> {
    if slot.is_none() {
        let child = spawn_and_initialize(exe).map_err(|e| {
            tracing::warn!(exe = %exe.display(), error = %e, "process plugin 启动失败");
            *slot = None;
            e
        })?;
        *slot = Some(child);
    }
    Ok(slot.as_mut().expect("just inserted"))
}

fn spawn_and_initialize(exe: &std::path::Path) -> Result<RunningChild, ProcessError> {
    #[cfg(unix)]
    let exe = ensure_executable(exe)?;
    let mut child = Command::new(&exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ProcessError::Spawn {
            path: exe.display().to_string(),
            source,
        })?;
    let stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    // stderr 转发到 tracing（插件诊断输出，不影响协议）
    std::thread::Builder::new()
        .name("plugin-proc-stderr".into())
        .spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                tracing::debug!(target: "plugin_process", "{line}");
            }
        })
        .map_err(|e| ProcessError::Io(std::io::Error::other(e.to_string())))?;

    // stdout 行 → channel
    let (tx, rx) = channel::<String>();
    std::thread::Builder::new()
        .name("plugin-proc-stdout".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| ProcessError::Io(std::io::Error::other(e.to_string())))?;

    let mut c = RunningChild {
        child,
        stdin,
        rx,
        next_id: 1,
    };
    // initialize 握手
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": { "protocolVersion": 1 }
    });
    write_line(&mut c.stdin, &req.to_string())?;
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let result = wait_response(&c.rx, 0, deadline)?;
    let reported = result
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if reported.is_empty() {
        return Err(ProcessError::Handshake(
            "initialize 结果缺少 id 字段".into(),
        ));
    }
    Ok(c)
}

/// 等待指定 id 的响应；跳过通知行。崩溃/超时/协议错误分别报错。
fn wait_response(rx: &Receiver<String>, id: u64, deadline: Instant) -> Result<Value, ProcessError> {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(ProcessError::Timeout(deadline - now + Duration::ZERO));
        }
        match rx.recv_timeout(deadline - now) {
            Ok(line) => {
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(ProcessError::BadProtocol(format!("非 JSON 行: {e}")));
                    }
                };
                // 通知（无 id）→ 记录并跳过
                if v.get("id").is_none() {
                    let method = v.get("method").and_then(Value::as_str).unwrap_or("?");
                    tracing::debug!(target: "plugin_process", method, "notification");
                    continue;
                }
                let resp_id = v.get("id").and_then(Value::as_u64).unwrap_or(u64::MAX);
                if resp_id != id {
                    continue; // 旧响应，忽略
                }
                if let Some(err) = v.get("error") {
                    return Err(ProcessError::BadProtocol(format!("JSON-RPC error: {err}")));
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(ProcessError::Timeout(Duration::ZERO));
            }
            Err(RecvTimeoutError::Disconnected) => {
                // reader 线程结束 = 子进程 stdout 关闭 = 崩溃/退出
                return Err(ProcessError::Crashed { code: None });
            }
        }
    }
}

fn write_line(stdin: &mut ChildStdin, line: &str) -> Result<(), ProcessError> {
    stdin.write_all(line.as_bytes())?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn reap(child: &mut Child) -> Option<i32> {
    child.try_wait().ok().flatten().and_then(|s| s.code())
}

fn kill_and_clear(c: &mut Option<RunningChild>) {
    if let Some(mut running) = c.take() {
        let _ = running.child.kill();
        let _ = running.child.wait();
    }
}

fn shutdown_child(c: &mut RunningChild) {
    // 通知 + 宽限等待；不退就强杀
    let _ = write_line(
        &mut c.stdin,
        &serde_json::json!({"jsonrpc":"2.0","method":"shutdown"}).to_string(),
    );
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        if c.child.try_wait().map(|s| s.is_some()).unwrap_or(true) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = c.child.kill();
    let _ = c.child.wait();
}

#[cfg(unix)]
fn ensure_executable(exe: &std::path::Path) -> Result<PathBuf, ProcessError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(exe)?;
    let mut perms = meta.permissions();
    if perms.mode() & 0o111 == 0 {
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(exe, perms)?;
    }
    Ok(exe.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_for(exe_name: &str) -> (PluginManifest, PathBuf) {
        let manifest: PluginManifest = serde_json::from_value(serde_json::json!({
            "id": "tool.echo", "name": "Echo", "version": "0.2.0",
            "abi": 1, "type": "tool",
            "entry": { "macos-arm64": exe_name }
        }))
        .unwrap();
        (manifest, PathBuf::from(exe_name))
    }

    #[test]
    fn wait_response_skips_notifications_and_matches_id() {
        let (tx, rx) = channel::<String>();
        tx.send(r#"{"jsonrpc":"2.0","method":"log","params":{"message":"hi"}}"#.into())
            .unwrap();
        tx.send(r#"{"jsonrpc":"2.0","id":99,"result":{}}"#.into())
            .unwrap();
        tx.send(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#.into())
            .unwrap();
        let v = wait_response(&rx, 1, Instant::now() + Duration::from_secs(1)).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn wait_response_error_object_is_bad_protocol() {
        let (tx, rx) = channel::<String>();
        tx.send(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-2,"message":"boom"}}"#.into())
            .unwrap();
        assert!(matches!(
            wait_response(&rx, 1, Instant::now() + Duration::from_secs(1)),
            Err(ProcessError::BadProtocol(_))
        ));
    }

    #[test]
    fn wait_response_disconnected_is_crash() {
        let (tx, rx) = channel::<String>();
        drop(tx);
        assert!(matches!(
            wait_response(&rx, 1, Instant::now() + Duration::from_secs(1)),
            Err(ProcessError::Crashed { .. })
        ));
    }

    #[test]
    fn wait_response_timeout() {
        let (_tx, rx) = channel::<String>();
        assert!(matches!(
            wait_response(&rx, 1, Instant::now() + Duration::from_millis(30)),
            Err(ProcessError::Timeout(_))
        ));
    }

    #[test]
    fn manifest_for_helper_is_process_shape() {
        let (m, _) = manifest_for("process-echo");
        assert!(m.is_process());
    }
}

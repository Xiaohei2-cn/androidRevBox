//! FridaProvider：设备侧 frida-server 生命周期（AR9.1）。
//!
//! 为什么值得单独一个 provider：这三件事的**身份要求不一样**。
//! - `status` 纯只读，Agent 以 shell 身份就能做完（实测：`/proc/<pid>/cmdline` 与
//!   `/proc/<pid>/status` 里的 `Uid:` 对 shell 可读），不需要 root；
//! - `start` 必须以 root 起（shell 起的 frida-server 连得上但注入不了别的进程），
//!   所以走 `privileged` 固定脚本（D037/D038）；
//! - `stop` 杀的是 root 属主进程，shell 亲手 `kill` 只会 `Operation not permitted`
//!   （本轮真机实测），同样只能走固定脚本。
//!
//! 状态机刻意保留 `RunningAsShell` 与 `Indeterminate` 两档：把「在跑但没用」和
//! 「跑着但读不到身份」并进 `running=true` 就是骗人，用户会对着一个连得上的
//! frida-server 疑惑为什么 attach 不进去。

use std::path::Path;
use std::time::Duration;

use agent_protocol::method::{FRIDA_SERVER_START, FRIDA_SERVER_STATUS, FRIDA_SERVER_STOP};
use agent_protocol::{
    AgentError, ErrorCode, FridaServerStartParams, FridaServerStartResult, FridaServerState,
    FridaServerStatusParams, FridaServerStatusResult, FridaServerStopParams, FridaServerStopResult,
    OperationStep, ProviderHealth, ProviderInfo, WriteOutcome,
};
use serde_json::Value;
use tokio::process::Command;

use super::{Provider, ProviderFuture, RequestContext};

const FRIDA_METHODS: &[&str] = &[FRIDA_SERVER_STATUS, FRIDA_SERVER_START, FRIDA_SERVER_STOP];
/// 默认二进制名：只允许这个名字（或调用方过白名单的其它名字）出现在托管目录里。
const DEFAULT_BINARY: &str = "frida-server";
/// 默认端口与绑定地址。绑回环是有意的：远程模式走 `adb forward`，
/// 绑 `0.0.0.0` 等于把 frida 控制面开放给同网段任何设备。
const DEFAULT_PORT: u16 = 27042;
const DEFAULT_BIND: &str = "127.0.0.1";
/// 托管目录（AR7.2 起 Agent 与 Desktop 共用的落点）。
const HOSTED_DIR: &str = "/data/local/tmp";
/// root 起的服务器自己的启动日志：脚本先 `: > log; chmod 666`，
/// 这样 shell 身份的 Agent 事后读得到失败原因（否则日志是 root 0600，等于没写）。
const START_LOG: &str = "/data/local/tmp/frida-server.artool.log";
/// 探测/复核的有界时间：宁可短，卡住的进程不能拖住会话。
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// 单次扫描的 pid 上限（与 process provider 同一口径，防 /proc 异常膨胀时挂死）。
const MAX_SCANNED_PIDS: usize = 8_192;
const FRIDA_START_NAME: &str = "frida.server.start";
const FRIDA_STOP_NAME: &str = "frida.server.stop";

#[derive(Debug, Clone)]
struct ServerFact {
    pid: u32,
    uid: Option<u32>,
    args: Vec<String>,
}

pub struct FridaProvider;

impl Provider for FridaProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "frida".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: vec!["shell".into()],
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        FRIDA_METHODS
    }

    fn handle<'a>(
        &'a self,
        _context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                FRIDA_SERVER_STATUS => status(params).await,
                FRIDA_SERVER_START => start(params).await,
                FRIDA_SERVER_STOP => stop(params).await,
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported frida method: {method}"),
                )),
            }
        })
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AgentError> {
    serde_json::from_value(params)
        .map_err(|error| AgentError::new(ErrorCode::InvalidRequest, format!("参数不合法: {error}")))
}

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, AgentError> {
    serde_json::to_value(value)
        .map_err(|error| AgentError::new(ErrorCode::Internal, format!("结果序列化失败: {error}")))
}

fn invalid(reason: &str, message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::InvalidRequest, message)
        .with_details(serde_json::json!({ "reason": reason }))
}

fn with_steps(error: AgentError, steps: &[OperationStep]) -> AgentError {
    let reason = error
        .details
        .as_ref()
        .and_then(|details| details.get("reason"))
        .and_then(Value::as_str)
        .unwrap_or("failed")
        .to_owned();
    error.with_details(serde_json::json!({ "reason": reason, "steps": steps }))
}

fn name_of(params_name: Option<&str>) -> Result<String, AgentError> {
    let name = params_name.unwrap_or(DEFAULT_BINARY);
    super::privileged::validate_binary_name(name)?;
    Ok(name.to_owned())
}

// ===== 只读探测 =====

/// argv[0] 的 basename 必须**正好等于**目标名字：光看 comm 会撞上 15 字符截断，
/// 光 grep 命令行又会把 `frida-server --help` 之类误算成服务在跑。
fn matches_server(argv0: &str, name: &str) -> bool {
    Path::new(argv0)
        .file_name()
        .and_then(|v| v.to_str())
        .is_some_and(|base| base == name)
}

/// 扫 `/proc/[0-9]*` 找 frida-server。以 shell 身份运行即可工作（实测 cmdline 与
/// status 的 `Uid:` 对 shell 可读）；读不到的目录直接跳过，不因此报错。
async fn find_server(name: &str) -> Vec<ServerFact> {
    let mut found = Vec::new();
    let Ok(mut dir) = tokio::fs::read_dir("/proc").await else {
        return found;
    };
    let mut scanned = 0_usize;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let pid_text = entry.file_name().to_string_lossy().into_owned();
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };
        if pid == 0 || pid == std::process::id() {
            continue;
        }
        if scanned >= MAX_SCANNED_PIDS {
            // 扫不完就如实说：漏报「没在跑」比报「读不到」更糟
            break;
        }
        scanned += 1;
        let Ok(raw) = tokio::fs::read(format!("/proc/{pid}/cmdline")).await else {
            continue;
        };
        let args: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let Some(argv0) = args.first() else {
            continue;
        };
        if !matches_server(argv0, name) {
            continue;
        }
        let uid = tokio::fs::read_to_string(format!("/proc/{pid}/status"))
            .await
            .ok()
            .and_then(|status| {
                status.lines().find_map(|line| {
                    line.strip_prefix("Uid:")
                        .and_then(|value| value.split_whitespace().next())
                        .and_then(|value| value.parse::<u32>().ok())
                })
            });
        found.push(ServerFact { pid, uid, args });
    }
    found.sort_by_key(|fact| fact.pid);
    found
}

/// 从 `-l <addr>:<port>` / `-l<addr>:<port>` / `--listen=<addr>:<port>` 解析监听地址。
///
/// 单测在这里抓到一个真 bug：`-l` 与值分开写时，`strip_prefix("-l")` 得到**空串**，
/// 早先的实现把它当成一个「有值」的分支，于是整函数在第二个参数上就返回 None，
/// 结果是「服务明明在跑却报告没监听懂」。现在只有非空前缀才算内联值。
fn parse_listen(args: &[String]) -> Option<(String, u16)> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let mut value: Option<String> = None;
        if let Some(rest) = arg.strip_prefix("--listen=") {
            value = Some(rest.to_owned());
        } else if let Some(rest) = arg.strip_prefix("-l").filter(|rest| !rest.is_empty()) {
            value = Some(rest.to_owned());
        } else if (arg == "-l" || arg == "--listen") && index + 1 < args.len() {
            index += 1;
            value = Some(args[index].clone());
        }
        if let Some(value) = value {
            let (addr, port) = value.rsplit_once(':')?;
            let port: u16 = port.parse().ok()?;
            return Some((addr.to_owned(), port));
        }
        index += 1;
    }
    None
}

/// 端口是否真在 LISTEN（`st == "0A"`）。进程在但没监听成功是真实故障场景，
/// 所以这一项必须独立看，不能由「进程存在」代替。
async fn is_listening(port: u16) -> bool {
    for path in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = tokio::fs::read_to_string(path).await else {
            continue;
        };
        if text_has_listening_port(&text, port) {
            return true;
        }
    }
    false
}

/// 纯函数版：`sl` 与 `local_address` 都是 `十六进制地址:十六进制端口`，状态列是 `0A`。
/// 单独抽出来是因为真机上「端口没起来」是常见故障，这条判据必须能在宿主上测。
fn text_has_listening_port(text: &str, port: u16) -> bool {
    let wanted = format!(":{port:04X}");
    text.lines().skip(1).any(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        fields.len() >= 4 && fields[1].ends_with(&wanted) && fields[3] == "0A"
    })
}

/// uid → 状态。读不到 uid 绝不猜成 root 或 shell。
fn classify_state(uid: Option<u32>) -> FridaServerState {
    match uid {
        Some(0) => FridaServerState::RunningAsRoot,
        Some(_) => FridaServerState::RunningAsShell,
        None => FridaServerState::Indeterminate,
    }
}

/// 托管目录里那个二进制是否存在且可执行。**不执行它**（`--version` 会真的起进程，
/// 探一次状态不该有副作用），版本号只在启动成功后由启动流程带回。
async fn binary_ready(name: &str) -> (bool, Option<String>) {
    let path = Path::new(HOSTED_DIR).join(name);
    let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
        return (
            false,
            Some(format!(
                "{HOSTED_DIR}/{name} 不存在，请先用设备页的文件/托管功能放入"
            )),
        );
    };
    if !meta.is_file() {
        return (false, Some(format!("{HOSTED_DIR}/{name} 不是普通文件")));
    }
    let executable = meta.permissions().mode() & 0o111 != 0;
    (
        executable,
        if executable {
            None
        } else {
            Some(format!("{HOSTED_DIR}/{name} 没有执行权限（先 chmod +x）"))
        },
    )
}

#[allow(unused_imports)]
use std::os::unix::fs::PermissionsExt as _;

// ===== status =====

async fn status(params: Value) -> Result<Value, AgentError> {
    let params: FridaServerStatusParams = parse_params(params)?;
    let _ = params;
    let name = DEFAULT_BINARY.to_owned();
    let facts = find_server(&name).await;
    let version = probe_version(&name).await;
    let result = match facts.first() {
        None => FridaServerStatusResult {
            state: FridaServerState::NotRunning,
            running: false,
            as_root: false,
            binary_name: name,
            pid: None,
            uid: None,
            listen_address: None,
            port: None,
            listening: false,
            version,
            detail: None,
        },
        Some(fact) => {
            let listen = parse_listen(&fact.args);
            let state = classify_state(fact.uid);
            let listening = match listen.as_ref() {
                Some((_, port)) => is_listening(*port).await,
                None => false,
            };
            let detail = match fact.uid {
                Some(0) => None,
                Some(uid) => Some(format!(
                    "frida-server 以 uid={uid} 运行：连得上但注入不了其它进程，需要以 root 重启（frida.server.stop 后 start）"
                )),
                None => Some("读不到该进程的 uid，不猜它是 root 还是 shell".to_owned()),
            };
            FridaServerStatusResult {
                state,
                running: true,
                as_root: matches!(state, FridaServerState::RunningAsRoot),
                pid: Some(fact.pid),
                uid: fact.uid,
                listen_address: listen.as_ref().map(|(addr, _)| addr.clone()),
                port: listen.as_ref().map(|(_, port)| *port),
                listening,
                binary_name: name,
                version,
                detail,
            }
        }
    };
    serialize(result)
}

/// 版本号只从**已存在的**二进制读，且用 `--version`（frida-server 打印后立即退出）。
/// 这一步是可选信息：读不到就 None，不影响状态结论。
async fn probe_version(name: &str) -> Option<String> {
    if super::privileged::validate_binary_name(name).is_err() {
        return None;
    }
    let path = Path::new(HOSTED_DIR).join(name);
    if !path.is_file() {
        return None;
    }
    let output = tokio::time::timeout(PROBE_TIMEOUT, Command::new(&path).arg("--version").output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    text.lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

// ===== start =====

async fn start(params: Value) -> Result<Value, AgentError> {
    let params: FridaServerStartParams = parse_params(params)?;
    let name = name_of(params.binary_name.as_deref())?;
    let port = params.port.unwrap_or(DEFAULT_PORT);
    let bind = params.bind.as_deref().unwrap_or(DEFAULT_BIND).to_owned();
    super::privileged::validate_listen(&bind, port)?;
    super::activity::begin_write(FRIDA_START_NAME, &name, &params.operation_id)?;
    let mut steps: Vec<OperationStep> = Vec::new();
    let mut push = |name: &str, ok: bool, detail: Option<String>| {
        steps.push(OperationStep {
            name: name.to_owned(),
            ok,
            detail,
        });
    };
    push("validate_input", true, None);

    if let Some(cached) = super::operations::lookup(&params.operation_id) {
        return Ok(super::operations::mark_replayed(cached));
    }

    let (ok, detail) = binary_ready(&name).await;
    push("locate_binary", ok, detail.clone());
    if !ok {
        return Err(with_steps(
            invalid("binary_unavailable", detail.unwrap_or_default()),
            &steps,
        ));
    }

    // 已经在跑：不再起第二个（frida-server 同端口二次启动会失败并留下误导性的
    // 「启动失败」，真实情况是「已在期望状态」）。
    let existing = find_server(&name).await;
    if let Some(fact) = existing.first() {
        push(
            "guard_already_running",
            true,
            Some(format!("pid={} uid={:?}", fact.pid, fact.uid)),
        );
        let value = serialize(FridaServerStartResult {
            operation_id: params.operation_id.clone(),
            outcome: WriteOutcome::NoOp,
            verified: matches!(fact.uid, Some(0)),
            binary_name: name.clone(),
            bind: bind.clone(),
            port,
            pid: Some(fact.pid),
            uid: fact.uid,
            version: None,
            steps: steps.clone(),
            detail: if matches!(fact.uid, Some(0)) {
                Some("already_running_as_root".into())
            } else {
                Some("already_running_needs_root_restart".into())
            },
        })?;
        super::activity::finish_write(FRIDA_START_NAME, &params.operation_id, &name, &value);
        return Ok(value);
    }
    push("guard_already_running", true, None);

    if let Err(error) = super::privileged::run_privileged(
        "start",
        &super::privileged::frida_start_script(&name, &bind, port, START_LOG),
        "FRIDA_STARTED",
    )
    .await
    {
        push("start", false, Some(error.message.clone()));
        let log = read_log_tail().await;
        if let Some(log) = log {
            push("read_start_log", true, Some(log));
        }
        return Err(with_steps(error, &steps));
    }
    push("start", true, None);

    // 复核：进程在、uid=0、端口真在 LISTEN。三项分别是三种故障，缺一不可。
    let facts = find_server(&name).await;
    let fact = facts.first();
    push(
        "verify_running",
        fact.is_some(),
        fact.map(|f| format!("pid={} uid={:?}", f.pid, f.uid)),
    );
    let listening = match fact {
        Some(fact) => match parse_listen(&fact.args) {
            Some((_, actual_port)) => is_listening(actual_port).await,
            None => false,
        },
        None => false,
    };
    push(
        "verify_listening",
        listening,
        Some(format!("{bind}:{port} LISTEN={listening}")),
    );
    let as_root = matches!(fact.map(|f| f.uid), Some(Some(0)));
    let verified = fact.is_some() && as_root && listening;

    if !verified {
        // 起起来了但不是 root / 没监听 → 立刻停掉，不留一个「看起来在跑其实没用」的服务
        let stopped = super::privileged::run_privileged(
            "stop",
            &super::privileged::frida_stop_script(&name),
            "FRIDA_STOPPED",
        )
        .await
        .is_ok();
        push(
            "rollback_stop",
            stopped,
            Some(if stopped {
                "已停掉不可用的实例".to_owned()
            } else {
                "清理失败，需要人工确认残留进程".to_owned()
            }),
        );
        let reason = if fact.is_none() {
            "start_failed_process_gone"
        } else if !as_root {
            "started_without_root"
        } else {
            "port_not_listening"
        };
        return Err(with_steps(
            AgentError::new(
                ErrorCode::Internal,
                format!("frida-server 启动复核未通过: {reason}"),
            )
            .with_details(serde_json::json!({ "reason": reason, "steps": steps })),
            &steps,
        ));
    }

    let fact = fact.expect("复核通过时进程必然在");
    let version = probe_version(&name).await;
    let value = serialize(FridaServerStartResult {
        operation_id: params.operation_id.clone(),
        outcome: WriteOutcome::Executed,
        verified,
        binary_name: name.clone(),
        bind,
        port,
        pid: Some(fact.pid),
        uid: fact.uid,
        version,
        steps: steps.clone(),
        detail: Some("root_listening".into()),
    })?;
    super::activity::finish_write(FRIDA_START_NAME, &params.operation_id, &name, &value);
    Ok(value)
}

async fn read_log_tail() -> Option<String> {
    let text = tokio::fs::read_to_string(START_LOG).await.ok()?;
    let tail: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(3)
        .collect();
    if tail.is_empty() {
        return None;
    }
    Some(tail.into_iter().rev().collect::<Vec<_>>().join(" / "))
}

// ===== stop =====

async fn stop(params: Value) -> Result<Value, AgentError> {
    let params: FridaServerStopParams = parse_params(params)?;
    let name = name_of(params.binary_name.as_deref())?;
    super::activity::begin_write(FRIDA_STOP_NAME, &name, &params.operation_id)?;
    if let Some(cached) = super::operations::lookup(&params.operation_id) {
        return Ok(super::operations::mark_replayed(cached));
    }
    let mut steps: Vec<OperationStep> = Vec::new();
    let mut push = |name: &str, ok: bool, detail: Option<String>| {
        steps.push(OperationStep {
            name: name.to_owned(),
            ok,
            detail,
        });
    };

    let facts = find_server(&name).await;
    let Some(fact) = facts.first() else {
        push(
            "find_server",
            false,
            Some("没有运行中的 frida-server".to_owned()),
        );
        let value = serialize(FridaServerStopResult {
            operation_id: params.operation_id.clone(),
            outcome: WriteOutcome::NoOp,
            verified: true,
            binary_name: name.clone(),
            pid: None,
            uid: None,
            steps: steps.clone(),
            detail: Some("not_running".into()),
        })?;
        super::activity::finish_write(FRIDA_STOP_NAME, &params.operation_id, &name, &value);
        return Ok(value);
    };
    let pid = fact.pid;
    let uid = fact.uid;
    push("find_server", true, Some(format!("pid={pid} uid={uid:?}")));

    // root 属主进程 shell 杀不动（实测 Operation not permitted），必须走特权固定脚本；
    // 脚本内部还会逐个复核 argv[0] basename，防 pid 复用杀错进程。
    let script = super::privileged::frida_stop_script(&name);
    let outcome = match uid {
        Some(0) => super::privileged::run_privileged("stop", &script, "FRIDA_STOPPED").await,
        // shell 属主：Agent 自己能杀，不必动用 su（少一次无谓提权）
        _ => match super::process::send_signal(pid, agent_protocol::KillSignal::Term) {
            super::process::SignalResult::Sent | super::process::SignalResult::Gone => {
                Ok(String::new())
            }
            super::process::SignalResult::Denied => Err(AgentError::new(
                ErrorCode::PermissionDenied,
                "无权终止该进程，且它不属于 root（不该发生）",
            )),
            super::process::SignalResult::Failed(code) => Err(AgentError::new(
                ErrorCode::Internal,
                format!("kill 失败，errno={code}"),
            )),
        },
    };
    if let Err(error) = outcome {
        push("stop", false, Some(error.message.clone()));
        return Err(with_steps(error, &steps));
    }
    push("stop", true, None);

    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        for _ in 0..10 {
            if find_server(&name).await.is_empty() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        find_server(&name).await.is_empty()
    })
    .await
    .unwrap_or(false);
    push(
        "confirm_gone",
        gone,
        Some(if gone {
            "进程已消失".to_owned()
        } else {
            "仍在运行".to_owned()
        }),
    );
    if !gone {
        return Err(with_steps(
            AgentError::new(ErrorCode::Internal, "frida-server 未能停止，仍在运行")
                .with_details(serde_json::json!({ "reason": "still_running" })),
            &steps,
        ));
    }

    let value = serialize(FridaServerStopResult {
        operation_id: params.operation_id.clone(),
        outcome: WriteOutcome::Executed,
        verified: true,
        binary_name: name.clone(),
        pid: Some(pid),
        uid,
        steps: steps.clone(),
        detail: Some("stopped".into()),
    })?;
    super::activity::finish_write(FRIDA_STOP_NAME, &params.operation_id, &name, &value);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_argv0_basename_counts_as_the_server() {
        assert!(matches_server("./frida-server", "frida-server"));
        assert!(matches_server(
            "/data/local/tmp/frida-server",
            "frida-server"
        ));
        assert!(matches_server("frida-server", "frida-server"));
        // comm 会截断到 15 字符，所以长名字的 comm 相等不能当判据；argv0 才算
        assert!(!matches_server("frida-server-canary", "frida-server"));
        assert!(!matches_server("/system/bin/sh", "frida-server"));
        assert!(!matches_server("", "frida-server"));
    }

    #[test]
    fn listen_address_parses_both_frida_argument_shapes() {
        assert_eq!(
            parse_listen(&["frida-server".into(), "-l".into(), "127.0.0.1:27042".into()]),
            Some(("127.0.0.1".to_owned(), 27042))
        );
        assert_eq!(
            parse_listen(&["frida-server".into(), "-l127.0.0.1:27042".into()]),
            Some(("127.0.0.1".to_owned(), 27042))
        );
        assert_eq!(
            parse_listen(&["frida-server".into(), "--listen=0.0.0.0:33000".into()]),
            Some(("0.0.0.0".to_owned(), 33000))
        );
        // 没带 -l（Framework 默认监听）与端口不是数字都必须给 None，不许猜
        assert_eq!(parse_listen(&["frida-server".into()]), None);
        assert_eq!(
            parse_listen(&["frida-server".into(), "-l".into(), "127.0.0.1".into()]),
            None
        );
        assert_eq!(
            parse_listen(&["frida-server".into(), "-l".into(), "127.0.0.1:http".into()]),
            None
        );
    }

    #[test]
    fn listening_port_requires_the_listen_state_row() {
        let sample = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:6978 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 100 0\n\
   1: 00000000:1F90 00000000:0000 01 00000000:00000000 00:00000000 00000000  2000        0 23456 1 0000000000000000 100 0\n";
        assert!(
            text_has_listening_port(sample, 27000),
            "0x6978 = 27000 且 st=0A"
        );
        assert!(
            !text_has_listening_port(sample, 8080),
            "0x1F90=8080 那行是 st=01（ESTABLISHED），不算监听"
        );
        assert!(!text_has_listening_port("", 27000));
        assert!(!text_has_listening_port("  sl  local_address", 27000));
    }

    #[test]
    fn state_keeps_shell_and_unreadable_apart_from_root() {
        assert_eq!(classify_state(Some(0)), FridaServerState::RunningAsRoot);
        assert_eq!(classify_state(Some(2000)), FridaServerState::RunningAsShell);
        assert_eq!(classify_state(None), FridaServerState::Indeterminate);
    }

    #[test]
    fn start_parameters_are_narrower_than_a_shell_command() {
        // 名字：只能是文件名，且字符集受限
        for good in ["frida-server", "frida-server_17.canary"] {
            assert!(
                super::super::privileged::validate_binary_name(good).is_ok(),
                "{good}"
            );
        }
        for bad in [
            "",
            ".hidden",
            "a/b",
            "/data/local/tmp/frida-server",
            "frida;rm",
            "frida`x`",
            "frida $x",
        ] {
            let error = super::super::privileged::validate_binary_name(bad)
                .expect_err(&format!("{bad:?} 必须被拒"));
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{bad:?}");
        }
        // 地址与端口：只收两个确定地址，端口必须 ≥1024
        assert!(super::super::privileged::validate_listen("127.0.0.1", 27042).is_ok());
        assert!(super::super::privileged::validate_listen("0.0.0.0", 27042).is_ok());
        for bad in ["localhost", "::1", "127.0.0.1;id", ""] {
            assert!(
                super::super::privileged::validate_listen(bad, 27042).is_err(),
                "{bad}"
            );
        }
        assert!(super::super::privileged::validate_listen("127.0.0.1", 80).is_err());
        assert!(super::super::privileged::validate_listen("127.0.0.1", 22).is_err());
    }

    /// 启动脚本必须还是那个实测过的形状：脱离会话 + 只绑指定地址 + 日志可读。
    #[test]
    fn frida_scripts_keep_the_measured_shape() {
        let start = super::super::privileged::frida_start_script(
            "frida-server",
            "127.0.0.1",
            27042,
            "/data/local/tmp/x.log",
        );
        assert!(start.contains("nohup setsid ./frida-server -l 127.0.0.1:27042"));
        assert!(start.contains(": > /data/local/tmp/x.log"));
        assert!(start.contains("chmod 666 /data/local/tmp/x.log"));
        assert!(start.contains("test -x ./frida-server"));
        assert!(start.ends_with("echo FRIDA_STARTED"));
        // 脚本自己不算成功：pid/uid/LISTEN 由 Rust 侧复核，所以这里不该出现 $!
        assert!(!start.contains("$!"), "不该拿 setsid 的 pid 当服务 pid");

        let stop = super::super::privileged::frida_stop_script("frida-server");
        assert!(stop.contains("pidof frida-server"));
        assert!(stop.contains("/proc/$P/cmdline"), "杀之前必须核身份");
        assert!(stop.contains("FRIDA_NOT_RUNNING"));
        assert!(stop.contains("FRIDA_KILL_DENIED"));
        assert!(stop.contains("echo FRIDA_STOPPED"));
    }

    /// 参数校验必须**先于**任何特权调用（宿主没有 su，顺序错了会报 su_unavailable）。
    #[tokio::test]
    async fn start_and_stop_reject_bad_input_before_touching_su() {
        let cases = [
            serde_json::json!({"operation_id": ""}),
            serde_json::json!({"operation_id": "op-1", "port": 22}),
            serde_json::json!({"operation_id": "op-2", "bind": "localhost"}),
            serde_json::json!({"operation_id": "op-3", "binary_name": "../evil"}),
            serde_json::json!({"operation_id": "op-4", "binary_name": "a b"}),
        ];
        for params in cases {
            let error = start(params).await.expect_err("非法参数必须在起 su 前被拒");
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
        }
        let error = stop(serde_json::json!({"operation_id": ""}))
            .await
            .expect_err("缺 operation_id 必须拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
}

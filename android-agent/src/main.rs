use std::env;
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_protocol::PermissionInfo;
use android_agent::provider::zygisk::ZygiskProvider;
use android_agent::server::serve_connection;
use android_agent::system_router_with;
use tokio::net::TcpListener;

#[cfg(target_os = "android")]
use std::os::android::net::SocketAddrExt;
#[cfg(target_os = "android")]
use std::os::unix::net::{SocketAddr as UnixSocketAddr, UnixListener as StdUnixListener};
#[cfg(target_os = "android")]
use tokio::net::UnixListener;

enum ListenTarget {
    Tcp(SocketAddr),
    Abstract(String),
}

struct Config {
    listen: ListenTarget,
    auth_token_file: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_args(env::args().skip(1))?;
    let auth_token = read_and_remove_auth_token(&config.auth_token_file)?;
    let zygisk = Arc::new(ZygiskProvider::new());
    let router = Arc::new(system_router_with(
        auth_token,
        detect_permissions(),
        zygisk.clone(),
    ));
    // 先监听、后预热，顺序不能反。以前这里是 `zygisk.warm_up().await` 在 bind 之前：
    // 探测要起 `su`（读模块令牌、探 root），设备上 su 慢或者刚重启时，这段能吃掉好几秒，
    // 而桌面端等 Agent 可连的预算只有 5 秒——结果进程活着、日志一个字没写、宿主连不上，
    // 看起来像"Agent 起不来"，实际是"还没开始监听就被特权探测拖住了"。
    // 现在 bind/打印 LISTENING 之后在后台预热；没预热完之前 capability 走
    // `unavailable_reason=尚未探测完成`，是真状态而不是猜。
    match config.listen {
        ListenTarget::Tcp(address) => serve_tcp(address, router, zygisk).await?,
        ListenTarget::Abstract(name) => serve_abstract(&name, router, zygisk).await?,
    }
    Ok(())
}

/// 后台预热：不能挡住 accept 循环，也不能被"客户端已连上"这件事依赖。
fn warm_up_in_background(zygisk: Arc<ZygiskProvider>) {
    tokio::spawn(async move {
        zygisk.warm_up().await;
    });
}

async fn serve_tcp(
    address: SocketAddr,
    router: Arc<android_agent::router::Router>,
    zygisk: Arc<ZygiskProvider>,
) -> Result<(), Box<dyn Error>> {
    if !address.ip().is_loopback() {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "agent TCP listener must use a loopback address",
        )
        .into());
    }
    let listener = TcpListener::bind(address).await?;
    println!("LISTENING {}", listener.local_addr()?);
    warm_up_in_background(zygisk);

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, peer) = result?;
                let router = router.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve_connection(stream, router).await {
                        eprintln!("agent connection {peer} closed with error: {error}");
                    }
                });
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                break;
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "android")]
async fn serve_abstract(
    name: &str,
    router: Arc<android_agent::router::Router>,
    zygisk: Arc<ZygiskProvider>,
) -> Result<(), Box<dyn Error>> {
    let address = UnixSocketAddr::from_abstract_name(name.as_bytes())?;
    let listener = StdUnixListener::bind_addr(&address)?;
    listener.set_nonblocking(true)?;
    let listener = UnixListener::from_std(listener)?;
    println!("LISTENING localabstract:{name}");
    warm_up_in_background(zygisk);

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let router = router.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve_connection(stream, router).await {
                        eprintln!("agent abstract socket connection closed with error: {error}");
                    }
                });
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                break;
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
async fn serve_abstract(
    _name: &str,
    _router: Arc<android_agent::router::Router>,
    _zygisk: Arc<ZygiskProvider>,
) -> Result<(), Box<dyn Error>> {
    Err(IoError::new(
        ErrorKind::Unsupported,
        "abstract socket listener is only available on Android",
    )
    .into())
}

fn parse_args(arguments: impl Iterator<Item = String>) -> Result<Config, IoError> {
    let mut listen = None;
    let mut localabstract = None;
    let mut auth_token_file = None;
    let mut arguments = arguments;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--listen" => {
                let value = arguments.next().ok_or_else(|| {
                    IoError::new(ErrorKind::InvalidInput, "--listen requires an address")
                })?;
                listen = Some(value.parse().map_err(|error| {
                    IoError::new(
                        ErrorKind::InvalidInput,
                        format!("invalid --listen address: {error}"),
                    )
                })?);
            }
            "--localabstract" => {
                let name = arguments.next().ok_or_else(|| {
                    IoError::new(
                        ErrorKind::InvalidInput,
                        "--localabstract requires a socket name",
                    )
                })?;
                if name.is_empty()
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                {
                    return Err(IoError::new(
                        ErrorKind::InvalidInput,
                        "localabstract socket name contains invalid characters",
                    ));
                }
                localabstract = Some(name);
            }
            "--auth-token-file" => {
                auth_token_file = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                    IoError::new(ErrorKind::InvalidInput, "--auth-token-file requires a path")
                })?));
            }
            _ => {
                return Err(IoError::new(
                    ErrorKind::InvalidInput,
                    format!("unknown argument: {argument}"),
                ));
            }
        }
    }
    let listen = match (listen, localabstract) {
        (Some(address), None) => ListenTarget::Tcp(address),
        (None, Some(name)) => ListenTarget::Abstract(name),
        (Some(_), Some(_)) => {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "--listen and --localabstract are mutually exclusive",
            ));
        }
        (None, None) => {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "missing required --listen or --localabstract",
            ));
        }
    };
    Ok(Config {
        listen,
        auth_token_file: auth_token_file.ok_or_else(|| {
            IoError::new(
                ErrorKind::InvalidInput,
                "missing required --auth-token-file",
            )
        })?,
    })
}

fn read_and_remove_auth_token(path: &Path) -> Result<String, IoError> {
    validate_auth_token_permissions(path)?;
    let token = fs::read_to_string(path);
    let remove_result = fs::remove_file(path);
    let token = token?;
    remove_result?;
    let token = token.trim_end_matches(['\r', '\n']).to_owned();
    if token.is_empty() {
        return Err(IoError::new(
            ErrorKind::InvalidData,
            "authentication token file is empty",
        ));
    }
    Ok(token)
}

fn detect_permissions() -> PermissionInfo {
    PermissionInfo {
        shell: true,
        root: effective_uid_is_root(),
        selinux_enforcing: fs::read_to_string("/sys/fs/selinux/enforce")
            .is_ok_and(|value| value.trim() == "1"),
    }
}

#[cfg(unix)]
fn effective_uid_is_root() -> bool {
    // geteuid has no preconditions and does not mutate process state.
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn effective_uid_is_root() -> bool {
    false
}

#[cfg(unix)]
fn validate_auth_token_permissions(path: &Path) -> Result<(), IoError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(IoError::new(
            ErrorKind::PermissionDenied,
            "authentication token file permissions must be 0600 or stricter",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_auth_token_permissions(_path: &Path) -> Result<(), IoError> {
    Ok(())
}

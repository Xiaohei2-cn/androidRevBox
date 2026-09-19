use std::env;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use std::sync::Arc;
use std::time::Duration;

use agent_protocol::{ErrorCode, HealthStatus};
use app_reverse_tools_lib::agent_client::{AgentClient, AgentClientError};
use app_reverse_tools_lib::agent_transport::FramedAgentTransport;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};

const AUTH_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri must be inside the workspace")
}

fn cargo_program() -> OsString {
    env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"))
}

fn build_agent_binary() -> PathBuf {
    let metadata = StdCommand::new(cargo_program())
        .current_dir(workspace_root())
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .expect("run cargo metadata");
    assert!(
        metadata.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&metadata.stdout).expect("parse cargo metadata");
    let target_dir = metadata["target_directory"]
        .as_str()
        .map(PathBuf::from)
        .expect("cargo metadata target_directory");

    let build = StdCommand::new(cargo_program())
        .current_dir(workspace_root())
        .args(["build", "-p", "android-agent", "--bin", "android-agent"])
        .output()
        .expect("build host android-agent");
    assert!(
        build.status.success(),
        "android-agent build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let binary = target_dir
        .join("debug")
        .join(format!("android-agent{}", env::consts::EXE_SUFFIX));
    assert!(
        binary.is_file(),
        "agent binary missing: {}",
        binary.display()
    );
    binary
}

fn token_file() -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create auth token file");
    file.write_all(AUTH_TOKEN.as_bytes())
        .expect("write auth token");
    file.flush().expect("flush auth token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .expect("set auth token permissions");
    }
    file
}

async fn start_agent() -> (Child, String, tempfile::NamedTempFile) {
    let binary = build_agent_binary();
    let token = token_file();
    let mut child = Command::new(binary)
        .args(["--listen", "127.0.0.1:0", "--auth-token-file"])
        .arg(token.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn host android-agent");
    let stdout = child.stdout.take().expect("capture agent stdout");
    let mut stdout = BufReader::new(stdout);
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(10), stdout.read_line(&mut line))
        .await
        .expect("agent startup timed out")
        .expect("read agent listen address");
    let address = line
        .trim()
        .strip_prefix("LISTENING ")
        .expect("agent must print LISTENING address")
        .to_owned();
    assert!(
        !token.path().exists(),
        "agent must remove the token file after reading it"
    );
    (child, address, token)
}

async fn connect_client(address: &str) -> AgentClient {
    let stream = TcpStream::connect(address)
        .await
        .expect("connect to host android-agent");
    AgentClient::new(Arc::new(FramedAgentTransport::new(stream)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_agent_full_lifecycle() {
    let (mut child, address, _token) = start_agent().await;

    for _ in 0..100 {
        let client = connect_client(&address).await;
        let hello = client
            .hello(AUTH_TOKEN, "desktop-e2e", Duration::from_secs(2))
            .await
            .expect("hello must succeed");
        assert_eq!(hello.protocol_version, 1);
        assert_eq!(hello.providers.len(), 8);
        assert_eq!(hello.capabilities.len(), 22);
        assert!(
            hello.capabilities.iter().any(|capability| capability.method
                == agent_protocol::method::ACTIVITY_FOREGROUND
                && capability.available),
            "activity.foreground 应由 activity provider 宣告可用"
        );
        assert!(
            hello
                .capabilities
                .iter()
                .any(|capability| capability.method == agent_protocol::method::DEVICE_INFO)
        );
        // AR6.2/AR6.3/AR7.1：端口互查、进程终止与文件 API 都不依赖 root，
        // 宿主上必须由对应 provider 直接宣告可用（缺一个就说明注册漏了）
        for (method, provider) in [
            (agent_protocol::method::PROCESS_PORTS, "process"),
            (agent_protocol::method::PROCESS_BY_PORT, "process"),
            (agent_protocol::method::PROCESS_KILL, "process"),
            (agent_protocol::method::FILESYSTEM_LIST, "filesystem"),
            (agent_protocol::method::FILESYSTEM_STAT, "filesystem"),
            (agent_protocol::method::FILESYSTEM_PREVIEW, "filesystem"),
            (agent_protocol::method::HOSTED_LIST, "hosted"),
            (agent_protocol::method::HOSTED_CHMOD, "hosted"),
            (agent_protocol::method::HOSTED_START, "hosted"),
            (agent_protocol::method::HOSTED_STATUS, "hosted"),
            (agent_protocol::method::HOSTED_STOP, "hosted"),
            (agent_protocol::method::PACKAGE_NATIVE_LIB_DIR, "package"),
        ] {
            let capability = hello
                .capabilities
                .iter()
                .find(|capability| capability.method == method)
                .unwrap_or_else(|| panic!("{method} must be discoverable"));
            assert_eq!(capability.provider, provider, "{method} 的 provider 不对");
            assert!(capability.available, "{method} 应宣告可用");
        }
        // Zygisk 模块在宿主机上必然不存在：只能降级 provider.zygisk 的具体方法，
        // 既不能假成功，也不能让 shell/系统能力一起消失（AR5.3 第 6 条）。
        assert!(
            hello.capabilities.iter().any(|capability| capability.method
                == agent_protocol::method::PACKAGE_LIST
                && capability.provider == "shell"
                && capability.available),
            "package.list 应由 ShellProvider 宣告且始终可用"
        );
        let zygisk_status = hello
            .capabilities
            .iter()
            .find(|capability| capability.method == agent_protocol::method::ZYGISK_STATUS)
            .expect("zygisk.status must be discoverable for diagnostics");
        assert!(
            zygisk_status.available,
            "生命周期诊断必须始终可调用，否则 UI 拿不到失败原因"
        );
        let localized = hello
            .capabilities
            .iter()
            .find(|capability| capability.method == agent_protocol::method::PACKAGE_LIST_LOCALIZED)
            .expect("package.list_localized must be discoverable");
        assert!(!localized.available, "缺模块时不得声明本地化清单可用");
        assert!(
            localized.unavailable_reason.is_some(),
            "不可用必须带原因，不能只报失败"
        );
        let health = client
            .health(Duration::from_secs(2))
            .await
            .expect("health must succeed");
        assert_eq!(health.status, HealthStatus::Ready);
    }

    let client = connect_client(&address).await;
    client
        .hello(AUTH_TOKEN, "desktop-e2e", Duration::from_secs(2))
        .await
        .expect("hello before concurrent requests");
    let mut concurrent = Vec::new();
    for _ in 0..16 {
        let client = client.clone();
        concurrent.push(tokio::spawn(async move {
            client.health(Duration::from_secs(2)).await
        }));
    }
    for request in concurrent {
        assert_eq!(request.await.unwrap().unwrap().status, HealthStatus::Ready);
    }

    let unknown = client
        .request_value("unknown.method", json!({}), Duration::from_secs(2))
        .await
        .unwrap_err();
    assert!(matches!(
        unknown,
        AgentClientError::Remote(ref error) if error.code == ErrorCode::UnsupportedMethod
    ));
    assert_eq!(
        client
            .health(Duration::ZERO)
            .await
            .expect_err("zero deadline must fail locally"),
        AgentClientError::DeadlineExceeded
    );
    assert_eq!(
        client
            .health(Duration::from_secs(2))
            .await
            .expect("connection remains usable after local deadline")
            .status,
        HealthStatus::Ready
    );

    child.kill().await.expect("kill host android-agent");
    let status = child.wait().await.expect("reap host android-agent");
    assert!(
        !status.success(),
        "killed agent should not exit successfully"
    );
    let disconnected = tokio::time::timeout(
        Duration::from_secs(2),
        client.health(Duration::from_secs(10)),
    )
    .await
    .expect("disconnect must wake client without waiting for request deadline")
    .unwrap_err();
    assert!(matches!(disconnected, AgentClientError::TransportLost(_)));
}

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use agent_protocol::{ErrorCode, HelloResult, PROTOCOL_VERSION, ProviderHealth};
use tokio::net::TcpStream;
use tokio::sync::Mutex as AsyncMutex;

use crate::adapters::agent_bootstrap::{
    AgentBootstrap, AgentBootstrapError, AgentInstallOutcome, AgentLaunch,
};
use crate::adapters::agent_transport::FramedAgentTransport;
use crate::models::agent::{AgentDiagnostics, AgentSessionState, AgentSessionStatus};
use crate::services::agent_artifact::{AgentArtifact, AgentArtifactError, AgentArtifactResolver};
use crate::services::agent_client::{AgentClient, AgentClientError};
use crate::services::device_service::AdbRunner;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const REUSE_HEALTH_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
pub enum AgentManagerError {
    #[error("invalid device serial")]
    InvalidSerial,
    #[error(transparent)]
    Bootstrap(#[from] AgentBootstrapError),
    #[error(transparent)]
    Artifact(#[from] AgentArtifactError),
    #[error("failed to connect to forwarded Agent port {port}: {detail}")]
    Connect { port: u16, detail: String },
    #[error(transparent)]
    Client(#[from] AgentClientError),
    #[error("Agent protocol {actual} is incompatible with Desktop protocol {expected}")]
    IncompatibleProtocol { actual: u32, expected: u32 },
    #[error("Agent version {actual} does not match selected artifact version {expected}")]
    IncompatibleAgentVersion { actual: String, expected: String },
    #[error("Agent startup failed: {cause}; rollback: {rollback}")]
    StartupFailed {
        cause: Box<AgentManagerError>,
        rollback: String,
    },
}

struct DeviceSession {
    operation: AsyncMutex<()>,
    status: RwLock<AgentSessionStatus>,
    client: Mutex<Option<AgentClient>>,
    forward: Mutex<Option<String>>,
}

impl DeviceSession {
    fn new(serial: &str) -> Self {
        Self {
            operation: AsyncMutex::new(()),
            status: RwLock::new(AgentSessionStatus::disconnected(serial)),
            client: Mutex::new(None),
            forward: Mutex::new(None),
        }
    }

    fn status(&self) -> AgentSessionStatus {
        self.status
            .read()
            .expect("agent status lock poisoned")
            .clone()
    }

    fn transition(&self, state: AgentSessionState) {
        self.status
            .write()
            .expect("agent status lock poisoned")
            .transition(state);
    }

    fn fail(&self, state: AgentSessionState, error: impl Into<String>) {
        self.status
            .write()
            .expect("agent status lock poisoned")
            .fail(state, error);
    }
}

pub struct AgentManager {
    bootstrap: Arc<AgentBootstrap>,
    artifacts: Arc<AgentArtifactResolver>,
    sessions: Mutex<HashMap<String, Arc<DeviceSession>>>,
}

impl AgentManager {
    pub fn new(runner: Arc<dyn AdbRunner>, artifacts: Arc<AgentArtifactResolver>) -> Self {
        Self {
            bootstrap: Arc::new(AgentBootstrap::new(runner)),
            artifacts,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub async fn connect_resolved(
        &self,
        serial: &str,
    ) -> Result<AgentSessionStatus, AgentManagerError> {
        let artifact = self
            .artifacts
            .resolve(crate::adapters::agent_bootstrap::DeviceAbi::Arm64V8a)?;
        self.connect_artifact(serial, artifact).await
    }

    pub async fn connect(
        &self,
        serial: &str,
        binary: &Path,
    ) -> Result<AgentSessionStatus, AgentManagerError> {
        let artifact = AgentArtifact::explicit(
            binary,
            crate::adapters::agent_bootstrap::DeviceAbi::Arm64V8a,
        )?;
        self.connect_artifact(serial, artifact).await
    }

    async fn connect_artifact(
        &self,
        serial: &str,
        artifact: AgentArtifact,
    ) -> Result<AgentSessionStatus, AgentManagerError> {
        validate_serial(serial)?;
        let session = self.session_for(serial);
        let _operation = session.operation.lock().await;

        let current = session.status();
        let current_client = session
            .client
            .lock()
            .expect("agent client lock poisoned")
            .clone();
        if matches!(
            current.state,
            AgentSessionState::Ready | AgentSessionState::Degraded
        ) && let Some(client) = current_client
            && client.health(REUSE_HEALTH_TIMEOUT).await.is_ok()
        {
            return Ok(current);
        }

        *session.client.lock().expect("agent client lock poisoned") = None;
        *session.status.write().expect("agent status lock poisoned") =
            AgentSessionStatus::disconnected(serial);
        if let Err(error) = self.bootstrap.ensure_online(serial).await {
            session.fail(AgentSessionState::Disconnected, error.to_string());
            return Err(error.into());
        }
        session.transition(AgentSessionState::AdbOnline);
        let stale_forward = session
            .forward
            .lock()
            .expect("agent forward lock poisoned")
            .take();
        if let Some(local) = stale_forward {
            if let Err(error) = self.bootstrap.remove_forward(serial, &local).await {
                tracing::warn!(serial, local, error = %error, "failed to remove stale Agent forward");
            }
        }

        let abi = match self.bootstrap.device_abi(serial).await {
            Ok(abi) => abi,
            Err(error) => {
                session.fail(AgentSessionState::Disconnected, error.to_string());
                return Err(error.into());
            }
        };
        if abi != artifact.abi {
            let error =
                AgentManagerError::Artifact(AgentArtifactError::UnsupportedAbi(abi.as_str()));
            session.fail(AgentSessionState::Disconnected, error.to_string());
            return Err(error);
        }

        session.transition(AgentSessionState::Installing);
        if let Err(error) = self.bootstrap.stop(serial).await {
            session.fail(AgentSessionState::Disconnected, error.to_string());
            return Err(error.into());
        }
        let install = match self
            .bootstrap
            .install_artifact(serial, &artifact.path, &artifact.sha256)
            .await
        {
            Ok(install) => install,
            Err(error) => {
                session.fail(AgentSessionState::Disconnected, error.to_string());
                return Err(error.into());
            }
        };

        session.transition(AgentSessionState::Starting);
        let launch = match self.bootstrap.start(serial).await {
            Ok(launch) => launch,
            Err(error) => {
                let error = self
                    .recover_failed_start(
                        serial,
                        &session,
                        &install,
                        None,
                        error.into(),
                        AgentSessionState::Disconnected,
                    )
                    .await;
                return Err(error);
            }
        };
        let (local_forward, local_port) = match self.bootstrap.forward(serial).await {
            Ok(forward) => forward,
            Err(error) => {
                let error = self
                    .recover_failed_start(
                        serial,
                        &session,
                        &install,
                        Some(&launch),
                        error.into(),
                        AgentSessionState::Disconnected,
                    )
                    .await;
                return Err(error);
            }
        };
        *session.forward.lock().expect("agent forward lock poisoned") = Some(local_forward.clone());
        {
            let mut status = session.status.write().expect("agent status lock poisoned");
            status.local_port = Some(local_port);
            status.transition(AgentSessionState::Handshaking);
        }

        let result = self
            .handshake(
                serial,
                local_port,
                &launch,
                &artifact.expected_agent_version,
                &session,
            )
            .await;
        let status = match result {
            Ok(status) => status,
            Err(error) => {
                let failure_state = session.status().state;
                let error = self
                    .recover_failed_start(
                        serial,
                        &session,
                        &install,
                        Some(&launch),
                        error,
                        failure_state,
                    )
                    .await;
                return Err(error);
            }
        };
        self.bootstrap.cleanup_launch(serial, &launch).await;
        if install.changed || install.rollback_available {
            if let Err(error) = self.bootstrap.commit_install(serial).await {
                let error = self
                    .recover_failed_start(
                        serial,
                        &session,
                        &install,
                        None,
                        error.into(),
                        AgentSessionState::Disconnected,
                    )
                    .await;
                return Err(error);
            }
        }
        tracing::info!(
            serial,
            artifact_source = ?artifact.source,
            artifact_sha256 = %artifact.sha256,
            changed = install.changed,
            "Agent artifact accepted"
        );
        Ok(status)
    }

    pub fn status(&self, serial: &str) -> AgentSessionStatus {
        self.sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .get(serial)
            .map(|session| session.status())
            .unwrap_or_else(|| AgentSessionStatus::disconnected(serial))
    }

    pub fn statuses(&self) -> Vec<AgentSessionStatus> {
        let mut statuses: Vec<_> = self
            .sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .values()
            .map(|session| session.status())
            .collect();
        statuses.sort_by(|left, right| left.serial.cmp(&right.serial));
        statuses
    }

    pub fn client(&self, serial: &str) -> Option<AgentClient> {
        self.sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .get(serial)
            .and_then(|session| {
                session
                    .client
                    .lock()
                    .expect("agent client lock poisoned")
                    .clone()
            })
    }

    pub async fn diagnostics(&self, serial: &str) -> AgentDiagnostics {
        if validate_serial(serial).is_err() {
            return AgentDiagnostics {
                status: AgentSessionStatus::disconnected(serial),
                health: None,
                health_error: Some("invalid device serial".into()),
                routes: Vec::new(),
            };
        }
        let session = self.session_for(serial);
        let _operation = session.operation.lock().await;
        let client = session
            .client
            .lock()
            .expect("agent client lock poisoned")
            .clone();
        let Some(client) = client else {
            let status = session.status();
            return AgentDiagnostics {
                health_error: status
                    .last_error
                    .clone()
                    .or_else(|| Some("Agent session is not connected".into())),
                status,
                health: None,
                routes: Vec::new(),
            };
        };
        match client.health(REUSE_HEALTH_TIMEOUT).await {
            Ok(health) => AgentDiagnostics {
                status: session.status(),
                health: Some(health),
                health_error: None,
                routes: Vec::new(),
            },
            Err(error) => {
                *session.client.lock().expect("agent client lock poisoned") = None;
                session.fail(AgentSessionState::Disconnected, error.to_string());
                AgentDiagnostics {
                    status: session.status(),
                    health: None,
                    health_error: Some(error.to_string()),
                    routes: Vec::new(),
                }
            }
        }
    }

    pub async fn disconnect(&self, serial: &str) -> Result<(), AgentManagerError> {
        validate_serial(serial)?;
        let session = self.session_for(serial);
        let _operation = session.operation.lock().await;
        *session.client.lock().expect("agent client lock poisoned") = None;
        let local = session
            .forward
            .lock()
            .expect("agent forward lock poisoned")
            .take();
        let forward_result = match local {
            Some(local) => self.bootstrap.remove_forward(serial, &local).await,
            None => Ok(()),
        };
        let stop_result = self.bootstrap.stop(serial).await;
        *session.status.write().expect("agent status lock poisoned") =
            AgentSessionStatus::disconnected(serial);
        forward_result?;
        stop_result?;
        Ok(())
    }

    pub async fn device_disconnected(&self, serial: &str) {
        let session = {
            self.sessions
                .lock()
                .expect("agent sessions lock poisoned")
                .get(serial)
                .cloned()
        };
        let Some(session) = session else {
            return;
        };
        let _operation = session.operation.lock().await;
        if let Some(client) = session
            .client
            .lock()
            .expect("agent client lock poisoned")
            .take()
        {
            client.disconnect("ADB device disconnected");
        }
        let local = session
            .forward
            .lock()
            .expect("agent forward lock poisoned")
            .take();
        if let Some(local) = local {
            if let Err(error) = self.bootstrap.remove_forward(serial, &local).await {
                tracing::debug!(serial, local, error = %error, "forward cleanup after device disconnect was not acknowledged");
            }
        }
        session.fail(AgentSessionState::Disconnected, "ADB device disconnected");
    }

    async fn handshake(
        &self,
        serial: &str,
        local_port: u16,
        launch: &AgentLaunch,
        expected_agent_version: &str,
        session: &DeviceSession,
    ) -> Result<AgentSessionStatus, AgentManagerError> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let (client, hello) = loop {
            let stream = match connect_forwarded(local_port, deadline).await {
                Ok(stream) => stream,
                Err(error) => {
                    session.fail(AgentSessionState::Disconnected, error.to_string());
                    return Err(error);
                }
            };
            let client = AgentClient::new(Arc::new(FramedAgentTransport::new(stream)));
            let hello_timeout = deadline
                .saturating_duration_since(Instant::now())
                .min(HELLO_TIMEOUT);
            let hello = client
                .hello(
                    launch.auth_token.as_str(),
                    env!("CARGO_PKG_VERSION"),
                    hello_timeout,
                )
                .await;
            match hello {
                Ok(hello) => break (client, hello),
                Err(AgentClientError::TransportLost(_)) if Instant::now() < deadline => {
                    tokio::time::sleep(CONNECT_RETRY_DELAY).await;
                }
                Err(error) => {
                    let state = if matches!(
                        &error,
                        AgentClientError::Remote(remote)
                            if remote.code == ErrorCode::IncompatibleVersion
                    ) {
                        AgentSessionState::Incompatible
                    } else {
                        AgentSessionState::Disconnected
                    };
                    session.fail(state, error.to_string());
                    return Err(error.into());
                }
            }
        };
        if hello.protocol_version != PROTOCOL_VERSION {
            let error = AgentManagerError::IncompatibleProtocol {
                actual: hello.protocol_version,
                expected: PROTOCOL_VERSION,
            };
            session.fail(AgentSessionState::Incompatible, error.to_string());
            return Err(error);
        }
        if hello.agent_version != expected_agent_version {
            let error = AgentManagerError::IncompatibleAgentVersion {
                actual: hello.agent_version,
                expected: expected_agent_version.into(),
            };
            session.fail(AgentSessionState::Incompatible, error.to_string());
            return Err(error);
        }

        let state = session_state_from_hello(&hello);
        {
            let mut status = session.status.write().expect("agent status lock poisoned");
            status.state = state;
            status.agent_version = Some(hello.agent_version);
            status.protocol_version = Some(hello.protocol_version);
            status.permissions = Some(hello.permissions);
            status.providers = hello.providers;
            status.capabilities = hello.capabilities;
            status.local_port = Some(local_port);
            status.last_error = None;
        }
        *session.client.lock().expect("agent client lock poisoned") = Some(client);
        tracing::info!(serial, local_port, state = ?state, "Agent session established");
        Ok(session.status())
    }

    async fn recover_failed_start(
        &self,
        serial: &str,
        session: &DeviceSession,
        install: &AgentInstallOutcome,
        launch: Option<&AgentLaunch>,
        cause: AgentManagerError,
        failure_state: AgentSessionState,
    ) -> AgentManagerError {
        *session.client.lock().expect("agent client lock poisoned") = None;
        let local = session
            .forward
            .lock()
            .expect("agent forward lock poisoned")
            .take();
        if let Some(local) = local {
            let _ = self.bootstrap.remove_forward(serial, &local).await;
        }
        if let Some(launch) = launch {
            self.bootstrap.cleanup_launch(serial, launch).await;
        }
        let _ = self.bootstrap.stop(serial).await;
        let rollback = if install.rollback_available {
            match self.bootstrap.rollback_install(serial).await {
                Ok(true) => format!(
                    "restored previous binary {}",
                    install
                        .previous_sha256
                        .as_deref()
                        .unwrap_or("unknown SHA-256")
                ),
                Ok(false) => "previous binary was no longer available".into(),
                Err(error) => format!("failed: {error}"),
            }
        } else if install.changed {
            "not available (first install)".into()
        } else {
            "not required (binary was unchanged)".into()
        };
        let error = AgentManagerError::StartupFailed {
            cause: Box::new(cause),
            rollback,
        };
        session.fail(failure_state, error.to_string());
        error
    }

    fn session_for(&self, serial: &str) -> Arc<DeviceSession> {
        self.sessions
            .lock()
            .expect("agent sessions lock poisoned")
            .entry(serial.to_owned())
            .or_insert_with(|| Arc::new(DeviceSession::new(serial)))
            .clone()
    }
}

fn validate_serial(serial: &str) -> Result<(), AgentManagerError> {
    if serial.trim().is_empty() || serial.contains('\0') {
        Err(AgentManagerError::InvalidSerial)
    } else {
        Ok(())
    }
}

fn session_state_from_hello(hello: &HelloResult) -> AgentSessionState {
    let provider_degraded = hello
        .providers
        .iter()
        .any(|provider| provider.health != ProviderHealth::Ready);
    let capability_degraded = hello
        .capabilities
        .iter()
        .any(|capability| !capability.available);
    if provider_degraded || capability_degraded {
        AgentSessionState::Degraded
    } else {
        AgentSessionState::Ready
    }
}

async fn connect_forwarded(port: u16, deadline: Instant) -> Result<TcpStream, AgentManagerError> {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    loop {
        if Instant::now() >= deadline {
            return Err(AgentManagerError::Connect {
                port,
                detail: "Agent startup deadline exceeded".into(),
            });
        }
        match TcpStream::connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) if Instant::now() >= deadline => {
                return Err(AgentManagerError::Connect {
                    port,
                    detail: error.to_string(),
                });
            }
            Err(_) => {}
        }
        tokio::time::sleep(CONNECT_RETRY_DELAY).await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agent_protocol::{
        AgentError, CapabilityInfo, DeviceInfoParams, DeviceInfoResult, ErrorCode, HealthResult,
        HealthStatus, PermissionInfo, ProviderInfo, RequestEnvelope, ResponseEnvelope, encode_json,
    };
    use async_trait::async_trait;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;

    use crate::adapters::adb;
    use crate::core::error::CoreResult;
    use crate::db::Db;
    use crate::services::agent_artifact::AgentArtifactResolver;
    use crate::services::config_service::ConfigService;
    use crate::services::device_service::{
        AdbEnvironment, AdbRunOutput, MockAdbRunner, RealAdbRunner,
    };

    use super::*;

    fn hello(provider_health: ProviderHealth, capability_available: bool) -> HelloResult {
        HelloResult {
            protocol_version: PROTOCOL_VERSION,
            agent_version: env!("CARGO_PKG_VERSION").into(),
            permissions: PermissionInfo {
                shell: true,
                root: false,
                selinux_enforcing: true,
            },
            providers: vec![ProviderInfo {
                name: "system".into(),
                version: "0.1.0".into(),
                health: provider_health,
                required_permissions: Vec::new(),
                last_error: None,
            }],
            capabilities: vec![CapabilityInfo {
                method: "system.health".into(),
                version: 1,
                provider: "system".into(),
                available: capability_available,
                unavailable_reason: None,
            }],
        }
    }

    fn manager() -> AgentManager {
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let artifacts = Arc::new(AgentArtifactResolver::new(config, None));
        AgentManager::new(Arc::new(MockAdbRunner::new(true)), artifacts)
    }

    const TEST_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[derive(Clone, Copy)]
    enum FakeAgentMode {
        Ready,
        Incompatible,
        CloseFirstThenReady,
    }

    async fn spawn_fake_agent(mode: FakeAgentMode) -> (u16, JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let mut connection_index = 0;
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let current_index = connection_index;
                connection_index += 1;
                tokio::spawn(serve_fake_connection(stream, mode, current_index));
            }
        });
        (port, task)
    }

    async fn serve_fake_connection(
        mut stream: TcpStream,
        mode: FakeAgentMode,
        connection_index: usize,
    ) {
        if matches!(mode, FakeAgentMode::CloseFirstThenReady) && connection_index == 0 {
            return;
        }
        loop {
            let request = match read_request(&mut stream).await {
                Some(request) => request,
                None => return,
            };
            let response = match (mode, request.method.as_str()) {
                (FakeAgentMode::Incompatible, agent_protocol::method::SYSTEM_HELLO) => {
                    ResponseEnvelope::failure(
                        request.request_id,
                        AgentError::new(ErrorCode::IncompatibleVersion, "test mismatch"),
                    )
                }
                (_, agent_protocol::method::SYSTEM_HELLO) => ResponseEnvelope::success(
                    request.request_id,
                    serde_json::to_value(hello(ProviderHealth::Ready, true)).unwrap(),
                ),
                (_, agent_protocol::method::SYSTEM_HEALTH) => ResponseEnvelope::success(
                    request.request_id,
                    serde_json::to_value(HealthResult {
                        status: HealthStatus::Ready,
                        agent_version: env!("CARGO_PKG_VERSION").into(),
                        protocol_version: PROTOCOL_VERSION,
                        uptime_ms: 1,
                    })
                    .unwrap(),
                ),
                _ => continue,
            };
            if stream
                .write_all(&encode_json(&response).unwrap())
                .await
                .is_err()
            {
                return;
            }
            if matches!(mode, FakeAgentMode::Incompatible) {
                return;
            }
        }
    }

    async fn read_request(stream: &mut TcpStream) -> Option<RequestEnvelope> {
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await.ok()?;
        let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut payload).await.ok()?;
        serde_json::from_slice(&payload).ok()
    }

    struct SessionAdbRunner {
        ports: HashMap<String, u16>,
        get_state_delay: Duration,
        active_get_state: AtomicUsize,
        max_get_state: AtomicUsize,
        commands: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl SessionAdbRunner {
        fn new(ports: impl IntoIterator<Item = (String, u16)>, delay: Duration) -> Self {
            Self {
                ports: ports.into_iter().collect(),
                get_state_delay: delay,
                active_get_state: AtomicUsize::new(0),
                max_get_state: AtomicUsize::new(0),
                commands: Mutex::new(Vec::new()),
            }
        }

        fn count_command(&self, needle: &str) -> usize {
            self.commands
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, args)| args.iter().any(|argument| argument.contains(needle)))
                .count()
        }
    }

    #[async_trait]
    impl AdbRunner for SessionAdbRunner {
        async fn run(
            &self,
            _adb_path: &str,
            args: &[String],
            _timeout: Duration,
        ) -> CoreResult<AdbRunOutput> {
            let serial = args.get(1).cloned().unwrap_or_default();
            let command = args.get(2..).unwrap_or_default();
            self.commands
                .lock()
                .unwrap()
                .push((serial.clone(), command.to_vec()));
            let stdout = if command == ["get-state"] {
                let active = self.active_get_state.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_get_state.fetch_max(active, Ordering::SeqCst);
                tokio::time::sleep(self.get_state_delay).await;
                self.active_get_state.fetch_sub(1, Ordering::SeqCst);
                "device\n".into()
            } else if command
                .iter()
                .any(|argument| argument.contains("getprop ro.product.cpu.abi"))
            {
                "arm64-v8a\n".into()
            } else if command
                .iter()
                .any(|argument| argument.contains("app_reverse_tools_agent.rollback"))
            {
                String::new()
            } else if command.iter().any(|argument| {
                argument.contains("sha256sum /data/local/tmp/app_reverse_tools_agent")
            }) {
                format!("{TEST_SHA256}  /data/local/tmp/app_reverse_tools_agent\n")
            } else if command
                .first()
                .is_some_and(|argument| argument == "forward")
                && command.get(1).is_some_and(|argument| argument == "tcp:0")
            {
                format!("{}\n", self.ports[&serial])
            } else {
                String::new()
            };
            Ok(AdbRunOutput {
                stdout,
                stderr: String::new(),
                exit_code: Some(0),
            })
        }

        async fn environment(&self) -> AdbEnvironment {
            AdbEnvironment {
                installed: true,
                path: Some("/mock/adb".into()),
                source: Some("mock".into()),
                version: None,
                hint: None,
                probe_error: None,
            }
        }

        fn invalidate_cache(&self) {}
    }

    fn manager_with_runner(runner: Arc<dyn AdbRunner>) -> AgentManager {
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let artifacts = Arc::new(AgentArtifactResolver::new(config, None));
        AgentManager::new(runner, artifacts)
    }

    fn test_artifact() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("android-agent"), b"abc").unwrap();
        directory
    }

    #[test]
    fn hello_health_maps_to_ready_or_degraded() {
        assert_eq!(
            session_state_from_hello(&hello(ProviderHealth::Ready, true)),
            AgentSessionState::Ready
        );
        assert_eq!(
            session_state_from_hello(&hello(ProviderHealth::Degraded, true)),
            AgentSessionState::Degraded
        );
        assert_eq!(
            session_state_from_hello(&hello(ProviderHealth::Ready, false)),
            AgentSessionState::Degraded
        );
    }

    #[test]
    fn manager_reuses_one_session_lock_per_serial_only() {
        let manager = manager();
        let first = manager.session_for("serial-a");
        let same = manager.session_for("serial-a");
        let other = manager.session_for("serial-b");
        assert!(Arc::ptr_eq(&first, &same));
        assert!(!Arc::ptr_eq(&first, &other));

        let _first_guard = first.operation.try_lock().unwrap();
        assert!(same.operation.try_lock().is_err());
        assert!(other.operation.try_lock().is_ok());
    }

    #[test]
    fn status_for_unknown_serial_is_disconnected() {
        let manager = manager();
        assert_eq!(
            manager.status("unknown").state,
            AgentSessionState::Disconnected
        );
    }

    #[tokio::test]
    async fn same_serial_connect_is_deduplicated_and_different_serials_overlap() {
        let (port_a, server_a) = spawn_fake_agent(FakeAgentMode::Ready).await;
        let (port_b, server_b) = spawn_fake_agent(FakeAgentMode::Ready).await;
        let runner = Arc::new(SessionAdbRunner::new(
            [("serial-a".into(), port_a), ("serial-b".into(), port_b)],
            Duration::from_millis(80),
        ));
        let manager = Arc::new(manager_with_runner(runner.clone()));
        let artifact = test_artifact();
        let binary = artifact.path().join("android-agent");

        let (first, duplicate) = tokio::join!(
            manager.connect("serial-a", &binary),
            manager.connect("serial-a", &binary)
        );
        assert_eq!(first.unwrap().state, AgentSessionState::Ready);
        assert_eq!(duplicate.unwrap().state, AgentSessionState::Ready);
        assert_eq!(runner.count_command("get-state"), 1);

        manager.disconnect("serial-a").await.unwrap();
        let (serial_a, serial_b) = tokio::join!(
            manager.connect("serial-a", &binary),
            manager.connect("serial-b", &binary)
        );
        assert_eq!(serial_a.unwrap().state, AgentSessionState::Ready);
        assert_eq!(serial_b.unwrap().state, AgentSessionState::Ready);
        assert!(runner.max_get_state.load(Ordering::SeqCst) >= 2);
        manager.disconnect("serial-a").await.unwrap();
        manager.disconnect("serial-b").await.unwrap();
        server_a.abort();
        server_b.abort();
    }

    #[tokio::test]
    async fn handshake_retries_when_forward_connects_before_agent_listener_is_ready() {
        let (port, server) = spawn_fake_agent(FakeAgentMode::CloseFirstThenReady).await;
        let runner = Arc::new(SessionAdbRunner::new(
            [("serial-a".into(), port)],
            Duration::ZERO,
        ));
        let manager = manager_with_runner(runner);
        let artifact = test_artifact();

        let status = manager
            .connect("serial-a", &artifact.path().join("android-agent"))
            .await
            .unwrap();
        assert_eq!(status.state, AgentSessionState::Ready);
        manager.disconnect("serial-a").await.unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn incompatible_hello_blocks_session_and_cleans_forward() {
        let (port, server) = spawn_fake_agent(FakeAgentMode::Incompatible).await;
        let runner = Arc::new(SessionAdbRunner::new(
            [("serial-a".into(), port)],
            Duration::ZERO,
        ));
        let manager = manager_with_runner(runner.clone());
        let artifact = test_artifact();

        let error = manager
            .connect("serial-a", &artifact.path().join("android-agent"))
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AgentManagerError::StartupFailed { cause, .. }
                if matches!(*cause, AgentManagerError::Client(AgentClientError::Remote(ref remote)) if remote.code == ErrorCode::IncompatibleVersion)
        ));
        assert_eq!(
            manager.status("serial-a").state,
            AgentSessionState::Incompatible
        );
        assert_eq!(runner.count_command("--remove"), 1);
        assert!(manager.client("serial-a").is_none());
        server.abort();
    }

    #[tokio::test]
    async fn device_disconnect_wakes_pending_and_removes_forward() {
        let (port, server) = spawn_fake_agent(FakeAgentMode::Ready).await;
        let runner = Arc::new(SessionAdbRunner::new(
            [("serial-a".into(), port)],
            Duration::ZERO,
        ));
        let manager = Arc::new(manager_with_runner(runner.clone()));
        let artifact = test_artifact();
        manager
            .connect("serial-a", &artifact.path().join("android-agent"))
            .await
            .unwrap();

        let client = manager.client("serial-a").unwrap();
        let pending = tokio::spawn(async move {
            client
                .request_value(
                    "test.pending",
                    serde_json::json!({}),
                    Duration::from_secs(10),
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        manager.device_disconnected("serial-a").await;
        assert!(matches!(
            pending.await.unwrap(),
            Err(AgentClientError::TransportLost(_))
        ));
        assert_eq!(
            manager.status("serial-a").state,
            AgentSessionState::Disconnected
        );
        assert_eq!(runner.count_command("--remove"), 1);
        assert!(manager.client("serial-a").is_none());
        server.abort();
    }

    #[tokio::test]
    #[ignore = "需要真机；AR4_TEST_SERIAL=<serial> cargo test real_device_manager_first_install_then_sha_reuse_and_rollback -- --ignored --nocapture"]
    async fn real_device_manager_first_install_then_sha_reuse_and_rollback() {
        let serial = std::env::var("AR4_TEST_SERIAL").expect("AR4_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let artifacts = Arc::new(AgentArtifactResolver::new(config, None));
        let manager = AgentManager::new(runner, artifacts);

        let first = manager.connect_resolved(&serial).await.unwrap();
        assert!(matches!(
            first.state,
            AgentSessionState::Ready | AgentSessionState::Degraded
        ));
        assert_eq!(first.protocol_version, Some(PROTOCOL_VERSION));
        manager.disconnect(&serial).await.unwrap();

        let reused = manager.connect_resolved(&serial).await.unwrap();
        assert!(matches!(
            reused.state,
            AgentSessionState::Ready | AgentSessionState::Degraded
        ));
        assert_eq!(
            reused.agent_version.as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        manager.disconnect(&serial).await.unwrap();

        let directory = tempfile::tempdir().unwrap();
        let invalid = directory.path().join("invalid-android-agent");
        std::fs::write(&invalid, b"not an Android executable").unwrap();
        let error = manager.connect(&serial, &invalid).await.unwrap_err();
        assert!(matches!(error, AgentManagerError::StartupFailed { .. }));
        assert!(error.to_string().contains("restored previous binary"));

        let recovered = manager.connect_resolved(&serial).await.unwrap();
        assert!(matches!(
            recovered.state,
            AgentSessionState::Ready | AgentSessionState::Degraded
        ));

        let killed_client = manager.client(&serial).unwrap();
        manager.bootstrap.stop(&serial).await.unwrap();
        assert!(matches!(
            killed_client.health(REUSE_HEALTH_TIMEOUT).await,
            Err(AgentClientError::TransportLost(_))
        ));
        let restarted = manager.connect_resolved(&serial).await.unwrap();
        assert!(matches!(
            restarted.state,
            AgentSessionState::Ready | AgentSessionState::Degraded
        ));
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR5.5 路由矩阵的只读腿：Agent `package.list` 必须在设备端解析完成，
    /// 并与 Legacy `pm list packages -3` 的集合一致（迁移期 shadow 对照的真机版本）。
    #[tokio::test]
    #[ignore = "需要真机；AR4_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_package_list_matches_legacy -- --ignored --nocapture"]
    async fn real_agent_package_list_matches_legacy() {
        use agent_protocol::{PackageListParams, PackageListResult, PackageScope};

        let serial = std::env::var("AR4_TEST_SERIAL").expect("AR4_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let artifacts = Arc::new(AgentArtifactResolver::new(config, None));
        let manager = AgentManager::new(runner.clone(), artifacts);
        let status = manager.connect_resolved(&serial).await.unwrap();
        assert!(
            status
                .capabilities
                .iter()
                .any(|capability| capability.method == agent_protocol::method::PACKAGE_LIST),
            "Agent 未发布 package.list capability"
        );

        let client = manager.client(&serial).unwrap();
        let user = client
            .request::<_, PackageListResult>(
                agent_protocol::method::PACKAGE_LIST,
                &PackageListParams {
                    scope: PackageScope::User,
                    include_disabled: false,
                },
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        assert!(!user.items.is_empty(), "Agent 三方包列表为空");
        assert!(
            user.items.iter().all(|item| !item.is_system),
            "scope=user 混入系统包"
        );
        assert!(
            user.items
                .windows(2)
                .all(|pair| pair[0].package_name <= pair[1].package_name),
            "package.list 必须稳定排序，否则 UI 抖动"
        );
        let with_uid = user.items.iter().filter(|item| item.uid.is_some()).count();
        assert!(
            with_uid as f64 / user.items.len() as f64 > 0.9,
            "绝大多数三方包应带 uid，实际 {with_uid}/{}",
            user.items.len()
        );

        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();
        let legacy = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_list_packages(true)),
                Duration::from_secs(15),
            )
            .await
            .unwrap();
        let legacy_names: std::collections::HashSet<String> =
            crate::adapters::adb::parse_packages(&legacy.stdout)
                .into_iter()
                .collect();
        let agent_names: std::collections::HashSet<String> = user
            .items
            .iter()
            .map(|item| item.package_name.clone())
            .collect();
        assert_eq!(
            agent_names, legacy_names,
            "Agent 与 Legacy 三方包集合必须一致（差异即迁移缺陷）"
        );

        let all = client
            .request::<_, PackageListResult>(
                agent_protocol::method::PACKAGE_LIST,
                &PackageListParams {
                    scope: PackageScope::All,
                    include_disabled: true,
                },
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        assert!(
            all.items.len() >= user.items.len(),
            "include_disabled 不得减少条目"
        );
        assert!(
            all.items.iter().any(|item| !item.enabled),
            "真机存在停用系统包，必须能报 enabled=false"
        );
        assert!(
            all.items.iter().any(|item| item.is_system),
            "scope=all 应包含系统包"
        );
        eprintln!(
            "[package.list] user={} all_with_disabled={}",
            user.items.len(),
            all.items.len()
        );
    }

    #[tokio::test]
    #[ignore = "需要真机；AR4_TEST_SERIAL=<serial> cargo test real_device_info_matches_legacy_and_reports_latency -- --ignored --nocapture"]
    async fn real_device_info_matches_legacy_and_reports_latency() {
        let serial = std::env::var("AR4_TEST_SERIAL").expect("AR4_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let artifacts = Arc::new(AgentArtifactResolver::new(config, None));
        let manager = AgentManager::new(runner.clone(), artifacts);

        let connect_started = Instant::now();
        let status = manager.connect_resolved(&serial).await.unwrap();
        let connect_ms = connect_started.elapsed().as_millis();
        assert!(
            status
                .capabilities
                .iter()
                .any(|capability| capability.method == agent_protocol::method::DEVICE_INFO)
        );
        let client = manager.client(&serial).unwrap();
        let mut timings = Vec::new();
        let mut agent_info = None;
        for _ in 0..20 {
            let started = Instant::now();
            let info = client
                .request::<_, DeviceInfoResult>(
                    agent_protocol::method::DEVICE_INFO,
                    &DeviceInfoParams {},
                    Duration::from_secs(5),
                )
                .await
                .unwrap();
            timings.push(started.elapsed().as_micros());
            agent_info = Some(info);
        }
        timings.sort_unstable();
        let agent_info = agent_info.unwrap();

        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();
        let getprop = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_getprop()),
                Duration::from_secs(8),
            )
            .await
            .unwrap();
        assert_eq!(getprop.exit_code, Some(0));
        let properties = adb::parse_getprop(&getprop.stdout);
        assert_eq!(
            agent_info.model.as_deref(),
            properties.get("ro.product.model").map(String::as_str)
        );
        assert_eq!(
            agent_info.manufacturer.as_deref(),
            properties
                .get("ro.product.manufacturer")
                .map(String::as_str)
        );
        assert_eq!(
            agent_info.android_version.as_deref(),
            properties
                .get("ro.build.version.release")
                .map(String::as_str)
        );
        assert_eq!(
            agent_info.api_level,
            properties
                .get("ro.build.version.sdk")
                .and_then(|value| value.parse().ok())
        );
        assert_eq!(
            agent_info.primary_abi.as_deref(),
            properties.get("ro.product.cpu.abi").map(String::as_str)
        );
        let ip = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_ip_addr()),
                Duration::from_secs(8),
            )
            .await
            .unwrap();
        let legacy_ip = (ip.exit_code == Some(0))
            .then(|| adb::parse_wlan0_ip(&ip.stdout))
            .flatten();
        assert_eq!(agent_info.wlan_ipv4, legacy_ip);
        eprintln!(
            "[real device.info] connect_ms={connect_ms} requests=20 p50_us={} p95_us={} max_us={}",
            timings[timings.len() / 2],
            timings[timings.len() * 95 / 100],
            timings[timings.len() - 1]
        );
        manager.disconnect(&serial).await.unwrap();
    }
}

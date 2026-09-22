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
                legacy_fallbacks: Vec::new(),
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
                legacy_fallbacks: Vec::new(),
                routes: Vec::new(),
            };
        };
        match client.health(REUSE_HEALTH_TIMEOUT).await {
            Ok(health) => AgentDiagnostics {
                status: session.status(),
                health: Some(health),
                health_error: None,
                legacy_fallbacks: Vec::new(),
                routes: Vec::new(),
            },
            Err(error) => {
                *session.client.lock().expect("agent client lock poisoned") = None;
                session.fail(AgentSessionState::Disconnected, error.to_string());
                AgentDiagnostics {
                    status: session.status(),
                    health: None,
                    health_error: Some(error.to_string()),
                    legacy_fallbacks: Vec::new(),
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
                probe_pending: false,
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

    /// AR6.3 真机腿：`process.kill` 的身份拒止、幂等与 root 边界。
    ///
    /// 全程只用 shell 自属进程（Agent 也是 shell 启动），验证的是**语义**而不是权限：
    /// 名字对不上必须拒止且进程还活着；名字对得上必须真杀掉；重复杀是幂等成功；
    /// 声明需要 root 时必须显式拒绝而不是先去 `kill` 一下看运气。
    #[tokio::test]
    #[ignore = "需要真机；AR6_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_process_kill -- --ignored --nocapture"]
    async fn real_agent_process_kill_refuses_mismatch_and_is_idempotent() {
        use agent_protocol::method::PROCESS_KILL;
        use agent_protocol::{
            AgentError, ErrorCode, KillOutcome, KillSignal, ProcessKillParams, ProcessKillResult,
        };

        fn agent_error(error: crate::services::agent_client::AgentClientError) -> AgentError {
            match error {
                crate::services::agent_client::AgentClientError::Remote(error) => error,
                other => panic!("期望 Agent 结构化错误，实际 {other:?}"),
            }
        }

        const PROBE_PORT: u16 = 24568;

        async fn adb_shell(serial: &str, command: &str) -> String {
            let output = tokio::process::Command::new("adb")
                .args(["-s", serial, "shell", command])
                .output()
                .await
                .expect("adb shell 可用");
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        let serial = std::env::var("AR6_TEST_SERIAL").expect("AR6_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        manager.connect_resolved(&serial).await.unwrap();
        let client = manager.client(&serial).unwrap();

        let started = adb_shell(
            &serial,
            &format!("nohup toybox nc -4 -L -s 127.0.0.1 -p {PROBE_PORT} </dev/null >/dev/null 2>&1 & echo ok"),
        )
        .await;
        assert!(started.contains("ok"), "未能拉起探测进程: {started}");
        tokio::time::sleep(Duration::from_millis(800)).await;
        let pid: u32 = adb_shell(
            &serial,
            &format!("pgrep -f 'nc -4 -L -s 127.0.0.1 -p {PROBE_PORT}'"),
        )
        .await
        .lines()
        .next()
        .and_then(|line| line.trim().parse().ok())
        .expect("探测进程应在运行");

        // ① 身份不符：必须 precondition_failed 拒止，且进程仍然存活
        let mismatch = client
            .request::<_, ProcessKillResult>(
                PROCESS_KILL,
                &ProcessKillParams {
                    pid,
                    expected_comm: Some("definitely-not-this".into()),
                    signal: KillSignal::Kill,
                    require_root: false,
                },
                Duration::from_secs(10),
            )
            .await
            .expect_err("PID 复用场景必须拒止，不能硬发信号");
        let mismatch = agent_error(mismatch);
        assert_eq!(mismatch.code, ErrorCode::PreconditionFailed, "{mismatch:?}");
        let details = mismatch.details.expect("拒止必须带回身份证据");
        assert_eq!(details["reason"], "identity_mismatch");
        assert_eq!(details["expected"], "definitely-not-this");
        assert_eq!(details["actual_comm"], "toybox");
        assert!(
            adb_shell(&serial, &format!("kill -0 {pid} 2>/dev/null && echo alive"))
                .await
                .contains("alive"),
            "拒止后进程必须还活着"
        );

        // ② 声明需要 root 而 Agent 是 shell：显式 permission_denied，不去试 kill
        let rootless = client
            .request::<_, ProcessKillResult>(
                PROCESS_KILL,
                &ProcessKillParams {
                    pid,
                    expected_comm: Some("toybox".into()),
                    signal: KillSignal::Kill,
                    require_root: true,
                },
                Duration::from_secs(10),
            )
            .await
            .expect_err("Agent 以 shell 运行，不能假装满足 root 要求");
        let rootless = agent_error(rootless);
        assert_eq!(rootless.code, ErrorCode::PermissionDenied, "{rootless:?}");
        assert!(
            adb_shell(&serial, &format!("kill -0 {pid} 2>/dev/null && echo alive"))
                .await
                .contains("alive"),
            "root 拒止后进程必须还活着"
        );

        // ③ 正常终止：signaled + verified_dead，且 shell 复核确实没了
        let killed: ProcessKillResult = client
            .request(
                PROCESS_KILL,
                &ProcessKillParams {
                    pid,
                    expected_comm: Some("toybox".into()),
                    signal: KillSignal::Kill,
                    require_root: false,
                },
                Duration::from_secs(10),
            )
            .await
            .expect("终止 shell 自属进程应成功");
        eprintln!(
            "[process.kill] pid={} comm={:?} uid={:?} outcome={:?} verified_dead={} detail={:?}",
            killed.pid,
            killed.comm,
            killed.uid,
            killed.outcome,
            killed.verified_dead,
            killed.detail
        );
        assert_eq!(killed.outcome, KillOutcome::Signaled);
        assert_eq!(killed.comm.as_deref(), Some("toybox"));
        assert!(killed.verified_dead, "SIGKILL 后应能确认进程消失");
        assert!(!killed.ran_as_root);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !adb_shell(
                &serial,
                &format!("pgrep -f 'nc -4 -L -s 127.0.0.1 -p {PROBE_PORT}'")
            )
            .await
            .contains(&pid.to_string()),
            "设备上探测进程应已退出"
        );

        // ④ 幂等：再杀一次是 already_gone，不报错
        let again: ProcessKillResult = client
            .request(
                PROCESS_KILL,
                &ProcessKillParams {
                    pid,
                    expected_comm: Some("toybox".into()),
                    signal: KillSignal::Term,
                    require_root: false,
                },
                Duration::from_secs(10),
            )
            .await
            .expect("重复终止应作为幂等成功返回");
        assert_eq!(again.outcome, KillOutcome::AlreadyGone);
        assert_eq!(again.signal, KillSignal::Term);

        // ⑤ 保留给未来的危险输入：pid=0（kill(2) 语义是整组）与 pid=1 必须被拒
        for pid in [0_u32, 1] {
            let refused = client
                .request::<_, ProcessKillResult>(
                    PROCESS_KILL,
                    &ProcessKillParams {
                        pid,
                        expected_comm: None,
                        signal: KillSignal::Kill,
                        require_root: false,
                    },
                    Duration::from_secs(10),
                )
                .await
                .expect_err(&format!("pid={pid} 不得接受"));
            let refused = agent_error(refused);
            assert_eq!(
                refused.code,
                ErrorCode::InvalidRequest,
                "pid={pid}: {refused:?}"
            );
        }
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR8.1 真机腿：包写操作的语义只能在真机上验全——幂等台账、目标范围守卫、
    /// 执行后客观复核（pidof / pm path）。本腿**不做任何真实卸载**：卸载只验证
    /// 「系统包必须被拒」和「没装的包必须报没装」两条守卫，等用户指定可牺牲的靶子包
    /// 之后再补成功路径。启动/强停用 `com.termux`（终端类应用，强停等于上滑划掉，无数据损失）。
    #[tokio::test]
    #[ignore = "需要真机；AR8_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_package_writes -- --ignored --nocapture"]
    async fn real_agent_package_writes_are_idempotent_and_scoped() {
        use agent_protocol::method::{ACTIVITY_FORCE_STOP, ACTIVITY_LAUNCH, PACKAGE_UNINSTALL};
        use agent_protocol::{
            ActivityForceStopParams, ActivityLaunchParams, ErrorCode, PackageUninstallParams,
            PackageWriteResult, WriteOutcome,
        };

        // ⚠️ 本腿会真的**启动并强停**一个应用，所以靶子必须是「人明确给过的」而不是
        // 从装机列表里自动挑——那台机上可能有微信、银行 App。默认仍用 Termux（一台
        // 长期做实验机的终端工具，启停它没有副作用）；换设备时用
        // `AR8_WRITE_TARGET=<包名>` 指定你允许启停的应用。
        let target = std::env::var("AR8_WRITE_TARGET").unwrap_or_else(|_| "com.termux".into());
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let serial = std::env::var("AR8_TEST_SERIAL").expect("AR8_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        let status = manager.connect_resolved(&serial).await.unwrap();
        for method in [ACTIVITY_LAUNCH, ACTIVITY_FORCE_STOP, PACKAGE_UNINSTALL] {
            assert!(
                status
                    .capabilities
                    .iter()
                    .any(|capability| capability.method == method && capability.available),
                "Agent 未发布 {method}"
            );
        }
        let client = manager.client(&serial).unwrap();

        // 前置：靶子必须装在这台机上。缺应用是环境条件，不是代码回归——自证并说清缺什么
        let probe = tokio::process::Command::new("adb")
            .args(["-s", &serial, "shell", "pm list packages"])
            .arg(&target)
            .output()
            .await
            .expect("adb 可用");
        let installed = String::from_utf8_lossy(&probe.stdout)
            .lines()
            .any(|line| line.trim() == format!("package:{target}"));
        if !installed {
            eprintln!(
                "[跳过] 设备 {serial} 上没有 {target}；本腿默认靶子是 Termux，换设备请给 AR8_WRITE_TARGET=<你允许启停的包名>"
            );
            manager.disconnect(&serial).await.unwrap();
            return;
        }
        // 前置二（vivo 上真跑出来的）：装了 ≠ 能启动。`bin.mt.termex` 装了但没有
        // LAUNCHER 入口，Agent 如实报 no_launcher_activity，而腿当时把它当回归失败。
        // 环境不满足就说环境不满足，别伪装成代码问题。
        let entry = tokio::process::Command::new("adb")
            .args([
                "-s",
                &serial,
                "shell",
                "cmd package resolve-activity --brief -c android.intent.category.LAUNCHER",
            ])
            .arg(&target)
            .output()
            .await
            .expect("adb 可用");
        let entry_text = String::from_utf8_lossy(&entry.stdout).into_owned();
        if entry_text.contains("No activity found") || !entry_text.contains(&target) {
            eprintln!(
                "[跳过] {target} 在 {serial} 上没有 LAUNCHER 入口，启停成功路径无法验证: {entry_text}"
            );
            manager.disconnect(&serial).await.unwrap();
            return;
        }
        eprintln!("[ar8.1] 启停靶子 = {target}（LAUNCHER 入口已确认存在）");

        // ① 启动：必须看到 pid，否则 verified=false
        let launch_op = format!("ar81-launch-{stamp}");
        let launched: PackageWriteResult = client
            .request(
                ACTIVITY_LAUNCH,
                &ActivityLaunchParams {
                    package: target.clone(),
                    operation_id: launch_op.clone(),
                },
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        eprintln!(
            "[activity.launch] outcome={:?} verified={} pid={:?} detail={:?}",
            launched.outcome, launched.verified, launched.pid, launched.detail
        );
        assert_eq!(launched.outcome, WriteOutcome::Executed);
        assert!(launched.verified, "启动后必须复核到 pid");
        assert!(launched.pid.unwrap_or(0) > 0);
        assert!(!launched.ran_as_root);

        // ② 幂等：同一 operation_id 重发只回已知结果，不二次执行
        let replay: PackageWriteResult = client
            .request(
                ACTIVITY_LAUNCH,
                &ActivityLaunchParams {
                    package: target.clone(),
                    operation_id: launch_op.clone(),
                },
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        assert_eq!(replay.outcome, WriteOutcome::Replayed, "重发不得再次执行");
        assert_eq!(replay.pid, launched.pid, "复用结果必须与上次一致");

        // ③ 强停：先看到 pidof 消失，再复核一次；两次都要能分辨 executed / no_op
        let stop_op = format!("ar81-stop-{stamp}");
        let stopped: PackageWriteResult = client
            .request(
                ACTIVITY_FORCE_STOP,
                &ActivityForceStopParams {
                    package: target.clone(),
                    operation_id: stop_op.clone(),
                },
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        eprintln!(
            "[activity.force_stop] outcome={:?} verified={} pid_before={:?} detail={:?}",
            stopped.outcome, stopped.verified, stopped.pid, stopped.detail
        );
        assert_eq!(stopped.outcome, WriteOutcome::Executed);
        assert!(stopped.verified, "强停后必须复核到进程消失");
        assert_eq!(stopped.detail.as_deref(), Some("pid_gone"));
        assert!(
            !adb_shell_is_alive(&serial, &target).await,
            "设备上 {} 应已不在运行",
            &target
        );

        // ④ 已经停了再停一次：no_op（幂等），不是错误
        let again: PackageWriteResult = client
            .request(
                ACTIVITY_FORCE_STOP,
                &ActivityForceStopParams {
                    package: target.clone(),
                    operation_id: format!("ar81-stop-again-{stamp}"),
                },
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        assert_eq!(again.outcome, WriteOutcome::NoOp);
        assert!(again.verified);
        assert_eq!(again.detail.as_deref(), Some("not_running"));

        // ⑤ 目标范围守卫：系统包一律拒止（settings 连强停都不许，卸载更不行）。
        // 三个方法共用一段断言，避免「只测了一个入口的守卫」。
        for method in [ACTIVITY_FORCE_STOP, ACTIVITY_LAUNCH, PACKAGE_UNINSTALL] {
            let params = serde_json::json!({
                "package": "com.android.settings",
                "operation_id": format!("ar81-guard-{method}-{stamp}"),
            });
            let error = client
                .request::<_, PackageWriteResult>(method, &params, Duration::from_secs(15))
                .await
                .expect_err("系统包不允许写操作");
            let error = match error {
                crate::services::agent_client::AgentClientError::Remote(error) => error,
                other => panic!("期望结构化错误，实际 {other:?}"),
            };
            assert_eq!(
                error.code,
                ErrorCode::PreconditionFailed,
                "{method}: {error:?}"
            );
            assert_eq!(error.details.unwrap()["reason"], "system_package_protected");
        }

        // ⑥ 没装的包：not_found，而不是「没权限动系统包」这种误导
        let missing = client
            .request::<_, PackageWriteResult>(
                PACKAGE_UNINSTALL,
                &PackageUninstallParams {
                    package: "com.definitely.not.installed.pkg".into(),
                    operation_id: format!("ar81-missing-{stamp}"),
                    keep_data: false,
                    user: None,
                },
                Duration::from_secs(15),
            )
            .await
            .expect_err("未安装的包必须报 not_found");
        let missing = match missing {
            crate::services::agent_client::AgentClientError::Remote(error) => error,
            other => panic!("期望结构化错误，实际 {other:?}"),
        };
        assert_eq!(missing.code, ErrorCode::NotFound);
        assert_eq!(missing.details.unwrap()["reason"], "package_not_installed");

        // 收尾：把目标应用留在「已安装、未运行」的自然状态
        manager.disconnect(&serial).await.unwrap();
    }

    /// `pidof <pkg>` 是否有输出（走 Agent 的 shell 无关判定用 adb 直查）。
    async fn adb_shell_is_alive(serial: &str, package: &str) -> bool {
        let output = tokio::process::Command::new("adb")
            .args(["-s", serial, "shell", "pidof"])
            .arg(package)
            .output()
            .await
            .expect("adb 可用");
        !String::from_utf8_lossy(&output.stdout).trim().is_empty()
    }

    /// AR8.3 真机腿：`package.native_lib_dir` 必须与 Legacy 的 dumpsys 解析同结论，
    /// 而且要把 Legacy 只能报错的两种情况（framework 形态目录、多实例块）分开说清。
    #[tokio::test]
    #[ignore = "需要真机；AR8_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_native_lib_dir -- --ignored --nocapture"]
    async fn real_agent_native_lib_dir_matches_legacy_and_explains_itself() {
        use agent_protocol::method::PACKAGE_NATIVE_LIB_DIR;
        use agent_protocol::{
            ErrorCode, NativeLibDirSource, PackageNativeLibDirParams, PackageNativeLibDirResult,
        };

        let serial = std::env::var("AR8_TEST_SERIAL").expect("AR8_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        let status = manager.connect_resolved(&serial).await.unwrap();
        assert!(
            status
                .capabilities
                .iter()
                .any(|capability| capability.method == PACKAGE_NATIVE_LIB_DIR
                    && capability.available),
            "Agent 未发布 package.native_lib_dir"
        );
        let client = manager.client(&serial).unwrap();
        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();

        let dumpsys = |pkg: &str| {
            let runner = runner.clone();
            let adb_path = adb_path.clone();
            let serial = serial.clone();
            let pkg = pkg.to_string();
            async move {
                runner
                    .run(
                        &adb_path,
                        &adb::build_args(
                            Some(&serial),
                            &adb::cmd_shell(&format!("dumpsys package {pkg}")),
                        ),
                        Duration::from_secs(20),
                    )
                    .await
                    .unwrap()
                    .stdout
            }
        };

        // ① 普通三方应用：与 Legacy 换算逐项一致（arm64 与 arm 两个方向）。
        // 样本从**这台机器真实装着的应用**里挑，不把某台机器的装机列表写死进腿里——
        // 换台设备就跑不动的腿，等于只在一条机器上验证过（M1 要求两台，正是要防这个）。
        let listed = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_shell("pm list packages -3")),
                Duration::from_secs(20),
            )
            .await
            .unwrap()
            .stdout;
        let mut candidates: Vec<String> = listed
            .lines()
            .filter_map(|line| line.trim().strip_prefix("package:"))
            .map(str::to_owned)
            .filter(|pkg| adb::is_safe_pkg_name(pkg))
            .collect();
        candidates.sort();
        let mut samples: Vec<String> = Vec::new();
        for pkg in candidates {
            if samples.len() >= 3 {
                break;
            }
            // 只收「dumpsys 里真有 legacyNativeLibraryDir」的包：这类才有换算可比性
            if crate::services::env_service::parse_legacy_native_lib(&dumpsys(&pkg).await).is_some()
            {
                samples.push(pkg);
            }
        }
        assert!(
            !samples.is_empty(),
            "这台设备上一个可作样本的第三方应用都没有，本腿无法对照"
        );
        eprintln!("[ar8.3] 本机样本包: {samples:?}");
        for pkg in &samples {
            for abi in ["arm64", "arm"] {
                let params = PackageNativeLibDirParams {
                    package: pkg.clone(),
                    abi: Some(abi.into()),
                    user: None,
                };
                let result: PackageNativeLibDirResult = client
                    .request(PACKAGE_NATIVE_LIB_DIR, &params, Duration::from_secs(15))
                    .await
                    .unwrap();
                let legacy = adb::lib_dir_for_abi(
                    &crate::services::env_service::parse_legacy_native_lib(&dumpsys(pkg).await)
                        .expect("样本入选时已确认有 legacyNativeLibraryDir"),
                    abi,
                )
                .expect("对照样本应能按 ABI 换算");
                assert_eq!(
                    result.native_lib_dir, legacy,
                    "{pkg}/{abi} 必须与 Legacy 一致"
                );
                assert_eq!(result.abi, abi);
                assert_eq!(result.source, NativeLibDirSource::FrameworkField);
                assert!(result.code_path.is_some(), "codePath 应一并回传");
                assert!(result.primary_cpu_abi.is_some());
                eprintln!(
                    "[native_lib_dir] {pkg}/{abi} = {} splits={} detail={:?}",
                    result.native_lib_dir,
                    result.splits.len(),
                    result.detail
                );
            }
        }

        // ② splits 形状与 user 回显：同样只对**本机真实装着的包**下断言。
        // 契约是「splits 是数组而非 Option」：非空就必须含 base（那是 Framework 给的
        // 真实拆分清单），为空也必须真的是「看过、确实没有」——两种设备形态不同，
        // 但两条都不能把「没看」冒充成「没有」。
        for pkg in &samples {
            let probe = client
                .request::<_, PackageNativeLibDirResult>(
                    PACKAGE_NATIVE_LIB_DIR,
                    &PackageNativeLibDirParams {
                        package: pkg.clone(),
                        abi: None,
                        user: Some(0),
                    },
                    Duration::from_secs(15),
                )
                .await
                .unwrap();
            assert_eq!(probe.user, Some(0), "user 要回显，调用方才知道查的是谁");
            assert!(
                probe.abi == "arm64" || probe.abi == "arm",
                "ABI 缺省时要按 primaryCpuAbi 落成一个确定值，实际 {:?}",
                probe.abi
            );
            if probe.splits.is_empty() {
                eprintln!("[native_lib_dir] {pkg} 无 split（本机形态，空数组=确实没有）");
            } else {
                assert!(
                    probe.splits.iter().any(|split| split == "base"),
                    "{pkg} 的 splits 必须带 base，实际 {:?}",
                    probe.splits
                );
                eprintln!("[native_lib_dir] {pkg} splits={:?}", probe.splits);
            }
        }

        // ③ framework 包：Legacy 只能报「lib 目录结构异常」，Agent 按原值返回并说明没换算
        let framework = client
            .request::<_, PackageNativeLibDirResult>(
                PACKAGE_NATIVE_LIB_DIR,
                &PackageNativeLibDirParams {
                    package: "android".into(),
                    abi: Some("arm64".into()),
                    user: None,
                },
                Duration::from_secs(15),
            )
            .await
            .expect("framework 包不应被当成错误");
        assert!(
            framework.native_lib_dir.contains("/lib64/")
                || framework.native_lib_dir.ends_with("/lib"),
            "framework 包应按 Framework 原值返回，实际 {}",
            framework.native_lib_dir
        );
        eprintln!(
            "[native_lib_dir] android = {} source={:?} detail={:?} primary={:?} splits={}",
            framework.native_lib_dir,
            framework.source,
            framework.detail,
            framework.primary_cpu_abi,
            framework.splits.len()
        );
        assert_eq!(framework.source, NativeLibDirSource::FrameworkField);
        assert_eq!(
            framework.detail.as_deref(),
            Some("native_lib_dir_not_substitutable"),
            "没做 ABI 换算必须显式说明: {:?}",
            framework.detail
        );
        assert!(
            adb::lib_dir_for_abi(&framework.native_lib_dir, "arm64").is_none(),
            "样本必须真的是 Legacy 会报错的形态，否则这条断言没意义"
        );
        let raw = dumpsys("android").await;
        assert_eq!(
            crate::services::env_service::parse_legacy_native_lib(&raw).as_deref(),
            Some(framework.native_lib_dir.as_str()),
            "必须确认回传的就是 Framework dumpsys 里的原值"
        );

        // ④ 未安装与非法 ABI：都是可分辨的结构化错误
        let missing = client
            .request::<_, PackageNativeLibDirResult>(
                PACKAGE_NATIVE_LIB_DIR,
                &PackageNativeLibDirParams {
                    package: "com.definitely.not.installed.pkg".into(),
                    abi: None,
                    user: None,
                },
                Duration::from_secs(15),
            )
            .await
            .expect_err("未安装的包必须报错");
        let missing = match missing {
            crate::services::agent_client::AgentClientError::Remote(error) => error,
            other => panic!("期望结构化错误，实际 {other:?}"),
        };
        assert_eq!(missing.code, ErrorCode::NotFound);
        assert_eq!(missing.details.unwrap()["reason"], "package_not_found");

        let bad_abi = client
            .request::<_, PackageNativeLibDirResult>(
                PACKAGE_NATIVE_LIB_DIR,
                &PackageNativeLibDirParams {
                    package: "com.android.chrome".into(),
                    abi: Some("x86".into()),
                    user: None,
                },
                Duration::from_secs(15),
            )
            .await
            .expect_err("非法 ABI 必须拒");
        let bad_abi = match bad_abi {
            crate::services::agent_client::AgentClientError::Remote(error) => error,
            other => panic!("期望结构化错误，实际 {other:?}"),
        };
        assert_eq!(bad_abi.code, ErrorCode::InvalidRequest);
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR8.1 卸载成功路径真机腿（**破坏性**：会卸掉指定应用）。
    ///
    /// 双重门控：必须同时给 `AR8_UNINSTALL_TARGET=<包名>` 与 `AR8_UNINSTALL_CONFIRM=yes`
    /// 才会执行，否则打印跳过原因并返回。这样它既不会被 CI 误跑，也不会被忘了参数的
    /// 本地 `--ignored` 全量跑误伤——卸载是不可逆动作，门控必须是显式的。
    /// 用 `keep_data=true` 卸：数据目录保留，用户从应用商店重装即恢复原状。
    #[tokio::test]
    #[ignore = "破坏性真机腿；AR8_TEST_SERIAL=<serial> AR8_UNINSTALL_TARGET=<pkg> AR8_UNINSTALL_CONFIRM=yes cargo test -p app-reverse-tools real_agent_package_uninstall -- --ignored --nocapture"]
    async fn real_agent_package_uninstall_success_path_and_guard() {
        use agent_protocol::method::{PACKAGE_NATIVE_LIB_DIR, PACKAGE_UNINSTALL};
        use agent_protocol::{
            ErrorCode, PackageNativeLibDirParams, PackageNativeLibDirResult,
            PackageUninstallParams, PackageUninstallResult, WriteOutcome,
        };

        let target = std::env::var("AR8_UNINSTALL_TARGET").unwrap_or_default();
        let confirm = std::env::var("AR8_UNINSTALL_CONFIRM").unwrap_or_default();
        if target.is_empty() || confirm != "yes" {
            eprintln!(
                "[跳过] 卸载真机腿需要 AR8_UNINSTALL_TARGET=<包名> 且 AR8_UNINSTALL_CONFIRM=yes（当前 target={target:?} confirm={confirm:?}）"
            );
            return;
        }

        let serial = std::env::var("AR8_TEST_SERIAL").expect("AR8_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        manager.connect_resolved(&serial).await.unwrap();
        let client = manager.client(&serial).unwrap();

        let adb = |command: String| {
            let serial = serial.clone();
            async move {
                let output = tokio::process::Command::new("adb")
                    .args(["-s", &serial, "shell", &command])
                    .output()
                    .await
                    .expect("adb shell 可用");
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
        };

        // 前置：这台机上它必须是「已安装的第三方应用」，否则实验没有意义
        let listed: PackageNativeLibDirResult = client
            .request(
                PACKAGE_NATIVE_LIB_DIR,
                &PackageNativeLibDirParams {
                    package: target.clone(),
                    abi: None,
                    user: None,
                },
                Duration::from_secs(15),
            )
            .await
            .expect("卸载前目标必须是已安装的第三方应用");
        assert!(listed.code_path.is_some(), "目标包应能取到 codePath");

        // ① 真卸载：executed + verified + pm path 消失
        let operation_id = format!("uninstall-{}", std::process::id());
        let params = PackageUninstallParams {
            package: target.clone(),
            operation_id: operation_id.clone(),
            keep_data: true,
            user: None,
        };
        let result: PackageUninstallResult = client
            .request(PACKAGE_UNINSTALL, &params, Duration::from_secs(60))
            .await
            .expect("卸载应成功返回");
        eprintln!(
            "[package.uninstall] outcome={:?} verified={} steps={:?} detail={:?}",
            result.outcome, result.verified, result.steps, result.detail
        );
        assert_eq!(result.outcome, WriteOutcome::Executed);
        assert!(result.verified, "必须复核到 pm path 消失");
        assert_eq!(
            result.steps.iter().filter(|step| step.ok).count(),
            result.steps.len(),
            "成功路径每一步都该 ok，实际 {:?}",
            result.steps
        );
        // 判「还在不在」只能用 `package:` 前缀：第三方包路径里天然带 `==`，
        // 系统包路径又不带 `=`——拿字符猜会两头都判错（真机踩过的坑）。
        let path_out = adb(format!("pm path {target}")).await;
        let path_lines: Vec<&str> = path_out
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("package:"))
            .collect();
        assert!(
            path_lines.is_empty(),
            "设备上 pm path 仍指向安装包: {path_lines:?}"
        );
        assert!(
            !adb(format!("pm list packages {target}"))
                .await
                .contains(&target)
        );

        // ② keep_data=true 的效果必须看得见：数据目录还在（重装即恢复原状）
        let data_kept = adb(format!(
            "su -c 'if [ -d /data/data/{target} ]; then echo kept; fi'"
        ))
        .await
        .contains("kept");
        eprintln!("[package.uninstall] keep_data 生效={data_kept}");
        assert!(
            data_kept,
            "keep_data=true 却把数据删了，等于骗用户可无损重装"
        );

        // ③ 同一 operation_id 重发：replayed，不得二次执行
        let replay: PackageUninstallResult = client
            .request(PACKAGE_UNINSTALL, &params, Duration::from_secs(30))
            .await
            .expect("重发应返回已知结果");
        assert_eq!(replay.outcome, WriteOutcome::Replayed);
        assert_eq!(replay.steps, result.steps, "复用结果必须与首次一致");

        // ④ 换个 id 再卸：not_found（已经没了，不是「不许动」）
        let gone = client
            .request::<_, PackageUninstallResult>(
                PACKAGE_UNINSTALL,
                &PackageUninstallParams {
                    package: target.clone(),
                    operation_id: format!("{operation_id}-again"),
                    keep_data: true,
                    user: None,
                },
                Duration::from_secs(30),
            )
            .await
            .expect_err("已卸载的包必须报 not_found");
        let gone = match gone {
            crate::services::agent_client::AgentClientError::Remote(error) => error,
            other => panic!("期望结构化错误，实际 {other:?}"),
        };
        assert_eq!(gone.code, ErrorCode::NotFound);
        assert_eq!(gone.details.unwrap()["reason"], "package_not_installed");
        eprintln!("[提示] {target} 已卸载（数据保留）。要恢复原状：在 Play 商店点重装即可。");
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR8.4 SO 替换真机腿（**破坏性**：往目标包的安装目录写文件）。
    ///
    /// 验证姿势按用户指定实现，一步不改：
    /// ① 从目标 APK 里**解出**一个真实 `.so`；② 打一个**可控**补丁——只改
    ///    `.note.gnu.build-id` 里的一个字节，长度不变、指令不变，所以 App 行为不变；
    /// ③ 走产品链路装进 `<native_lib_dir>/<so>`；④ 判「换没换成」不看命令返回值，
    ///    看安卓自己的机制：冷启动后 `/proc/<pid>/maps` 里这个路径的映射来源——
    ///    `extractNativeLibs=false` 的应用原本只会映射到 `split_config.*.apk`，
    ///    装成功后必须出现来自**我们那个文件**的映射（偏移从 0 开始）；
    /// ⑤ 删掉文件再启动，maps 必须回到原来的命中数——证明信号由本次替换造成。
    /// 另外钉两条：目标 sha256 必须等于补丁件（内容真的换了）、非 ELF 暂存件必须被拒
    /// 且不留下任何文件（失败不影响 App）。
    #[tokio::test]
    #[ignore = "破坏性真机腿；AR8_TEST_SERIAL=<serial> AR84_TARGET_PKG=<pkg> AR84_SO=<libfoo.so> AR84_CONFIRM=yes cargo test -p app-reverse-tools real_agent_replace_native_library -- --ignored --nocapture"]
    async fn real_agent_replace_native_library_is_verified_by_proc_maps() {
        use agent_protocol::method::{PACKAGE_NATIVE_LIB_DIR, PACKAGE_REPLACE_NATIVE_LIBRARY};
        use agent_protocol::{
            ErrorCode, PackageNativeLibDirParams, PackageNativeLibDirResult,
            ReplaceNativeLibraryParams, ReplaceNativeLibraryResult, WriteOutcome,
        };
        use sha2::{Digest, Sha256};

        // 破坏性腿的参数没给就是没授权：安静跳过，不要 panic。
        // 否则 `cargo test -- --ignored` 或回归脚本不带靶子时会报一次假失败，
        // 久了人人都会把这条腿的红色当成噪音。（与卸载腿同一口径。）
        let (serial, pkg, so, confirm) = (
            std::env::var("AR8_TEST_SERIAL").unwrap_or_default(),
            std::env::var("AR84_TARGET_PKG").unwrap_or_default(),
            std::env::var("AR84_SO").unwrap_or_default(),
            std::env::var("AR84_CONFIRM").unwrap_or_default(),
        );
        if serial.is_empty() || pkg.is_empty() || so.is_empty() || confirm != "yes" {
            eprintln!(
                "[跳过] SO 替换腿需要靶子与显式确认：AR8_TEST_SERIAL + AR84_TARGET_PKG + AR84_SO + AR84_CONFIRM=yes（当前 serial={serial} pkg={pkg} so={so} confirm={confirm}）"
            );
            return;
        }
        assert!(adb::is_safe_so_name(&so), "AR84_SO 不合法: {so}");

        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        manager.connect_resolved(&serial).await.unwrap();
        let client = manager.client(&serial).unwrap();
        let adb_path = runner.environment().await.path.expect("本机应有 adb");

        let sh = |command: String| {
            let runner = runner.clone();
            let adb_path = adb_path.clone();
            let serial = serial.clone();
            async move {
                let output = runner
                    .run(
                        &adb_path,
                        &adb::build_args(Some(&serial), &adb::cmd_shell(&command)),
                        Duration::from_secs(30),
                    )
                    .await
                    .unwrap();
                output.stdout
            }
        };
        let root_sh = |command: String| {
            let sh = &sh;
            async move { sh(adb::su_wrap(&command)).await }
        };
        let transport = |subcommand: Vec<String>| {
            let runner = runner.clone();
            let adb_path = adb_path.clone();
            let serial = serial.clone();
            async move {
                runner
                    .run(
                        &adb_path,
                        &adb::build_args(Some(&serial), &subcommand),
                        Duration::from_secs(120),
                    )
                    .await
                    .unwrap()
            }
        };

        // 前置：特权步骤靠 Agent 起 su，这台机必须先给 shell root，否则整条腿没有意义
        let whoami = root_sh("id".to_string()).await;
        assert!(
            adb::is_root_probe_ok(&whoami),
            "AR8.4 需要 root（Agent 内的特权固定脚本要 su），实际: {whoami}"
        );

        // ① 目标目录与 APK：都由 Agent 从包信息推导，腿不自己拼路径
        let native: PackageNativeLibDirResult = client
            .request(
                PACKAGE_NATIVE_LIB_DIR,
                &PackageNativeLibDirParams {
                    package: pkg.clone(),
                    abi: Some("arm64".into()),
                    user: None,
                },
                Duration::from_secs(15),
            )
            .await
            .expect("目标包必须已安装");
        let dir = native.native_lib_dir.clone();
        // ⚠️ `splits` 是 dumpsys 里的**名字**（`config.arm64_v8a`），不是文件路径；
        // 解包要的是真实 APK，所以按 AR8.3 的字段设计去 `pm path` 取路径。
        let paths: Vec<String> = sh(format!("pm path {pkg}"))
            .await
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("package:"))
            .map(str::to_owned)
            .collect();
        let apk = paths
            .iter()
            .find(|path| path.contains("arm64_v8a"))
            .cloned()
            .or_else(|| native.code_path.clone())
            .unwrap_or_else(|| panic!("拿不到可解包的 APK，pm path 返回 {paths:?}"));
        let target = format!("{dir}/{so}");
        eprintln!("[ar8.4] pkg={pkg} dir={dir} apk={apk} target={target}");

        // ② 从 APK 里解出原件（设备侧 unzip → 拉回宿主），确认它确实来自 APK
        let work = format!("/data/local/tmp/ar84-{}", std::process::id());
        root_sh(format!(
            "rm -rf {work}; mkdir -p {work}; unzip -o -q -d {work} \"{apk}\" \"lib/arm64-v8a/{so}\""
        ))
        .await;
        let extracted = format!("{work}/lib/arm64-v8a/{so}");
        let listed = root_sh(format!("ls -l {extracted} 2>&1")).await;
        assert!(
            listed.contains(&so),
            "APK 里解不出 {so}，这条腿的靶子选错了：{listed}"
        );
        root_sh(format!("chmod -R a+rX {work}")).await;
        let host = tempfile::tempdir().unwrap();
        let original_on_host = host.path().join("original.so");
        transport(adb::cmd_pull(
            &extracted,
            &original_on_host.display().to_string(),
        ))
        .await;
        let original = std::fs::read(&original_on_host).expect("pull 回来的原件应可读");
        assert_eq!(&original[..4], b"\x7fELF", "解出来的必须是 ELF");

        // ③ 可控补丁：只动 .note.gnu.build-id 的一个字节（原件保持纯净，⑩ 撤销要用它）
        let original_sha = {
            let mut digest = Sha256::new();
            digest.update(&original);
            format!("{:x}", digest.finalize())
        };
        let mut patched = original.clone();
        let patched_offset = patch_build_id_byte(&mut patched)
            .expect("靶子 so 必须带 .note.gnu.build-id，否则这条腿测的是空操作");
        assert_eq!(
            patched.len(),
            original.len(),
            "修补必须等长，否则不是同一个 so"
        );
        let diffs: Vec<usize> = (0..original.len())
            .filter(|index| original[*index] != patched[*index])
            .collect();
        assert_eq!(
            diffs,
            vec![patched_offset],
            "只允许改 build-id 的最后 1 个字节"
        );
        let expected_sha = {
            let mut digest = Sha256::new();
            digest.update(&patched);
            format!("{:x}", digest.finalize())
        };
        let staged_local = host.path().join(&so);
        std::fs::write(&staged_local, &patched).unwrap();

        // ④ 起点：目标存在吗？maps 现在命中几条？（两种安装形态都要说得清）
        let existed_before = root_sh(format!("test -e {target} && echo YES || echo NO"))
            .await
            .contains("YES");
        let maps_hits = |probe: String| {
            let root_sh = &root_sh;
            async move {
                let out = root_sh(probe).await;
                out.trim()
                    .rsplit('\n')
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .to_owned()
            }
        };
        let probe_hits = |pid: &str| format!("grep -c -F {dir}/{so} /proc/{pid}/maps");
        let restart = || {
            let sh = &sh;
            let pkg = pkg.clone();
            async move {
                sh(format!("am force-stop {pkg}")).await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                sh(format!(
                    "monkey -p {pkg} -c android.intent.category.LAUNCHER 1"
                ))
                .await;
                // 等进程起来（冷启动慢的机器上这不叫放宽，叫不猜）
                for _ in 0..15 {
                    let pid = sh(format!("pidof {pkg}")).await;
                    let pid = pid.split_whitespace().next().unwrap_or("").to_owned();
                    if !pid.is_empty() {
                        tokio::time::sleep(Duration::from_secs(6)).await;
                        return Some(pid);
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                None
            }
        };
        let pid_before = restart().await.expect("目标 App 冷启动必须能起来");
        let hits_before: usize = maps_hits(probe_hits(&pid_before)).await.parse().unwrap();
        eprintln!(
            "[ar8.4] 起点：目标存在={existed_before} maps 命中={hits_before}（extractNativeLibs={}）",
            if hits_before == 0 && !existed_before {
                "false"
            } else {
                "true/未知"
            }
        );

        // ⑤ 装：Desktop 的等价动作 = push 到唯一暂存目录 + 一次 typed 请求
        let operation_id = format!("ar84-{}", std::process::id());
        let staged_dir = format!(
            "{}/{}-{}",
            agent_protocol::SO_STAGED_ROOT,
            pkg,
            &operation_id[operation_id.len() - 8..]
        );
        let staged_path = format!("{staged_dir}/{so}");
        let pushed = transport(adb::cmd_push(
            &staged_local.display().to_string(),
            &staged_path,
        ))
        .await;
        assert_eq!(pushed.exit_code, Some(0), "adb push 暂存件失败");
        let params = ReplaceNativeLibraryParams {
            package: pkg.clone(),
            abi: "arm64".into(),
            so_name: so.clone(),
            staged_path: staged_path.clone(),
            operation_id: operation_id.clone(),
        };
        let result: ReplaceNativeLibraryResult = client
            .request(
                PACKAGE_REPLACE_NATIVE_LIBRARY,
                &params,
                Duration::from_secs(100),
            )
            .await
            .expect("替换应成功返回步骤链");
        eprintln!(
            "[ar8.4] outcome={:?} verified={} replaced_existing={} steps={:?}",
            result.outcome, result.verified, result.replaced_existing, result.steps
        );
        assert_eq!(result.outcome, WriteOutcome::Executed);
        assert!(
            result.verified,
            "sha256 复核必须过，步骤链 {:?}",
            result.steps
        );
        assert_eq!(result.target_path, target);
        assert_eq!(
            result.replaced_existing, existed_before,
            "「原本有没有文件」必须与起点观测一致"
        );
        assert_eq!(result.rolled_back, None, "成功路径不该有回滚记录");
        assert!(
            result.steps.iter().all(|step| step.ok),
            "成功路径每一步都得 ok：{:?}",
            result.steps
        );
        assert_eq!(
            result.backup_path.is_some(),
            existed_before,
            "只有原本存在文件才谈得上备份"
        );

        // ⑥ 内容证据：设备上目标文件的 sha256 == 补丁件的 sha256（不信 Agent 自称复核过）
        let on_device_sha = root_sh(format!("sha256sum {target}"))
            .await
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        assert_eq!(
            on_device_sha, expected_sha,
            "目标内容不是我们那个补丁件——替换是假的"
        );

        // ⑦ 机制证据：冷启动后 maps 里必须出现来自该文件的映射
        let pid_after = restart().await.expect("替换后 App 仍必须能起");
        let hits_after: usize = maps_hits(probe_hits(&pid_after)).await.parse().unwrap();
        eprintln!("[ar8.4] 替换后 maps 命中={hits_after}");
        if !existed_before {
            assert!(
                hits_after >= 1,
                "安卓按「先文件后 APK」找 native lib：装进 {dir} 的 {so} 必须被映射到；\
                 命中 0 说明这个库当前没被加载（换一个 AR84_SO）或替换没生效"
            );
            let detail = root_sh(format!(
                "grep -F {dir}/{so} /proc/{pid_after}/maps | head -3"
            ))
            .await;
            assert!(
                detail.contains("00000000"),
                "来自散装文件的映射首段偏移必须是 0（APK 内加载会是大偏移）：{detail}"
            );
        } else {
            eprintln!("[ar8.4] 目标原本就是已解包的实体文件，机制判据退化为 sha256（⑥ 已过）");
        }

        // ⑧ 幂等：同 operation_id 重发必须是 replayed，且不得再动设备
        let replay: ReplaceNativeLibraryResult = client
            .request(
                PACKAGE_REPLACE_NATIVE_LIBRARY,
                &params,
                Duration::from_secs(30),
            )
            .await
            .expect("重发应返回已知结果");
        assert_eq!(replay.outcome, WriteOutcome::Replayed);
        assert_eq!(replay.steps, result.steps, "复用结果必须与首次一致");

        // ⑨ 非 ELF 暂存件必须被拒，且安装目录不得多出任何东西
        let fake_so = "libar84fake.so";
        let fake_dir = format!("{}/{}-fake", agent_protocol::SO_STAGED_ROOT, pkg);
        let fake_path = format!("{fake_dir}/{fake_so}");
        let fake_local = host.path().join(fake_so);
        std::fs::write(&fake_local, b"#!/system/bin/sh\necho not an elf\n").unwrap();
        transport(adb::cmd_push(&fake_local.display().to_string(), &fake_path)).await;
        let rejected = client
            .request::<_, ReplaceNativeLibraryResult>(
                PACKAGE_REPLACE_NATIVE_LIBRARY,
                &ReplaceNativeLibraryParams {
                    package: pkg.clone(),
                    abi: "arm64".into(),
                    so_name: fake_so.into(),
                    staged_path: fake_path.clone(),
                    operation_id: format!("{operation_id}-fake"),
                },
                Duration::from_secs(30),
            )
            .await
            .expect_err("非 ELF 暂存件必须被拒");
        let rejected = match rejected {
            crate::services::agent_client::AgentClientError::Remote(error) => error,
            other => panic!("期望结构化错误，实际 {other:?}"),
        };
        assert_eq!(rejected.code, ErrorCode::InvalidRequest);
        assert_eq!(rejected.details.unwrap()["reason"], "staged_not_elf");
        // 被拒时 Agent **不动**暂存件（那是调用方的文件，也是「为什么被拒」的证据），
        // 清理由调用方负责——Desktop 的产品路径里就是那句无条件 `rm -f`。
        // 这条腿刻意镜像同样的顺序：先断言安装目录没被污染，再按产品姿势自己扫干净。
        transport(adb::cmd_shell(&format!("rm -f {fake_path}"))).await;
        sh(format!("rm -rf {fake_dir}")).await;
        assert!(
            !root_sh(format!(
                "test -e {fake_dir}/{fake_so} && echo YES || echo NO"
            ))
            .await
            .contains("YES"),
            "调用方按产品姿势清理后，暂存目录不该还有残留"
        );
        assert!(
            !root_sh(format!("test -e {dir}/{fake_so} && echo YES || echo NO"))
                .await
                .contains("YES"),
            "被拒的请求绝不能往安装目录写任何东西——这才是「失败不影响 App」的判据"
        );

        // ⑩ 撤销必须回到「起点」，而且两种安装形态走法不同：
        // · 原本没有这个文件（extractNativeLibs=false，靶子是 APK 内加载）→ 删掉我们加的；
        // · 原本就有（已解包）→ 必须用产品链路把**原件**装回去，顺带证明「覆盖已存在文件」
        //   这条支路也对（replaced_existing=true 且会留下备份件）。
        if existed_before {
            let restore_local = host.path().join("restore.so");
            std::fs::write(&restore_local, &original).unwrap();
            let restore_dir = format!("{}/{}-restore", agent_protocol::SO_STAGED_ROOT, pkg);
            let restore_path = format!("{restore_dir}/{so}");
            transport(adb::cmd_push(
                &restore_local.display().to_string(),
                &restore_path,
            ))
            .await;
            let restored: ReplaceNativeLibraryResult = client
                .request(
                    PACKAGE_REPLACE_NATIVE_LIBRARY,
                    &ReplaceNativeLibraryParams {
                        package: pkg.clone(),
                        abi: "arm64".into(),
                        so_name: so.clone(),
                        staged_path: restore_path.clone(),
                        operation_id: format!("{operation_id}-restore"),
                    },
                    Duration::from_secs(100),
                )
                .await
                .expect("装回原件应成功（这一步决定用户下次启动看到的是不是原样）");
            assert!(restored.verified && restored.replaced_existing);
            assert!(restored.backup_path.is_some(), "覆盖已存在文件必须先备份");
            let back_sha = root_sh(format!("sha256sum {target}"))
                .await
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
            assert_eq!(back_sha, original_sha, "装回去的必须是原件本体");
            root_sh(format!("rm -rf {restore_dir}")).await;
            transport(adb::cmd_shell(&format!("rm -f {restore_path}"))).await;
        } else {
            root_sh(format!("rm -f {target}; sync")).await;
        }
        let pid_reverted = restart().await.expect("撤销后 App 仍必须能起");
        let hits_reverted: usize = maps_hits(probe_hits(&pid_reverted)).await.parse().unwrap();
        eprintln!("[ar8.4] 撤销后 maps 命中={hits_reverted}（起点 {hits_before}）");
        assert_eq!(
            hits_reverted, hits_before,
            "撤销后必须回到起点的加载形态，否则前面的判据不算因果"
        );
        if existed_before {
            let final_sha = root_sh(format!("sha256sum {target}"))
                .await
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned();
            assert_eq!(final_sha, original_sha, "设备侧终态必须与实验前逐字节一致");
        } else {
            assert!(
                !root_sh(format!("test -e {target} && echo YES || echo NO"))
                    .await
                    .contains("YES"),
                "实验前没有的文件，实验后也不该在"
            );
        }

        // 清理：暂存目录（含备份件）与解包工作目录
        root_sh(format!("rm -rf {staged_dir} {fake_dir} {work}; sync")).await;
        sh(format!("rm -f {staged_path} {fake_path}")).await;
        let leftovers = root_sh(format!(
            "ls -A {}/ 2>/dev/null | grep -c -F {pkg} || true",
            agent_protocol::SO_STAGED_ROOT
        ))
        .await;
        assert_eq!(
            leftovers.trim().rsplit('\n').next().unwrap_or("0").trim(),
            "0",
            "腿跑完不该在暂存根目录留下任何本次痕迹"
        );
        manager.disconnect(&serial).await.unwrap();
        eprintln!("[ar8.4] 机制双向验证完成：{so} 装/撤各自改变 maps，App 三次冷启动均正常");
    }

    /// 补丁器本身要能在宿主上测：合成一个「ELF64 头 + 一个 PT_NOTE + build-id note」，
    /// 断言只改 1 字节、改的位置落在 note 描述里、非 note 文件返回 None。
    /// 真机腿跑不了几次，但这个解析错了整条腿就是自欺。
    #[test]
    fn build_id_patch_touches_exactly_one_byte_inside_the_note() {
        fn synthetic(with_note: bool) -> Vec<u8> {
            let mut bytes = vec![0_u8; 4096];
            bytes[0..4].copy_from_slice(b"\x7fELF");
            bytes[4] = 2; // ELFCLASS64
            let phoff = 64_u64; // 程序头紧跟 ELF 头
            bytes[0x20..0x28].copy_from_slice(&phoff.to_le_bytes());
            bytes[0x36..0x38].copy_from_slice(&56_u16.to_le_bytes()); // phentsize
            bytes[0x38..0x3a].copy_from_slice(&1_u16.to_le_bytes()); // phnum
            let note_off = 256_u64;
            if with_note {
                // PT_NOTE 段
                let header = phoff as usize;
                bytes[header..header + 4].copy_from_slice(&4_u32.to_le_bytes());
                bytes[header + 8..header + 16].copy_from_slice(&note_off.to_le_bytes());
                bytes[header + 32..header + 40].copy_from_slice(&32_u64.to_le_bytes());
                // note: namesz=4 descsz=8 type=3 "GNU\0" + 8 字节 id
                let n = note_off as usize;
                bytes[n..n + 4].copy_from_slice(&4_u32.to_le_bytes());
                bytes[n + 4..n + 8].copy_from_slice(&8_u32.to_le_bytes());
                bytes[n + 8..n + 12].copy_from_slice(&3_u32.to_le_bytes());
                bytes[n + 12..n + 16].copy_from_slice(b"GNU\0");
                bytes[n + 16..n + 24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
            } else {
                let header = phoff as usize;
                bytes[header..header + 4].copy_from_slice(&6_u32.to_le_bytes()); // PT_LOAD
                bytes[header + 8..header + 16].copy_from_slice(&note_off.to_le_bytes());
            }
            bytes
        }

        let base = synthetic(true);
        let mut patched = base.clone();
        let offset = patch_build_id_byte(&mut patched).expect("合成 ELF 应能定位 build-id");
        assert!(
            (272..280).contains(&offset),
            "改动必须落在 build-id 描述区内，实际 {offset}"
        );
        assert_eq!(patched.len(), base.len());
        let diffs: Vec<usize> = (0..base.len())
            .filter(|i| base[*i] != patched[*i])
            .collect();
        assert_eq!(
            diffs,
            vec![offset],
            "只能有一个字节不同，且必须是 build-id 内"
        );
        // 没有 note 的 ELF 必须返回 None，而不是随手改一处当成功
        assert_eq!(patch_build_id_byte(&mut synthetic(false)), None);
        // 非 ELF / 太短同样拒绝
        assert_eq!(
            patch_build_id_byte(&mut b"not an elf at all, really".to_vec()),
            None
        );
    }

    /// 在这份 ELF 里定位 `.note.gnu.build-id` 并**只翻一个比特**（可控修补）。
    ///
    /// 为什么选 build-id：它是纯元数据，改它不影响任何指令与符号，App 行为不变；
    /// 但它同时在「文件字节」和「加载日志」两侧可见，是代价最小的可辨识差异。
    /// 找不到 note 就返回 None——宁可让腿失败，也不要悄悄测了个空操作。
    fn patch_build_id_byte(bytes: &mut [u8]) -> Option<usize> {
        if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" || bytes[4] != 2 {
            return None;
        }
        let phoff = usize::try_from(u64::from_le_bytes(bytes[0x20..0x28].try_into().ok()?)).ok()?;
        let phentsize = usize::from(u16::from_le_bytes(bytes[0x36..0x38].try_into().ok()?));
        let phnum = usize::from(u16::from_le_bytes(bytes[0x38..0x3a].try_into().ok()?));
        for index in 0..phnum {
            let base = phoff.checked_add(index.checked_mul(phentsize)?)?;
            let header = bytes.get(base..base + 56)?;
            if u32::from_le_bytes(header[0..4].try_into().ok()?) != 4 {
                continue; // PT_NOTE
            }
            let offset =
                usize::try_from(u64::from_le_bytes(header[8..16].try_into().ok()?)).ok()?;
            let size = usize::try_from(u64::from_le_bytes(header[32..40].try_into().ok()?)).ok()?;
            let mut cursor = offset;
            let end = offset.checked_add(size)?;
            while cursor + 12 <= end {
                let chunk = bytes.get(cursor..cursor + 12)?;
                let namesz =
                    usize::try_from(u32::from_le_bytes(chunk[0..4].try_into().ok()?)).ok()?;
                let descsz =
                    usize::try_from(u32::from_le_bytes(chunk[4..8].try_into().ok()?)).ok()?;
                let ntype = u32::from_le_bytes(chunk[8..12].try_into().ok()?);
                let name_start = cursor.checked_add(12)?;
                let name = bytes.get(name_start..name_start.checked_add(namesz)?)?;
                let desc_start = name_start.checked_add(namesz.div_ceil(4).checked_mul(4)?)?;
                if ntype == 3 && name.starts_with(b"GNU") {
                    let last = desc_start.checked_add(descsz)?.checked_sub(1)?;
                    bytes[last] ^= 0x01;
                    return Some(last);
                }
                cursor = desc_start.checked_add(descsz.div_ceil(4).checked_mul(4)?)?;
            }
        }
        None
    }

    /// AR9.1 真机腿：frida-server 全生命周期（root 起 / 状态 / 停 / 幂等 / 越权参数）。
    ///
    /// 刻意用**非默认端口**（27043）跑，避免和用户自己的工作流（27042）撞车；
    /// 并且开头先确认这台机上没有正在跑的 frida-server——有就跳过，
    /// 我们不能为了测试把用户正在用的注入服务停掉。
    #[tokio::test]
    #[ignore = "真机腿（会 root 起停 frida-server）；AR9_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_frida_server -- --ignored --nocapture"]
    async fn real_agent_frida_server_lifecycle_round_trip() {
        use agent_protocol::method::{
            DEVICE_ROOT_CHECK, FRIDA_SERVER_START, FRIDA_SERVER_STATUS, FRIDA_SERVER_STOP,
        };
        use agent_protocol::{
            DeviceRootCheckParams, DeviceRootCheckResult, ErrorCode, FridaServerStartParams,
            FridaServerStartResult, FridaServerState, FridaServerStatusParams,
            FridaServerStatusResult, FridaServerStopParams, FridaServerStopResult, WriteOutcome,
        };

        let serial = std::env::var("AR9_TEST_SERIAL").expect("AR9_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        manager.connect_resolved(&serial).await.unwrap();
        let client = manager.client(&serial).unwrap();
        let environment = runner.environment().await;
        let adb_path = environment.path.expect("本机应有 adb");
        let sh = |command: String| {
            let runner = runner.clone();
            let adb_path = adb_path.clone();
            let serial = serial.clone();
            async move {
                runner
                    .run(
                        &adb_path,
                        &adb::build_args(Some(&serial), &adb::cmd_shell(&command)),
                        Duration::from_secs(20),
                    )
                    .await
                    .unwrap()
                    .stdout
            }
        };
        let status = || {
            let client = client.clone();
            async move {
                client
                    .request::<_, FridaServerStatusResult>(
                        FRIDA_SERVER_STATUS,
                        &FridaServerStatusParams {},
                        Duration::from_secs(10),
                    )
                    .await
                    .expect("frida.server.status 应返回结构化状态")
            }
        };

        // 前置一：二进制是否真的可执行。**必须问内核，不能看 ls 的输出文本**——
        // 文件不存在时 `ls` 的报错里同样带着 "frida-server"，靠 contains 判存在
        // 是同一类坑（AR8.1 那条 `contains('=')` 的亲戚），vivo 上就是这么假通过的。
        let probe = sh(
            "test -x /data/local/tmp/frida-server && echo FRIDA_EXEC_OK || echo FRIDA_EXEC_NO"
                .to_string(),
        )
        .await;
        let have_binary = probe.contains("FRIDA_EXEC_OK");
        // 前置二：不能停掉用户正在用的服务
        let before = status().await;
        if before.running {
            eprintln!(
                "[跳过] 设备上已有 frida-server 在跑（pid={:?} uid={:?} state={:?}），本腿不会把它停掉",
                before.pid, before.uid, before.state
            );
            manager.disconnect(&serial).await.unwrap();
            return;
        }
        assert_eq!(before.state, FridaServerState::NotRunning);
        assert!(!before.as_root && !before.listening);
        eprintln!("[frida] 起点：未运行；版本探测={:?}", before.version);

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // ① 越权参数必须被结构化拒绝（低端口 / 非白名单地址 / 带路径的名字）
        for params in [
            serde_json::json!({"operation_id": "ar91-a", "port": 22}),
            serde_json::json!({"operation_id": "ar91-b", "bind": "localhost"}),
            serde_json::json!({"operation_id": "ar91-c", "binary_name": "../frida-server"}),
            serde_json::json!({"operation_id": "", "port": 27043}),
        ] {
            let error = client
                .request::<_, FridaServerStartResult>(
                    FRIDA_SERVER_START,
                    &params,
                    Duration::from_secs(10),
                )
                .await
                .expect_err("非法参数必须被拒");
            let error = match error {
                crate::services::agent_client::AgentClientError::Remote(error) => error,
                other => panic!("期望结构化错误，实际 {other:?}"),
            };
            assert_eq!(error.code, ErrorCode::InvalidRequest, "{params:?}");
        }

        // ①' 非 root 设备（例如没 su 的 vivo）到这里为止：status 与参数校验已经验完，
        // 而且必须**如实**报告「拿不到 root」而不是假装起好了。这条负例就是它的价值。
        let root: DeviceRootCheckResult = client
            .request(
                DEVICE_ROOT_CHECK,
                &DeviceRootCheckParams {},
                Duration::from_secs(10),
            )
            .await
            .expect("device.root_check 应可用");
        if !have_binary || !root.root {
            let expected = if !have_binary {
                "binary_unavailable"
            } else {
                // 有二进制但设备没授权 root：必须报 su_unavailable / su_failed，不能是 executed
                "su_unavailable"
            };
            let error = client
                .request::<_, FridaServerStartResult>(
                    FRIDA_SERVER_START,
                    &FridaServerStartParams {
                        operation_id: format!("ar91-nocap-{stamp}"),
                        binary_name: Some("frida-server".into()),
                        port: Some(27043),
                        bind: Some("127.0.0.1".into()),
                    },
                    Duration::from_secs(30),
                )
                .await
                .expect_err("缺二进制或缺 root 时启动必须失败");
            let error = match error {
                crate::services::agent_client::AgentClientError::Remote(error) => error,
                other => panic!("期望结构化错误，实际 {other:?}"),
            };
            let reason = error
                .details
                .as_ref()
                .and_then(|d| d.get("reason"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            eprintln!(
                "[frida] 能力不足分支（有二进制={have_binary} root={}）如实失败: code={:?} reason={reason}",
                root.root, error.code
            );
            assert!(
                reason == expected || (have_binary && reason.starts_with("su_")),
                "理由必须是「缺二进制」或「拿不到 root」，实际 {reason}"
            );
            assert_eq!(
                status().await.state,
                FridaServerState::NotRunning,
                "失败之后不得留下半个服务"
            );
            manager.disconnect(&serial).await.unwrap();
            return;
        }

        // ② 以 root 起（非默认端口，避免撞用户工作流）
        let start_op = format!("ar91-start-{stamp}");
        let started: FridaServerStartResult = client
            .request(
                FRIDA_SERVER_START,
                &FridaServerStartParams {
                    operation_id: start_op.clone(),
                    binary_name: Some("frida-server".into()),
                    port: Some(27043),
                    bind: Some("127.0.0.1".into()),
                },
                Duration::from_secs(60),
            )
            .await
            .expect("以 root 启动 frida-server 应成功");
        eprintln!(
            "[frida.start] outcome={:?} verified={} pid={:?} uid={:?} version={:?} steps={:?}",
            started.outcome,
            started.verified,
            started.pid,
            started.uid,
            started.version,
            started
                .steps
                .iter()
                .map(|s| (s.name.as_str(), s.ok))
                .collect::<Vec<_>>()
        );
        assert_eq!(started.outcome, WriteOutcome::Executed);
        assert!(
            started.verified,
            "三项复核必须全过，步骤链 {:?}",
            started.steps
        );
        assert_eq!(
            started.uid,
            Some(0),
            "起不来 root 就该失败，不能报 verified=true"
        );
        assert!(started.pid.unwrap_or(0) > 0);
        assert!(
            started.steps.iter().all(|step| step.ok),
            "成功路径每步都得 ok: {:?}",
            started.steps
        );

        // ③ 独立复核（不信 Agent 自证）：设备侧自己看 pid / uid / LISTEN
        let pid = started.pid.expect("启动结果必须带回 pid");
        let fact = sh(format!(
            "grep -m1 '^Uid:' /proc/{pid}/status 2>&1; tr '\\0' ' ' < /proc/{pid}/cmdline 2>&1"
        ))
        .await;
        eprintln!(
            "[frida] 设备侧复核 pid={pid}: {}",
            fact.replace('\n', " | ")
        );
        assert!(fact.contains("Uid:	0"), "必须是 root 进程: {fact}");
        assert!(
            fact.contains("27043"),
            "cmdline 应带上我们指定的端口: {fact}"
        );
        let listening = sh("netstat -tln 2>/dev/null | grep -c 27043".to_string()).await;
        assert_eq!(listening.trim(), "1", "端口必须真在 LISTEN");

        let after = status().await;
        eprintln!(
            "[frida.status] state={:?} pid={:?} uid={:?} addr={:?} port={:?} listening={}",
            after.state, after.pid, after.uid, after.listen_address, after.port, after.listening
        );
        assert_eq!(after.state, FridaServerState::RunningAsRoot);
        assert!(after.running && after.as_root && after.listening);
        assert_eq!(after.pid, Some(pid));
        assert_eq!(after.port, Some(27043));
        assert_eq!(after.listen_address.as_deref(), Some("127.0.0.1"));
        assert!(after.version.is_some(), "版本号应能带回（17.x）");

        // ④ 幂等：同 id 重发必须是 replayed；换新 id 必须是 no_op（不启第二个）
        let replay: FridaServerStartResult = client
            .request(
                FRIDA_SERVER_START,
                &FridaServerStartParams {
                    operation_id: start_op.clone(),
                    binary_name: Some("frida-server".into()),
                    port: Some(27043),
                    bind: Some("127.0.0.1".into()),
                },
                Duration::from_secs(30),
            )
            .await
            .expect("重发应返回已知结果");
        assert_eq!(replay.outcome, WriteOutcome::Replayed);
        assert_eq!(replay.pid, started.pid, "复用结果不得变成另一个进程");
        let again: FridaServerStartResult = client
            .request(
                FRIDA_SERVER_START,
                &FridaServerStartParams {
                    operation_id: format!("ar91-start-again-{stamp}"),
                    binary_name: Some("frida-server".into()),
                    port: Some(27043),
                    bind: Some("127.0.0.1".into()),
                },
                Duration::from_secs(30),
            )
            .await
            .expect("已在跑时不该报错，而是 no_op");
        eprintln!(
            "[frida.start] 二次调用 outcome={:?} verified={} pid={:?}",
            again.outcome, again.verified, again.pid
        );
        assert_eq!(again.outcome, WriteOutcome::NoOp);
        assert_eq!(again.pid, Some(pid), "不该起第二个实例");
        let pids = sh("pidof frida-server".to_string()).await;
        assert_eq!(
            pids.split_whitespace().count(),
            1,
            "设备上只该有一个 frida-server: {pids}"
        );

        // ⑤ 停：root 属主进程只能由设备侧特权脚本杀，且必须复核消失
        let stopped: FridaServerStopResult = client
            .request(
                FRIDA_SERVER_STOP,
                &FridaServerStopParams {
                    operation_id: format!("ar91-stop-{stamp}"),
                    binary_name: Some("frida-server".into()),
                },
                Duration::from_secs(60),
            )
            .await
            .expect("停止应成功");
        eprintln!(
            "[frida.stop] outcome={:?} verified={} pid={:?} uid={:?}",
            stopped.outcome, stopped.verified, stopped.pid, stopped.uid
        );
        assert_eq!(stopped.outcome, WriteOutcome::Executed);
        assert!(stopped.verified, "必须复核到进程消失");
        assert_eq!(stopped.pid, Some(pid));
        assert_eq!(stopped.uid, Some(0), "停之前读到的 uid 要如实带回");
        let gone = status().await;
        assert_eq!(gone.state, FridaServerState::NotRunning);
        assert!(!gone.running && !gone.listening);
        assert_eq!(
            sh("netstat -tln 2>/dev/null | grep -c 27043".to_string())
                .await
                .trim(),
            "0",
            "端口必须已释放"
        );

        // ⑥ 再停一次：no_op + verified（已经是期望状态）
        let again: FridaServerStopResult = client
            .request(
                FRIDA_SERVER_STOP,
                &FridaServerStopParams {
                    operation_id: format!("ar91-stop-again-{stamp}"),
                    binary_name: Some("frida-server".into()),
                },
                Duration::from_secs(30),
            )
            .await
            .expect("已停时不该报错");
        assert_eq!(again.outcome, WriteOutcome::NoOp);
        assert!(again.verified);

        sh("rm -f /data/local/tmp/frida-server.artool.log".to_string()).await;
        manager.disconnect(&serial).await.unwrap();
        eprintln!("[frida] 全周期通过：起→复核→幂等→停→端口释放");
    }

    /// AR10.2 真机腿：**单个 handler 失败只熔断它自己**，且熔断期间能力位自动收缩。
    ///
    /// 制造内部失败的办法是临时把模块的 `helper.dex` 改名（需要 root），让所有
    /// 依赖 Java helper 的方法连续失败到阈值；随后必须还原文件并等过冷却窗口。
    /// 因为会临时改模块目录，这一条要显式 `AR5_FUSE_PROBE=yes` 才跑，默认跳过。
    ///
    /// 这条腿一次验四件事，都是文档里容易写但没人证明的东西：
    /// ①内部失败计入熔断、参数非法不计入；②熔断只关那几条方法，`zygisk.status`
    /// 与注册表查询照答；③能力位随熔断收缩 → Agent 侧门控在**发命令前**就拒
    /// （`capability_missing`），不再让调用方等超时；④冷却窗口过后自动恢复。
    #[tokio::test]
    #[ignore = "会临时改模块文件；APPLIST_TEST_SERIAL=<serial> AR5_FUSE_PROBE=yes cargo test -p app-reverse-tools real_agent_zygisk_single_handler -- --ignored --nocapture"]
    async fn real_agent_zygisk_single_handler_failure_fuses_only_that_method() {
        use agent_protocol::method::{PACKAGE_LIST_LOCALIZED, ZYGISK_STATUS};
        use agent_protocol::{
            ErrorCode, PackageListLocalizedParams, PackageScope, ZygiskStatusParams,
            ZygiskStatusResult,
        };
        if std::env::var("AR5_FUSE_PROBE").unwrap_or_default() != "yes" {
            eprintln!(
                "[跳过] 本腿会临时改名 /data/adb/modules/applistpro/helper.dex，需要 AR5_FUSE_PROBE=yes 显式授权"
            );
            return;
        }
        let serial = std::env::var("APPLIST_TEST_SERIAL").expect("APPLIST_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        manager.connect_resolved(&serial).await.unwrap();
        let client = manager.client(&serial).unwrap();
        let adb_path = runner.environment().await.path.expect("本机应有 adb");
        let root = |command: String| {
            let runner = runner.clone();
            let adb_path = adb_path.clone();
            let serial = serial.clone();
            async move {
                let out = runner
                    .run(
                        &adb_path,
                        &adb::build_args(Some(&serial), &adb::cmd_shell(&adb::su_wrap(&command))),
                        Duration::from_secs(20),
                    )
                    .await
                    .unwrap();
                out.stdout
            }
        };
        // 开跑前先自愈：上一次运行如果在中途 panic，可能把 helper.dex 留在 .off 状态，
        // 那样这一轮的"起点必须能用"就成了随机事件。先补回来再测。
        root(format!(
            "if [ -f {dex}.off ]; then mv {dex}.off {dex}; chmod 644 {dex}; fi",
            dex = "/data/adb/modules/applistpro/helper.dex"
        ))
        .await;
        // Agent 侧探测有 5 秒缓存（PROBE_TTL），熔断路径走特权脚本 + helper 失败计数，
        // 至少要 3 次内部失败 + 一次缓存过期后才看得到。所以下面每轮都等到 TTL 之后再看。
        const PROBE_WAIT: Duration = Duration::from_secs(6);
        let dex = "/data/adb/modules/applistpro/helper.dex";
        let status = || {
            let client = client.clone();
            async move {
                client
                    .request::<_, ZygiskStatusResult>(
                        ZYGISK_STATUS,
                        &ZygiskStatusParams {},
                        Duration::from_secs(15),
                    )
                    .await
                    .expect("zygisk.status 必须答话")
            }
        };
        let list_once = || async {
            client
                .request::<_, agent_protocol::PackageListLocalizedResult>(
                    PACKAGE_LIST_LOCALIZED,
                    &PackageListLocalizedParams {
                        locale: None,
                        scope: PackageScope::All,
                        include_disabled: false,
                    },
                    Duration::from_secs(30),
                )
                .await
        };

        // 前置：模块必须是 v2 且宣告 handlers 能力（否则本腿没有对象，不是回归）
        let before = status().await;
        if before.module_handlers.is_empty() {
            eprintln!(
                "[跳过] 模块未回传 handler 注册表（capability handlers 缺失），本腿需要装了 AR10.2 之后模块的设备"
            );
            manager.disconnect(&serial).await.unwrap();
            return;
        }
        assert!(
            before.module_handlers.iter().any(|h| h.cmd == "L"),
            "注册表里必须有清单方法: {:?}",
            before.module_handlers
        );
        // 起点允许存在"上一轮实验留下的熔断"——它本来就是 60 秒自动恢复的设计。
        // 所以这里不是断言"没有熔断"，而是等它自己好；等不到才说明冷却没生效。
        let mut before = before;
        for _ in 0..12 {
            if before.module_handlers.iter().all(|h| !h.fused) {
                break;
            }
            eprintln!(
                "[ar10.2] 起点仍有方法在冷却中（上一轮实验留下），等 8 秒：{:?}",
                before
                    .module_handlers
                    .iter()
                    .filter(|h| h.fused)
                    .map(|h| h.cmd.clone())
                    .collect::<Vec<_>>()
            );
            tokio::time::sleep(Duration::from_secs(8)).await;
            before = status().await;
        }
        assert!(
            before.module_handlers.iter().all(|h| !h.fused),
            "冷却窗口过后仍显示熔断，说明自动恢复没生效: {:?}",
            before.module_handlers
        );
        assert!(list_once().await.is_ok(), "起点：清单能力必须可用");

        // 参数非法**不该**计入熔断：连打 5 次非法 locale，能力必须还在
        let bads: Vec<String> = vec![
            "en-US;id".to_owned(),
            "a]b".to_owned(),
            "x9 y".to_owned(),
            "z".repeat(40),
        ];
        for bad in &bads {
            let error = client
                .request::<_, agent_protocol::PackageListLocalizedResult>(
                    PACKAGE_LIST_LOCALIZED,
                    &PackageListLocalizedParams {
                        locale: Some(bad.clone()),
                        scope: PackageScope::All,
                        include_disabled: false,
                    },
                    Duration::from_secs(20),
                )
                .await
                .expect_err("非法 locale 必须被拒");
            assert!(
                matches!(
                    error,
                    crate::services::agent_client::AgentClientError::Remote(_)
                ),
                "应为结构化错误: {error:?}"
            );
        }
        assert!(list_once().await.is_ok(), "参数错误被计入了熔断——方向反了");
        eprintln!("[ar10.2] 参数非法 5 次后清单仍可用（不计入熔断）✓");

        // 制造内部失败：临时改名 helper.dex（**必须**在 finally 之前还原）
        root(format!("mv {dex} {dex}.off")).await;
        let mut fuse_seen = false;
        for round in 1..=5 {
            // 每轮制造 3 次内部失败，够越过阈值（FUSE_THRESHOLD=3）
            for _ in 0..3 {
                let _ = list_once().await;
            }
            // 等 Agent 的探测缓存过期，才会重新问模块要注册表
            tokio::time::sleep(PROBE_WAIT).await;
            let st = status().await;
            if st.module_handlers.iter().any(|h| h.cmd == "L" && h.fused) {
                fuse_seen = true;
                eprintln!(
                    "[ar10.2] 第 {round} 轮：清单方法已熔断，同一次 status 里注册表与状态本身仍正常返回（fused 数={}）",
                    st.module_handlers.iter().filter(|h| h.fused).count()
                );
                break;
            }
        }
        // 先还原，再断言：反过来会让一次失败把设备留在坏状态里。
        let dex_now = dex.to_owned();
        root(format!(
            "mv {dex_now}.off {dex_now} 2>/dev/null; chmod 644 {dex_now} 2>/dev/null; echo restored"
        ))
        .await;
        assert!(
            fuse_seen,
            "连续内部失败后没能进入熔断，说明注册表没有按方法记账"
        );

        // 熔断期间：status 照答、清单必须被**门控**拒掉（不是超时），
        // 且理由要说清缺哪个能力 —— 这才是「能力位随熔断收缩」的端到端闭环
        let error = list_once()
            .await
            .expect_err("熔断期间清单命令应当在发出去之前就被拒");
        let error = match error {
            crate::services::agent_client::AgentClientError::Remote(error) => error,
            other => panic!("期望结构化错误，实际 {other:?}"),
        };
        assert_eq!(error.code, ErrorCode::UnsupportedMethod, "{error:?}");
        let reason = error
            .details
            .as_ref()
            .and_then(|d| d.get("reason"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned();
        assert_eq!(reason, "capability_missing", "实际理由: {reason}");
        eprintln!("[ar10.1/10.2] 熔断 → 能力位消失 → Agent 发命令前就拒 ✓");

        // 冷却窗口 60 s（+ 探测缓存 6 s）：等到注册表里 fused=false 且清单恢复
        let mut back = false;
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_secs(8)).await;
            let st = status().await;
            if st.module_handlers.iter().all(|h| !h.fused) && list_once().await.is_ok() {
                back = true;
                break;
            }
        }
        let leftover = root(format!("ls {dex}.off 2>/dev/null | wc -l")).await;
        assert_eq!(leftover.trim(), "0", "helper.dex 没还原干净，必须人工检查");
        assert!(back, "冷却窗口之后清单能力没有自动回来");
        eprintln!("[ar10.2] 冷却后自动恢复 ✓（helper.dex 已还原，无 .off 残留）");
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR9.1 前置真机腿：root 探测改由 Agent 执行后，结论必须与 Legacy `su -c id`
    /// 一致，而且要把「su 可用」与「Agent 自身有 root」分开带回——UI 之前把这两件事
    /// 混成一个绿色徽章，正是 D026 那批 `root=true` 支路必须留在 Legacy 的原因。
    #[tokio::test]
    #[ignore = "需要真机；AR9_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_root_check -- --ignored --nocapture"]
    async fn real_agent_root_check_separates_su_from_agent_identity() {
        use agent_protocol::method::DEVICE_ROOT_CHECK;
        use agent_protocol::{DeviceRootCheckParams, DeviceRootCheckResult};

        let serial = std::env::var("AR9_TEST_SERIAL").expect("AR9_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        let status = manager.connect_resolved(&serial).await.unwrap();
        assert!(
            status
                .capabilities
                .iter()
                .any(|capability| capability.method == DEVICE_ROOT_CHECK && capability.available),
            "Agent 未发布 device.root_check"
        );
        let client = manager.client(&serial).unwrap();

        let result: DeviceRootCheckResult = client
            .request(
                DEVICE_ROOT_CHECK,
                &DeviceRootCheckParams {},
                Duration::from_secs(15),
            )
            .await
            .unwrap();
        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();
        let legacy = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_shell(&adb::su_wrap("id"))),
                Duration::from_secs(15),
            )
            .await
            .unwrap();
        let legacy_root = legacy.exit_code == Some(0) && adb::is_root_probe_ok(&legacy.stdout);
        eprintln!(
            "[device.root_check] root={} agent_uid={} probe_ms={} detail={:?} legacy_root={}",
            result.root, result.agent_uid, result.probe_ms, result.detail, legacy_root
        );
        assert_eq!(
            result.root, legacy_root,
            "Agent 与 Legacy 的 su 可用性结论必须一致（一侧超时也不行）"
        );
        assert!(
            result.detail.is_some(),
            "false 也必须给出可区分的理由（超时/无 su/被拒不是一回事）"
        );
        let known = [
            "granted",
            "granted_but_failed",
            "denied",
            "not_root",
            "timeout",
            "su_unavailable",
            "su_exec_failed",
        ];
        assert!(
            known.contains(&result.detail.as_deref().unwrap_or("")),
            "detail 必须是已分类的理由，实际 {:?}",
            result.detail
        );
        // 关键区分：su 可用（root=true）时 Agent 自己仍是 shell(2000)
        assert_eq!(
            result.agent_uid, 2000,
            "Agent 由 adb shell 启动，uid 必须是 2000；若某天变了说明启动方式变了，D026 需要重评"
        );
        if result.root {
            assert_ne!(
                result.agent_uid, 0,
                "su 可用不等于 Agent 已提权，这条断言就是 D026 的机读版本"
            );
        }
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR7.3 真机腿：按句柄停止必须先核身份（PID 易主时拒止且不动手），
    /// 停止结果要能区分「已确认消失」与「发了信号但没确认到」，
    /// 端口方向则复用 AR6.2 的 `process.ports`（不再有 `ls -l` fd + grep 那条链）。
    #[tokio::test]
    #[ignore = "需要真机；AR7_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_hosted_stop -- --ignored --nocapture"]
    async fn real_agent_hosted_stop_verifies_identity_and_reuses_process_ports() {
        use agent_protocol::method::{HOSTED_LIST, HOSTED_START, HOSTED_STOP, PROCESS_PORTS};
        use agent_protocol::{
            HostedListResult, HostedRunState, HostedStartParams, HostedStartResult,
            HostedStopParams, HostedStopResult, KillOutcome, KillSignal, ProcessPortsParams,
            ProcessPortsResult,
        };

        const PROBE: &str = "toybox";
        const PROBE_PORT: u16 = 24574;

        async fn adb_shell(serial: &str, command: &str) -> String {
            let output = tokio::process::Command::new("adb")
                .args(["-s", serial, "shell", command])
                .output()
                .await
                .expect("adb shell 可用");
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        let serial = std::env::var("AR7_TEST_SERIAL").expect("AR7_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        let status = manager.connect_resolved(&serial).await.unwrap();
        assert!(
            status
                .capabilities
                .iter()
                .any(|capability| capability.method == HOSTED_STOP && capability.available),
            "Agent 未发布 hosted.stop"
        );
        let client = manager.client(&serial).unwrap();

        if !prepare_toybox_probe(&serial).await {
            return;
        }

        let started: HostedStartResult = client
            .request(
                HOSTED_START,
                &HostedStartParams {
                    name: PROBE.into(),
                    args: vec![
                        "nc".into(),
                        "-4".into(),
                        "-L".into(),
                        "-s".into(),
                        "127.0.0.1".into(),
                        "-p".into(),
                        PROBE_PORT.to_string(),
                    ],
                    root: false,
                },
                Duration::from_secs(15),
            )
            .await
            .unwrap();
        let handle = started.record.handle.clone();
        let pid = started.record.pid;
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            adb_shell(&serial, "netstat -tln 2>/dev/null")
                .await
                .contains(&format!(":{PROBE_PORT}")),
            "托管进程应在监听"
        );

        // ① 端口方向复用 AR6.2：按 pid 能查到它自己的监听端口
        let ports: ProcessPortsResult = client
            .request(
                PROCESS_PORTS,
                &ProcessPortsParams { pid },
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        eprintln!(
            "[hosted.ports] pid={} comm={:?} ports={:?}",
            pid,
            ports.comm,
            ports.ports.iter().map(|p| p.port).collect::<Vec<_>>()
        );
        assert_eq!(ports.comm.as_deref(), Some(PROBE));
        assert!(
            ports
                .ports
                .iter()
                .any(|port| port.port == PROBE_PORT && port.state == "listen"),
            "托管进程端口应能从 process.ports 查到"
        );

        // ② 过期视图：expected_pid 与记录不符 → 拒止，且进程必须还活着
        let stale = client
            .request::<_, HostedStopResult>(
                HOSTED_STOP,
                &HostedStopParams {
                    handle: handle.clone(),
                    expected_pid: Some(pid + 1),
                    signal: KillSignal::Kill,
                },
                Duration::from_secs(10),
            )
            .await
            .expect_err("PID 与调用方看到的不符时必须拒止");
        let code = match &stale {
            crate::services::agent_client::AgentClientError::Remote(error) => error
                .details
                .as_ref()
                .and_then(|v| v["reason"].as_str().map(str::to_string)),
            _ => None,
        };
        assert_eq!(code.as_deref(), Some("pid_mismatch"));
        assert!(
            adb_shell(&serial, &format!("kill -0 {pid} 2>/dev/null && echo alive"))
                .await
                .contains("alive"),
            "拒止后进程必须还活着"
        );

        // ③ 正常停止：核过身份 + 确认消失 + 记录出表
        let stopped: HostedStopResult = client
            .request(
                HOSTED_STOP,
                &HostedStopParams {
                    handle: handle.clone(),
                    expected_pid: Some(pid),
                    signal: KillSignal::Kill,
                },
                Duration::from_secs(10),
            )
            .await
            .unwrap();
        eprintln!(
            "[hosted.stop] outcome={:?} verified={} dropped={} state={:?} exit_code={:?} detail={:?}",
            stopped.outcome,
            stopped.identity_verified,
            stopped.record_dropped,
            stopped.record.state,
            stopped.record.exit_code,
            stopped.record.detail
        );
        assert_eq!(stopped.outcome, KillOutcome::Signaled);
        assert!(stopped.identity_verified, "start time 一致时必须报核过身份");
        assert!(
            stopped.record_dropped,
            "确认后应删除持久化记录，别在重启里复活"
        );
        assert_eq!(stopped.record.state, HostedRunState::Exited);
        assert_eq!(
            stopped.record.detail.as_deref(),
            Some("exited_signal_9"),
            "被 SIGKILL 的进程只能报信号，不得编退出码"
        );
        assert!(stopped.record.exit_code.is_none());
        assert!(
            !adb_shell(&serial, &format!("kill -0 {pid} 2>/dev/null && echo alive"))
                .await
                .contains("alive"),
            "设备上进程确实已退出"
        );
        let listing = adb_shell(
            &serial,
            &format!("ls /data/local/tmp/app-reverse-tools-hosted/{handle}.json 2>&1"),
        )
        .await;
        assert!(
            listing.contains("No such file"),
            "持久化记录应已删除: {listing}"
        );
        assert!(
            !adb_shell(&serial, "netstat -tln 2>/dev/null")
                .await
                .contains(&format!(":{PROBE_PORT}")),
            "端口应随进程释放"
        );

        // ④ 幂等：再停一次不报错（记录已在内存里标 exited，进程不在 → already_gone）
        let again: HostedStopResult = client
            .request(
                HOSTED_STOP,
                &HostedStopParams {
                    handle: handle.clone(),
                    expected_pid: None,
                    signal: KillSignal::Term,
                },
                Duration::from_secs(10),
            )
            .await
            .expect("重复停止应是幂等成功");
        assert_eq!(again.outcome, KillOutcome::AlreadyGone);

        // ⑤ list 的运行表里该记录不该再是 running
        let listed: HostedListResult = client
            .request(HOSTED_LIST, &serde_json::json!({}), Duration::from_secs(20))
            .await
            .unwrap();
        assert!(
            !listed
                .runs
                .iter()
                .any(|run| run.handle == handle && run.state == HostedRunState::Running),
            "已停止的句柄不该仍标 running"
        );

        adb_shell(
            &serial,
            &format!("rm -f /data/local/tmp/{PROBE} /data/local/tmp/.{PROBE}.run.log"),
        )
        .await;
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR7.2 真机腿：托管生命周期交给 Agent 之后，Legacy 的 `ls -l` + `file` + `$!`
    /// 反查必须全部能被替代，而且要能拿到 Legacy 拿不到的东西（稳定句柄、
    /// pid+start time 身份、被自己回收的子进程的真实死因）。
    #[tokio::test]
    #[ignore = "需要真机；AR7_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_hosted -- --ignored --nocapture"]
    async fn real_agent_hosted_lifecycle_handles_identity_and_reaping() {
        use agent_protocol::method::{HOSTED_CHMOD, HOSTED_LIST, HOSTED_START, HOSTED_STATUS};
        use agent_protocol::{
            AgentError, ErrorCode, HostedChmodParams, HostedChmodResult, HostedListParams,
            HostedListResult, HostedRunState, HostedStartParams, HostedStartResult,
            HostedStatusParams, HostedStatusResult,
        };

        // toybox 靠 argv[0] 的 basename 派发 applet，所以探针文件名必须是 toybox
        const PROBE: &str = "toybox";
        const PROBE_PORT: u16 = 24573;

        async fn adb_shell(serial: &str, command: &str) -> String {
            let output = tokio::process::Command::new("adb")
                .args(["-s", serial, "shell", command])
                .output()
                .await
                .expect("adb shell 可用");
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        fn agent_error(error: crate::services::agent_client::AgentClientError) -> AgentError {
            match error {
                crate::services::agent_client::AgentClientError::Remote(error) => error,
                other => panic!("期望 Agent 结构化错误，实际 {other:?}"),
            }
        }

        let serial = std::env::var("AR7_TEST_SERIAL").expect("AR7_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        let status = manager.connect_resolved(&serial).await.unwrap();
        for method in [HOSTED_LIST, HOSTED_CHMOD, HOSTED_START, HOSTED_STATUS] {
            assert!(
                status
                    .capabilities
                    .iter()
                    .any(|capability| capability.method == method && capability.available),
                "Agent 未发布 {method} capability"
            );
        }
        let client = manager.client(&serial).unwrap();

        if !prepare_toybox_probe(&serial).await {
            return;
        }
        let copied = adb_shell(
            &serial,
            &format!("chmod 644 /data/local/tmp/{PROBE} && echo copied"),
        )
        .await;
        assert!(copied.contains("copied"), "探针文件准备失败: {copied}");

        // ① list：ELF 判定与 Legacy `file` 同结论；无执行位的文件也要列出来
        let listed: HostedListResult = client
            .request(HOSTED_LIST, &HostedListParams {}, Duration::from_secs(20))
            .await
            .unwrap();
        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();
        let legacy_ls = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_ls(adb::HOSTED_DIR)),
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        let legacy_file = adb_shell(&serial, &format!("file {}/*", adb::HOSTED_DIR)).await;
        let legacy = adb::hosted_binaries(&legacy_ls.stdout, &legacy_file);
        let agent_names: std::collections::HashSet<&str> = listed
            .binaries
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        let legacy_names: std::collections::HashSet<&str> =
            legacy.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(
            agent_names, legacy_names,
            "Agent 用文件头 magic 判 ELF 必须与设备端 `file` 命令同结论"
        );
        let probe = listed
            .binaries
            .iter()
            .find(|item| item.name == PROBE)
            .expect("探针文件应出现在托管列表");
        assert!(!probe.has_exec, "chmod 644 之后不该有执行位");
        assert_eq!(probe.mode & 0o100, 0);
        assert_eq!(probe.mode_text, "-rw-r--r--");
        assert!(
            !listed.runs.iter().any(|run| run.name == PROBE),
            "还没启动不该有运行记录，实际={:?}",
            listed
                .runs
                .iter()
                .map(|run| format!("{}#{}:{:?}", run.name, run.handle, run.state))
                .collect::<Vec<_>>()
        );
        eprintln!(
            "[hosted.list] agent_binaries={} legacy_binaries={} truncated={} unreadable={}",
            listed.binaries.len(),
            legacy.len(),
            listed.truncated,
            listed.unreadable.len()
        );

        // ② chmod：只补执行位，且重复调用幂等
        let params = HostedChmodParams { name: PROBE.into() };
        let chmodded: HostedChmodResult = client
            .request(HOSTED_CHMOD, &params, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(chmodded.has_exec);
        assert_eq!(chmodded.mode & 0o100, 0o100, "只补执行位，不改读位");
        assert_eq!(chmodded.mode_text, "-rwxr-xr-x");
        let again: HostedChmodResult = client
            .request(HOSTED_CHMOD, &params, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(again.mode, chmodded.mode, "重复 chmod 必须幂等");

        // ③ start：稳定句柄 + pid + start time，进程真的在监听
        let params = HostedStartParams {
            name: PROBE.into(),
            args: vec![
                "nc".into(),
                "-4".into(),
                "-L".into(),
                "-s".into(),
                "127.0.0.1".into(),
                "-p".into(),
                PROBE_PORT.to_string(),
            ],
            root: false,
        };
        let started: HostedStartResult = client
            .request(HOSTED_START, &params, Duration::from_secs(15))
            .await
            .unwrap();
        let record = started.record;
        eprintln!(
            "[hosted.start] handle={} pid={} ticks={} log={}",
            record.handle, record.pid, record.start_time_ticks, record.log_path
        );
        assert_eq!(record.handle.len(), 16, "句柄是 16 位十六进制");
        assert!(
            record.handle.chars().all(|c| c.is_ascii_hexdigit()),
            "句柄必须是 hex: {}",
            record.handle
        );
        assert!(record.pid > 0);
        assert!(
            record.start_time_ticks > 0,
            "start time 是身份的一部分，拿不到就是缺陷"
        );
        assert!(!record.root);
        assert_eq!(record.log_path, format!("/data/local/tmp/.{PROBE}.run.log"));
        assert_eq!(record.state, HostedRunState::Running);
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(
            adb_shell(&serial, "netstat -tln 2>/dev/null")
                .await
                .contains(&format!(":{PROBE_PORT}")),
            "托管进程应真的在监听"
        );
        // 记录确实落盘且权限收住（Agent 重启后对账全靠它）
        let state_listing = adb_shell(
            &serial,
            &format!(
                "ls -ld /data/local/tmp/app-reverse-tools-hosted; ls -l /data/local/tmp/app-reverse-tools-hosted/{}.json",
                record.handle
            ),
        )
        .await;
        eprintln!(
            "[hosted.state] {}",
            state_listing.trim().replace('\n', " | ")
        );
        assert!(
            state_listing.contains("drwx------"),
            "状态目录必须 0700，实际: {state_listing}"
        );
        assert!(
            state_listing.contains("-rw-------"),
            "运行记录必须 0600，实际: {state_listing}"
        );

        // ④ status：running → 被 SIGKILL 后由 Agent 回收，给出真实死因而非编造退出码
        let params = HostedStatusParams {
            handle: record.handle.clone(),
        };
        let live: HostedStatusResult = client
            .request(HOSTED_STATUS, &params, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(live.record.state, HostedRunState::Running);
        assert!(!live.reconciled, "本 Agent 启动的记录不该标成对账恢复");
        assert!(live.record.exit_code.is_none(), "还在跑就没有退出码");

        adb_shell(&serial, &format!("kill -9 {}", record.pid)).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let dead: HostedStatusResult = client
            .request(HOSTED_STATUS, &params, Duration::from_secs(10))
            .await
            .unwrap();
        eprintln!(
            "[hosted.status] state={:?} exit_code={:?} detail={:?}",
            dead.record.state, dead.record.exit_code, dead.record.detail
        );
        assert_eq!(dead.record.state, HostedRunState::Exited);
        assert_eq!(
            dead.record.exit_code, None,
            "被 SIGKILL 杀掉的进程没有退出码只有信号，不能把 137 当 exit_code"
        );
        assert_eq!(dead.record.detail.as_deref(), Some("exited_signal_9"));
        let listed_after: HostedListResult = client
            .request(HOSTED_LIST, &HostedListParams {}, Duration::from_secs(20))
            .await
            .unwrap();
        let in_list = listed_after
            .runs
            .iter()
            .find(|run| run.handle == record.handle)
            .expect("list 的 runs 里应能看到同一条记录");
        assert_eq!(in_list.state, HostedRunState::Exited);
        assert_eq!(
            adb_shell(
                &serial,
                &format!("kill -0 {} 2>/dev/null && echo alive", record.pid)
            )
            .await
            .trim(),
            "",
            "设备上该进程确实已退出"
        );

        // ⑤ 写操作前置拒止：非法名、root 要求、未知句柄
        let bad_name = agent_error(
            client
                .request::<_, HostedStartResult>(
                    HOSTED_START,
                    &HostedStartParams {
                        name: "../escape".into(),
                        args: vec![],
                        root: false,
                    },
                    Duration::from_secs(10),
                )
                .await
                .expect_err("非法文件名必须拒"),
        );
        assert_eq!(bad_name.code, ErrorCode::InvalidRequest);
        assert_eq!(bad_name.details.unwrap()["reason"], "invalid_name");

        let root_required = agent_error(
            client
                .request::<_, HostedStartResult>(
                    HOSTED_START,
                    &HostedStartParams {
                        name: PROBE.into(),
                        args: vec![],
                        root: true,
                    },
                    Duration::from_secs(10),
                )
                .await
                .expect_err("Agent 是 shell 身份，不能假装以 root 启动"),
        );
        assert_eq!(root_required.code, ErrorCode::PermissionDenied);
        assert_eq!(root_required.details.unwrap()["reason"], "root_required");

        let unknown = agent_error(
            client
                .request::<_, HostedStatusResult>(
                    HOSTED_STATUS,
                    &HostedStatusParams {
                        handle: "0000000000000000".into(),
                    },
                    Duration::from_secs(10),
                )
                .await
                .expect_err("未知句柄必须 not_found"),
        );
        assert_eq!(unknown.code, ErrorCode::NotFound);
        assert_eq!(unknown.details.unwrap()["reason"], "unknown_handle");

        adb_shell(
            &serial,
            &format!(
                "rm -f /data/local/tmp/{PROBE} /data/local/tmp/.{PROBE}.run.log /data/local/tmp/app-reverse-tools-hosted/{}.json",
                record.handle
            ),
        )
        .await;
        manager.disconnect(&serial).await.unwrap();
    }

    /// AR7.1 真机腿：设备端文件 API 与 Legacy `ls -lA` 文本解析必须同结论，
    /// 且路径策略（允许根、`..` 拒止、符号链接逃逸）在真机上真的挡得住。
    #[tokio::test]
    #[ignore = "需要真机；AR7_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_filesystem -- --ignored --nocapture"]
    async fn real_agent_filesystem_list_stat_preview_and_path_policy() {
        use agent_protocol::method::{FILESYSTEM_LIST, FILESYSTEM_PREVIEW, FILESYSTEM_STAT};
        use agent_protocol::{
            ErrorCode, FileKind, FilesystemListParams, FilesystemListResult,
            FilesystemPreviewParams, FilesystemPreviewResult, FilesystemStatParams,
            FilesystemStatResult, PreviewEncoding,
        };

        async fn adb_shell(serial: &str, command: &str) -> String {
            let output = tokio::process::Command::new("adb")
                .args(["-s", serial, "shell", command])
                .output()
                .await
                .expect("adb shell 可用");
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        fn agent_error(
            error: crate::services::agent_client::AgentClientError,
        ) -> agent_protocol::AgentError {
            match error {
                crate::services::agent_client::AgentClientError::Remote(error) => error,
                other => panic!("期望 Agent 结构化错误，实际 {other:?}"),
            }
        }

        let serial = std::env::var("AR7_TEST_SERIAL").expect("AR7_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        let status = manager.connect_resolved(&serial).await.unwrap();
        for method in [FILESYSTEM_LIST, FILESYSTEM_STAT, FILESYSTEM_PREVIEW] {
            assert!(
                status
                    .capabilities
                    .iter()
                    .any(|capability| capability.method == method && capability.available),
                "Agent 未发布 {method} capability"
            );
        }
        let client = manager.client(&serial).unwrap();

        // 造一个已知内容的小文本文件 + 一个指向白名单外的符号链接
        adb_shell(
            &serial,
            "printf 'line-1\\nline-2\\n' > /data/local/tmp/ar71_probe.txt",
        )
        .await;
        adb_shell(
            &serial,
            "ln -sf /data/data/com.android.providers.contacts/databases /data/local/tmp/ar71_escape",
        )
        .await;

        // ① list：与 Legacy `ls -lA` 解析结果同名同类型
        let params = FilesystemListParams {
            path: "/data/local/tmp".into(),
            include_hidden: true,
        };
        let listed: FilesystemListResult = client
            .request(FILESYSTEM_LIST, &params, Duration::from_secs(20))
            .await
            .unwrap();
        assert_eq!(listed.path, "/data/local/tmp");
        assert!(!listed.truncated, "托管目录不应触发截断");
        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();
        let legacy = runner
            .run(
                &adb_path,
                &adb::build_args(Some(&serial), &adb::cmd_ls("/data/local/tmp")),
                Duration::from_secs(20),
            )
            .await
            .unwrap();
        let legacy_entries: Vec<adb::FileEntry> = legacy
            .stdout
            .lines()
            .filter_map(adb::parse_ls_long)
            .collect();
        let legacy_names: std::collections::HashSet<&str> = legacy_entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        let agent_names: std::collections::HashSet<&str> = listed
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            agent_names, legacy_names,
            "Agent 与 Legacy 的目录项集合必须一致（差异即迁移缺陷）"
        );
        for legacy_entry in &legacy_entries {
            let agent_entry = listed
                .entries
                .iter()
                .find(|entry| entry.name == legacy_entry.name)
                .unwrap();
            assert_eq!(
                agent_entry.kind == FileKind::Dir,
                legacy_entry.is_dir,
                "{} 的目录判定不一致",
                legacy_entry.name
            );
            assert_eq!(
                agent_entry.symlink_target, legacy_entry.symlink,
                "{} 的符号链接目标不一致",
                legacy_entry.name
            );
            if agent_entry.kind == FileKind::File {
                assert_eq!(
                    agent_entry.size as i64, legacy_entry.size,
                    "{} 大小不一致",
                    legacy_entry.name
                );
            }
        }
        let probe = listed
            .entries
            .iter()
            .find(|entry| entry.name == "ar71_probe.txt")
            .expect("探测文件应在列表里");
        assert_eq!(probe.kind, FileKind::File);
        assert_eq!(probe.size, 14);
        assert_eq!(probe.uid, 2000, "探测文件由 shell 创建");
        assert!(probe.readable && probe.mtime_unix > 1_700_000_000);
        eprintln!(
            "[filesystem.list] entries={} mode_text={} mtime={}",
            listed.entries.len(),
            probe.mode_text,
            probe.mtime_unix
        );

        // ② stat：lstat 与 follow 两种语义
        let params = FilesystemStatParams {
            path: "/data/local/tmp/ar71_escape".into(),
            follow_symlink: false,
        };
        let stated: FilesystemStatResult = client
            .request(FILESYSTEM_STAT, &params, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(stated.stat.kind, FileKind::Symlink);
        assert_eq!(
            stated.stat.symlink_target.as_deref(),
            Some("/data/data/com.android.providers.contacts/databases")
        );
        assert_eq!(stated.requested_path, "/data/local/tmp/ar71_escape");

        // ③ preview：文本按 utf8 返回，且与设备侧 `cat` 完全一致
        let params = FilesystemPreviewParams {
            path: "/data/local/tmp/ar71_probe.txt".into(),
            max_bytes: Some(4096),
            from_end: false,
        };
        let previewed: FilesystemPreviewResult = client
            .request(FILESYSTEM_PREVIEW, &params, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(previewed.encoding, PreviewEncoding::Utf8);
        assert_eq!(previewed.text.as_deref(), Some("line-1\nline-2\n"));
        assert_eq!(previewed.returned_bytes, 14);
        assert!(!previewed.truncated);

        // ④ preview：ELF 走 hex，不落 base64，也不当文本
        let params = FilesystemPreviewParams {
            path: "/data/local/tmp/app_reverse_tools_agent".into(),
            max_bytes: Some(16),
            from_end: false,
        };
        let elf: FilesystemPreviewResult = client
            .request(FILESYSTEM_PREVIEW, &params, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(elf.encoding, PreviewEncoding::Hex);
        assert!(
            elf.hex.as_deref().unwrap().starts_with("7f454c46"),
            "ELF magic 应出现在 hex 预览开头，实际 {:?}",
            elf.hex
        );
        assert!(elf.text.is_none());
        assert!(elf.truncated, "只取 16 字节时必须标截断");

        // ⑤ preview from_end：日志尾读语义（替代 `tail -c`）
        let params = FilesystemPreviewParams {
            path: "/data/local/tmp/ar71_probe.txt".into(),
            max_bytes: Some(7),
            from_end: true,
        };
        let tail: FilesystemPreviewResult = client
            .request(FILESYSTEM_PREVIEW, &params, Duration::from_secs(10))
            .await
            .unwrap();
        assert_eq!(tail.offset, 7);
        assert_eq!(tail.text.as_deref(), Some("line-2\n"));

        // ⑥ 路径策略：`..` 拒止、白名单外拒止、符号链接逃逸拒止
        let traversal = agent_error(
            client
                .request::<_, FilesystemListResult>(
                    FILESYSTEM_LIST,
                    &FilesystemListParams {
                        path: "/data/local/tmp/../../data/data".into(),
                        include_hidden: false,
                    },
                    Duration::from_secs(10),
                )
                .await
                .expect_err("含 .. 的路径必须被拒"),
        );
        assert_eq!(traversal.code, ErrorCode::InvalidRequest);
        assert_eq!(
            traversal.details.expect("必须带 reason")["reason"],
            "parent_escape"
        );

        // 其他应用私有目录：Agent 以 shell 身份运行，内核 DAC 直接挡住。
        // 未配置 APP_REVERSE_TOOLS_AGENT_FS_ROOTS 时理由是 permission_denied（真实边界在内核）；
        // 配了白名单则会是 path_not_allowed。两者都必须是「拒绝 + 有 reason」，不能返回空目录。
        let outside = agent_error(
            client
                .request::<_, FilesystemListResult>(
                    FILESYSTEM_LIST,
                    &FilesystemListParams {
                        path: "/data/data/com.android.providers.contacts".into(),
                        include_hidden: false,
                    },
                    Duration::from_secs(10),
                )
                .await
                .expect_err("其他应用私有目录必须被拒"),
        );
        assert_eq!(outside.code, ErrorCode::PermissionDenied, "{outside:?}");
        let reason = outside.details.expect("必须带 reason")["reason"].clone();
        assert!(
            reason == "permission_denied" || reason == "path_not_allowed",
            "拒绝原因必须可区分，实际 {reason}"
        );

        let escape = agent_error(
            client
                .request::<_, FilesystemStatResult>(
                    FILESYSTEM_STAT,
                    &FilesystemStatParams {
                        path: "/data/local/tmp/ar71_escape".into(),
                        follow_symlink: true,
                    },
                    Duration::from_secs(10),
                )
                .await
                .expect_err("符号链接逃逸必须被拒"),
        );
        assert_eq!(escape.code, ErrorCode::PermissionDenied, "{escape:?}");
        assert!(
            escape.details.expect("必须带 reason")["reason"].is_string(),
            "符号链接逃逸也要给出可区分理由"
        );

        // ⑦ 符号链接根：/sdcard 解析成 /storage/emulated/0 后仍在允许范围内
        let sdcard: FilesystemListResult = client
            .request(
                FILESYSTEM_LIST,
                &FilesystemListParams {
                    path: "/sdcard".into(),
                    include_hidden: false,
                },
                Duration::from_secs(20),
            )
            .await
            .expect("/sdcard 必须可列（否则文件浏览页直接废掉）");
        assert_eq!(sdcard.path, "/storage/emulated/0");
        eprintln!(
            "[filesystem.list] /sdcard -> {} entries={}",
            sdcard.path,
            sdcard.entries.len()
        );

        adb_shell(
            &serial,
            "rm -f /data/local/tmp/ar71_probe.txt /data/local/tmp/ar71_escape",
        )
        .await;
        manager.disconnect(&serial).await.unwrap();
    }

    /// 准备好设备侧的 toybox 探针副本，返回 `false` 表示这台机不该跑本腿。
    ///
    /// 两处修正都是真机跑出来的：
    /// * **存在性要问内核**：原来判 `ls <路径>` 的输出里有没有文件名，可 `ls` 的报错
    ///   本身就带文件名（`ls: /data/local/tmp/toybox: No such file or directory`），
    ///   于是文件不存在时也会判成"已有"——一条永远红的腿。
    /// * **自己的残留要能自愈**：两条宿主腿共用 `/data/local/tmp/toybox`（探针靠
    ///   argv[0] 的 basename 派发 applet，名字不能改），任何一次中途被打断都会留下
    ///   副本，把之后每一次运行都毒死。sha256 与 `/system/bin/toybox` 一致的就是我们
    ///   自己的副本，直接接管；不一致才可能是用户的文件，那种情况仍然拒绝。
    async fn prepare_toybox_probe(serial: &str) -> bool {
        async fn probe_shell(serial: &str, command: &str) -> String {
            let output = tokio::process::Command::new("adb")
                .args(["-s", serial, "shell", command])
                .output()
                .await
                .expect("adb shell 可用");
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        let present = probe_shell(
            serial,
            "if [ -e /data/local/tmp/toybox ]; then echo PRESENT; fi",
        )
        .await
        .contains("PRESENT");
        if !present {
            let copied = probe_shell(
                serial,
                "cp /system/bin/toybox /data/local/tmp/toybox && chmod 755 /data/local/tmp/toybox && echo copied",
            )
            .await;
            assert!(copied.contains("copied"), "探针准备失败: {copied}");
            return true;
        }
        let hashes = probe_shell(
            serial,
            "sha256sum /data/local/tmp/toybox /system/bin/toybox 2>/dev/null | awk '{print $1}'",
        )
        .await;
        let lines: Vec<&str> = hashes
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() == 2 && lines[0] == lines[1] {
            // 接管残留时必须把**状态**一起归零，否则腿看到的不是干净起点：
            // ①两条腿对执行位要求不同（一条 755、一条 644）；②中断那次跑还留着
            // 运行日志、托管登记记录和一个活着的探针进程——「还没启动不该有运行记录」
            // 这类断言会因此说谎（真机跑出来过）。杀进程走登记里的 pid，并且先核
            // `/proc/<pid>/cmdline` 确明确实是我们那个路径——不用 `pidof toybox`
            // （设备上叫 toybox 的进程未必是我们起的），也不用 `pgrep -f 路径`
            // （它会匹配到自己这条命令行，真机跑出来过一次：把执行清理的 shell 自己杀了）。
            // 登记记录里存的是应用名而不是路径，所以按名字匹配删除；用户的同名文件
            // 在上一步 sha256 比对时就已经被排除掉了。
            let normalized = probe_shell(
                serial,
                "chmod 755 /data/local/tmp/toybox; \\
                 rm -f /data/local/tmp/.toybox.run.log; \\
                 for f in /data/local/tmp/app-reverse-tools-hosted/*.json; do \\
                   grep -q toybox $f || continue; \\
                   pid=$(grep -o 'pid[^0-9]*[0-9]*' $f | tr -dc 0-9); \\
                   if [ -n \"$pid\" ] && grep -q /data/local/tmp/toybox /proc/$pid/cmdline 2>/dev/null; then \\
                     kill -9 $pid; \\
                   fi; \\
                   rm -f $f; \\
                 done; \\
                 echo normalized",
            )
            .await;
            assert!(
                normalized.contains("normalized"),
                "接管残留副本时归一化失败: {normalized}"
            );
            eprintln!(
                "[ar7] 接管上次中断留下的 toybox 副本（与 /system/bin 一致，不是用户文件），已归一化"
            );
            return true;
        }
        eprintln!("[跳过] 设备上有一个不是我们副本的 /data/local/tmp/toybox，测试不碰用户文件");
        false
    }

    /// 用 `toybox nc` 起一个 shell 自己属主的监听端口，这样两条路径都以 shell 身份
    /// 观察（Agent 也是 shell 启动的），比较的是解析能力而不是权限差异；
    /// 另取一个 root 属主监听端口验证「读不到属主」被如实报成 `unowned`+`skipped`。
    #[tokio::test]
    #[ignore = "需要真机；AR6_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_agent_process_ports_match_legacy -- --ignored --nocapture"]
    async fn real_agent_process_ports_match_legacy() {
        use agent_protocol::method::{PROCESS_BY_PORT, PROCESS_PORTS};
        use agent_protocol::{
            ProcessByPortParams, ProcessByPortResult, ProcessPortsParams, ProcessPortsResult,
            SocketFamily,
        };

        const PROBE_PORT: u16 = 24567;

        async fn adb_shell(serial: &str, command: &str) -> String {
            let output = tokio::process::Command::new("adb")
                .args(["-s", serial, "shell", command])
                .output()
                .await
                .expect("adb shell 可用");
            String::from_utf8_lossy(&output.stdout).into_owned()
        }

        let serial = std::env::var("AR6_TEST_SERIAL").expect("AR6_TEST_SERIAL is required");
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config.clone(), None)),
        );
        let status = manager.connect_resolved(&serial).await.unwrap();
        for method in [PROCESS_PORTS, PROCESS_BY_PORT] {
            assert!(
                status
                    .capabilities
                    .iter()
                    .any(|capability| capability.method == method && capability.available),
                "Agent 未发布 {method} capability"
            );
        }
        let client = manager.client(&serial).unwrap();

        // ① shell 自属监听端口：Agent 与 Legacy 都应查到同一个 pid
        let started = adb_shell(
            &serial,
            &format!("nohup toybox nc -4 -L -s 127.0.0.1 -p {PROBE_PORT} </dev/null >/dev/null 2>&1 & echo ok"),
        )
        .await;
        assert!(started.contains("ok"), "未能拉起监听进程: {started}");
        tokio::time::sleep(Duration::from_millis(800)).await;

        let params = ProcessByPortParams { port: PROBE_PORT };
        let agent_by_port = client
            .request::<_, ProcessByPortResult>(PROCESS_BY_PORT, &params, Duration::from_secs(20))
            .await
            .unwrap();
        let environment = runner.environment().await;
        let adb_path = environment.path.unwrap();
        let legacy_holders = {
            let grep = runner
                .run(
                    &adb_path,
                    &adb::build_args(
                        Some(&serial),
                        &adb::cmd_shell(&adb::port_grep_cmd(PROBE_PORT)),
                    ),
                    Duration::from_secs(20),
                )
                .await
                .unwrap();
            let inodes: std::collections::HashSet<u64> = adb::parse_proc_net_entries(&grep.stdout)
                .into_iter()
                .filter(|entry| {
                    entry.listen && entry.listen_port.port == PROBE_PORT && entry.inode != 0
                })
                .map(|entry| entry.inode)
                .collect();
            assert!(!inodes.is_empty(), "Legacy 应先看到探测端口，否则对照无效");
            let scan = runner
                .run(
                    &adb_path,
                    &adb::build_args(Some(&serial), &adb::cmd_shell(&adb::inode_scan_cmd())),
                    Duration::from_secs(30),
                )
                .await
                .unwrap();
            let pids = adb::parse_fd_scan(&scan.stdout, &inodes);
            assert!(!pids.is_empty(), "Legacy 应能定位探测进程，否则对照无效");
            let names = runner
                .run(
                    &adb_path,
                    &adb::build_args(Some(&serial), &adb::cmd_shell(&adb::comm_batch_cmd(&pids))),
                    Duration::from_secs(20),
                )
                .await
                .unwrap();
            adb::parse_port_holders(&names.stdout)
        };
        let agent_pids: std::collections::HashSet<u32> = agent_by_port
            .sockets
            .iter()
            .map(|socket| socket.pid)
            .collect();
        let legacy_pids: std::collections::HashSet<u32> =
            legacy_holders.iter().map(|holder| holder.pid).collect();
        eprintln!(
            "[process.by_port] agent={agent_pids:?} legacy={legacy_pids:?} unowned={} skipped={:?}",
            agent_by_port.unowned.len(),
            agent_by_port.skipped
        );
        assert_eq!(
            agent_pids, legacy_pids,
            "Agent 与 Legacy 的属主集合必须一致（探测进程都是 shell 自属，不涉及权限差异）"
        );

        // ② 反方向：同一个 pid 的监听端口集合也要逐项一致（含地址/族/去重/排序）
        let pid = *agent_pids.iter().next().expect("已有属主 pid");
        let params = ProcessPortsParams { pid };
        let agent_ports = client
            .request::<_, ProcessPortsResult>(PROCESS_PORTS, &params, Duration::from_secs(20))
            .await
            .unwrap();
        let mapped = crate::services::device_service::map_agent_listening_ports(&agent_ports.ports);
        let raw = adb_shell(&serial, &adb::hosted_ports_cmd(pid)).await;
        let legacy_ports = adb::parse_listening_ports(&raw);
        eprintln!(
            "[process.ports] pid={pid} comm={:?} agent={} legacy={} cmdline={:?}",
            agent_ports.comm,
            mapped.len(),
            legacy_ports.len(),
            agent_ports.cmdline
        );
        assert_eq!(
            mapped, legacy_ports,
            "Agent process.ports 映射后必须与 Legacy 解析逐项相同"
        );
        assert!(
            mapped.iter().any(|entry| entry.port == PROBE_PORT
                && entry.address == "127.0.0.1"
                && entry.family == "tcp"),
            "探测端口应出现在 PID→端口结果里"
        );
        assert!(
            agent_ports
                .ports
                .iter()
                .any(|entry| entry.family == SocketFamily::Ipv4 && entry.state == "listen"),
            "Agent 侧应保留 address family 与状态原值"
        );

        // ③ root 属主端口（Zygisk 模块桥 11500/11501）：shell 读不到别人的 fd，
        //    必须报 unowned + skipped，而不是「没有进程监听」
        let root_probe = adb_shell(&serial, "grep -E ' 0A ' /proc/net/tcp | head -20").await;
        let root_port = adb::parse_proc_net_entries(
            &root_probe
                .lines()
                .map(|line| format!("tcp:{line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .into_iter()
        .find(|entry| entry.listen && entry.uid == 0 && entry.inode != 0)
        .map(|entry| entry.listen_port.port);
        if let Some(port) = root_port {
            let params = ProcessByPortParams { port };
            let result = client
                .request::<_, ProcessByPortResult>(
                    PROCESS_BY_PORT,
                    &params,
                    Duration::from_secs(20),
                )
                .await
                .unwrap();
            eprintln!(
                "[process.by_port root] port={} sockets={} unowned={} skipped={:?}",
                port,
                result.sockets.len(),
                result.unowned.len(),
                result.skipped
            );
            assert!(
                !result.sockets.is_empty() || !result.unowned.is_empty(),
                "root 属主端口 {port} 至少应报出 socket，不得当成「无监听」"
            );
            assert!(
                result.unowned.iter().all(|socket| socket.pid == 0),
                "属主未知的 socket 必须以 pid=0 + unowned 表达"
            );
        }

        adb_shell(
            &serial,
            &format!("pkill -f 'nc -4 -L -s 127.0.0.1 -p {PROBE_PORT}'"),
        )
        .await;
        manager.disconnect(&serial).await.unwrap();
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

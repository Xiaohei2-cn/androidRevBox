use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_protocol::{AgentError, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::core::error::{CoreError, CoreResult};
use crate::models::agent::{
    AgentRouteDiagnostics, AgentSessionState, AgentSessionStatus, AndroidBackendSource,
};
use crate::services::agent_client::AgentClientError;
use crate::services::agent_manager::AgentManager;
use crate::services::device_service::{AdbRunOutput, AdbRunner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    ReadOnlyIdempotent,
    Mutating,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendAvailability {
    Available,
    AgentUnavailable(String),
    UnsupportedMethod,
    ProviderUnavailable(String),
    Incompatible(String),
}

pub trait AndroidBackend: Send + Sync {
    fn kind(&self) -> AndroidBackendSource;
    fn availability(&self, serial: &str, method: &str) -> BackendAvailability;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    AgentUnavailable,
    UnsupportedMethod,
}

impl FallbackReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::AgentUnavailable => "agent_unavailable",
            Self::UnsupportedMethod => "unsupported_method",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDecision {
    pub backend: AndroidBackendSource,
    pub fallback_reason: Option<FallbackReason>,
    pub agent_version: Option<String>,
    pub protocol_version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyCapability {
    pub method: String,
    pub removal_stage: String,
}

impl LegacyCapability {
    pub fn new(method: impl Into<String>, removal_stage: impl Into<String>) -> Self {
        Self {
            method: method.into(),
            removal_stage: removal_stage.into(),
        }
    }
}

pub fn default_legacy_capabilities() -> Vec<LegacyCapability> {
    [
        (
            "device.root_check",
            "AR12.1 after AR9.1 and two stable phase regressions",
        ),
        (
            "device.info",
            "AR12.1 after AR5.2 and two stable phase regressions",
        ),
        (
            "package.list",
            "AR12.1 after AR5.5 and two stable phase regressions",
        ),
        (
            "activity.foreground",
            "AR12.1 after AR6.1 and two stable phase regressions",
        ),
        (
            "process.ports",
            "AR12.1 after AR6.2 and two stable phase regressions",
        ),
        (
            "process.by_port",
            "AR12.1 after AR6.2 and two stable phase regressions",
        ),
        (
            "filesystem.list",
            "AR12.1 after AR7.1 and two stable phase regressions",
        ),
        (
            "hosted.list",
            "AR12.1 after AR7.2 and two stable phase regressions",
        ),
        (
            "package.native_lib_dir",
            "AR12.1 after AR8.3 and two stable phase regressions",
        ),
    ]
    .into_iter()
    .map(|(method, removal)| LegacyCapability::new(method, removal))
    .collect()
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AgentBackendError {
    #[error("Agent is unavailable: {0}")]
    Unavailable(String),
    #[error("Agent does not support method {0}")]
    UnsupportedMethod(String),
    #[error("Agent provider is unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("Agent version is incompatible: {0}")]
    Incompatible(String),
    #[error("Agent transport was lost: {0}")]
    TransportLost(String),
    #[error("Agent request deadline exceeded")]
    DeadlineExceeded,
    #[error("Agent request was cancelled")]
    Cancelled,
    #[error("Agent protocol error: {0}")]
    Protocol(String),
    #[error("Agent business error: {message}", message = .0.message)]
    Business(AgentError),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RouteError {
    #[error("Agent is unavailable for {method}: {reason}")]
    AgentUnavailable { method: String, reason: String },
    #[error("Agent does not support {0}")]
    UnsupportedMethod(String),
    #[error("Agent provider is unavailable for {method}: {reason}")]
    ProviderUnavailable { method: String, reason: String },
    #[error("Agent is incompatible: {0}")]
    Incompatible(String),
    #[error("automatic fallback is forbidden for mutating method {0}")]
    MutatingFallbackForbidden(String),
    #[error("no registered Legacy ADB fallback for {0}")]
    LegacyFallbackNotRegistered(String),
    #[error("Agent call failed and cannot fall back: {0}")]
    AgentFailure(AgentBackendError),
}

pub struct AgentBackend {
    manager: Arc<AgentManager>,
}

impl AgentBackend {
    fn new(manager: Arc<AgentManager>) -> Self {
        Self { manager }
    }

    pub async fn request<P, R>(
        &self,
        serial: &str,
        method: &str,
        params: &P,
        timeout: Duration,
    ) -> Result<R, AgentBackendError>
    where
        P: Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let client = self.manager.client(serial).ok_or_else(|| {
            AgentBackendError::Unavailable("Agent session is not connected".into())
        })?;
        client
            .request(method, params, timeout)
            .await
            .map_err(|error| map_client_error(method, error))
    }

    fn availability_from_status(status: &AgentSessionStatus, method: &str) -> BackendAvailability {
        match status.state {
            AgentSessionState::Incompatible => BackendAvailability::Incompatible(
                status
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "protocol or Agent version mismatch".into()),
            ),
            AgentSessionState::Ready | AgentSessionState::Degraded => {
                match status
                    .capabilities
                    .iter()
                    .find(|capability| capability.method == method)
                {
                    Some(capability) if capability.available => BackendAvailability::Available,
                    Some(capability) => BackendAvailability::ProviderUnavailable(
                        capability.unavailable_reason.clone().unwrap_or_else(|| {
                            format!("provider {} is unavailable", capability.provider)
                        }),
                    ),
                    None => BackendAvailability::UnsupportedMethod,
                }
            }
            state => BackendAvailability::AgentUnavailable(format!(
                "session state is {}",
                session_state_name(state)
            )),
        }
    }
}

impl AndroidBackend for AgentBackend {
    fn kind(&self) -> AndroidBackendSource {
        AndroidBackendSource::Agent
    }

    fn availability(&self, serial: &str, method: &str) -> BackendAvailability {
        Self::availability_from_status(&self.manager.status(serial), method)
    }
}

pub struct LegacyAdbBackend {
    runner: Arc<dyn AdbRunner>,
    capabilities: HashMap<String, LegacyCapability>,
}

impl LegacyAdbBackend {
    fn new(
        runner: Arc<dyn AdbRunner>,
        capabilities: impl IntoIterator<Item = LegacyCapability>,
    ) -> Self {
        Self {
            runner,
            capabilities: capabilities
                .into_iter()
                .map(|capability| (capability.method.clone(), capability))
                .collect(),
        }
    }

    pub async fn run(&self, args: &[String], timeout: Duration) -> CoreResult<AdbRunOutput> {
        let environment = self.runner.environment().await;
        let path = environment.path.ok_or_else(|| {
            CoreError::Internal(
                environment
                    .probe_error
                    .or(environment.hint)
                    .unwrap_or_else(|| "adb 不可用".into()),
            )
        })?;
        self.runner.run(&path, args, timeout).await
    }

    pub fn removal_stage(&self, method: &str) -> Option<&str> {
        self.capabilities
            .get(method)
            .map(|capability| capability.removal_stage.as_str())
    }
}

impl AndroidBackend for LegacyAdbBackend {
    fn kind(&self) -> AndroidBackendSource {
        AndroidBackendSource::LegacyAdb
    }

    fn availability(&self, _serial: &str, method: &str) -> BackendAvailability {
        if self.capabilities.contains_key(method) {
            BackendAvailability::Available
        } else {
            BackendAvailability::UnsupportedMethod
        }
    }
}

pub struct CapabilityRouter {
    agent: AgentBackend,
    legacy: LegacyAdbBackend,
    routes: Mutex<HashMap<(String, String), AgentRouteDiagnostics>>,
}

impl CapabilityRouter {
    pub fn new(
        manager: Arc<AgentManager>,
        runner: Arc<dyn AdbRunner>,
        legacy_capabilities: impl IntoIterator<Item = LegacyCapability>,
    ) -> Self {
        Self {
            agent: AgentBackend::new(manager),
            legacy: LegacyAdbBackend::new(runner, legacy_capabilities),
            routes: Mutex::new(HashMap::new()),
        }
    }

    pub fn select(
        &self,
        serial: &str,
        method: &str,
        operation: OperationKind,
    ) -> Result<RouteDecision, RouteError> {
        self.select_from_agent_availability(
            serial,
            method,
            operation,
            self.agent.availability(serial, method),
        )
    }

    pub fn fallback_after_agent_error(
        &self,
        serial: &str,
        method: &str,
        operation: OperationKind,
        error: &AgentBackendError,
    ) -> Result<RouteDecision, RouteError> {
        let reason = match error {
            AgentBackendError::Unavailable(_) => FallbackReason::AgentUnavailable,
            AgentBackendError::UnsupportedMethod(_) => FallbackReason::UnsupportedMethod,
            _ => return Err(RouteError::AgentFailure(error.clone())),
        };
        self.select_legacy(serial, method, operation, reason)
    }

    pub fn agent(&self) -> &AgentBackend {
        &self.agent
    }

    pub fn legacy(&self) -> &LegacyAdbBackend {
        &self.legacy
    }

    pub async fn device_disconnected(&self, serial: &str) {
        self.agent.manager.device_disconnected(serial).await;
    }

    pub fn last_route(&self, serial: &str, method: &str) -> Option<AgentRouteDiagnostics> {
        self.routes
            .lock()
            .expect("Android route diagnostics lock poisoned")
            .get(&(serial.to_owned(), method.to_owned()))
            .cloned()
    }

    /// 当前 Agent 会话状态（只读诊断用，不触发连接）。
    pub fn agent_status(&self, serial: &str) -> AgentSessionStatus {
        self.agent.manager.status(serial)
    }

    /// Agent 调用错误 -> CoreError（供不经过路由决策的 typed API 复用同一映射）。
    pub fn agent_error(error: AgentBackendError) -> CoreError {
        agent_backend_core_error(error)
    }

    pub fn core_error(error: RouteError) -> CoreError {
        match error {
            RouteError::AgentUnavailable { reason, .. }
            | RouteError::ProviderUnavailable { reason, .. } => CoreError::AgentUnavailable(reason),
            RouteError::UnsupportedMethod(method)
            | RouteError::LegacyFallbackNotRegistered(method) => {
                CoreError::AgentUnavailable(format!("method is unavailable: {method}"))
            }
            RouteError::Incompatible(reason) => CoreError::AgentIncompatible(reason),
            RouteError::MutatingFallbackForbidden(method) => CoreError::Internal(format!(
                "automatic fallback is forbidden for mutating method {method}"
            )),
            RouteError::AgentFailure(error) => agent_backend_core_error(error),
        }
    }

    pub fn routes_for_serial(&self, serial: &str) -> Vec<AgentRouteDiagnostics> {
        let mut routes: Vec<_> = self
            .routes
            .lock()
            .expect("Android route diagnostics lock poisoned")
            .values()
            .filter(|record| record.serial == serial)
            .cloned()
            .collect();
        routes.sort_by(|left, right| {
            right
                .recorded_at
                .cmp(&left.recorded_at)
                .then_with(|| left.method.cmp(&right.method))
        });
        routes
    }

    fn select_from_agent_availability(
        &self,
        serial: &str,
        method: &str,
        operation: OperationKind,
        availability: BackendAvailability,
    ) -> Result<RouteDecision, RouteError> {
        match availability {
            BackendAvailability::Available => {
                let decision = self.agent_decision(serial);
                self.record(serial, method, decision.clone());
                Ok(decision)
            }
            BackendAvailability::AgentUnavailable(reason) => self
                .select_legacy(serial, method, operation, FallbackReason::AgentUnavailable)
                .map_err(|error| match error {
                    RouteError::LegacyFallbackNotRegistered(_) => RouteError::AgentUnavailable {
                        method: method.into(),
                        reason,
                    },
                    other => other,
                }),
            BackendAvailability::UnsupportedMethod => {
                self.select_legacy(serial, method, operation, FallbackReason::UnsupportedMethod)
            }
            BackendAvailability::ProviderUnavailable(reason) => {
                Err(RouteError::ProviderUnavailable {
                    method: method.into(),
                    reason,
                })
            }
            BackendAvailability::Incompatible(reason) => Err(RouteError::Incompatible(reason)),
        }
    }

    fn select_legacy(
        &self,
        serial: &str,
        method: &str,
        operation: OperationKind,
        fallback_reason: FallbackReason,
    ) -> Result<RouteDecision, RouteError> {
        if operation == OperationKind::Mutating {
            return Err(RouteError::MutatingFallbackForbidden(method.into()));
        }
        if self.legacy.availability(serial, method) != BackendAvailability::Available {
            return Err(RouteError::LegacyFallbackNotRegistered(method.into()));
        }
        let status = self.agent.manager.status(serial);
        let decision = RouteDecision {
            backend: AndroidBackendSource::LegacyAdb,
            fallback_reason: Some(fallback_reason.clone()),
            agent_version: status.agent_version,
            protocol_version: status.protocol_version,
        };
        tracing::warn!(
            serial,
            method,
            reason = fallback_reason.as_str(),
            agent_version = ?decision.agent_version,
            protocol_version = ?decision.protocol_version,
            removal_stage = self.legacy.removal_stage(method),
            "Android capability routed to Legacy ADB fallback"
        );
        self.record(serial, method, decision.clone());
        Ok(decision)
    }

    fn agent_decision(&self, serial: &str) -> RouteDecision {
        let status = self.agent.manager.status(serial);
        RouteDecision {
            backend: AndroidBackendSource::Agent,
            fallback_reason: None,
            agent_version: status.agent_version,
            protocol_version: status.protocol_version,
        }
    }

    fn record(&self, serial: &str, method: &str, decision: RouteDecision) {
        let recorded_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        self.routes
            .lock()
            .expect("Android route diagnostics lock poisoned")
            .insert(
                (serial.to_owned(), method.to_owned()),
                AgentRouteDiagnostics {
                    serial: serial.into(),
                    method: method.into(),
                    backend: decision.backend,
                    fallback_reason: decision
                        .fallback_reason
                        .map(|reason| reason.as_str().to_owned()),
                    agent_version: decision.agent_version,
                    protocol_version: decision.protocol_version,
                    recorded_at,
                },
            );
    }
}

fn map_client_error(method: &str, error: AgentClientError) -> AgentBackendError {
    match error {
        AgentClientError::TransportLost(reason) => AgentBackendError::TransportLost(reason),
        AgentClientError::DeadlineExceeded => AgentBackendError::DeadlineExceeded,
        AgentClientError::Cancelled => AgentBackendError::Cancelled,
        AgentClientError::Protocol(reason) => AgentBackendError::Protocol(reason),
        AgentClientError::Remote(error) if error.code == ErrorCode::UnsupportedMethod => {
            AgentBackendError::UnsupportedMethod(method.into())
        }
        AgentClientError::Remote(error) if error.code == ErrorCode::ProviderUnavailable => {
            AgentBackendError::ProviderUnavailable(error.message)
        }
        AgentClientError::Remote(error) if error.code == ErrorCode::IncompatibleVersion => {
            AgentBackendError::Incompatible(error.message)
        }
        AgentClientError::Remote(error) => AgentBackendError::Business(error),
    }
}

fn agent_backend_core_error(error: AgentBackendError) -> CoreError {
    match error {
        AgentBackendError::Unavailable(reason)
        | AgentBackendError::UnsupportedMethod(reason)
        | AgentBackendError::ProviderUnavailable(reason) => CoreError::AgentUnavailable(reason),
        AgentBackendError::Incompatible(reason) => CoreError::AgentIncompatible(reason),
        AgentBackendError::TransportLost(reason) => CoreError::AgentTransportLost(reason),
        AgentBackendError::DeadlineExceeded => {
            CoreError::Internal("Agent request deadline exceeded".into())
        }
        AgentBackendError::Cancelled => CoreError::Internal("Agent request was cancelled".into()),
        AgentBackendError::Protocol(reason) => CoreError::Internal(reason),
        AgentBackendError::Business(error) => {
            CoreError::Internal(format!("Agent returned {}: {}", error.code, error.message))
        }
    }
}

fn session_state_name(state: AgentSessionState) -> &'static str {
    match state {
        AgentSessionState::Disconnected => "disconnected",
        AgentSessionState::AdbOnline => "adb_online",
        AgentSessionState::Installing => "installing",
        AgentSessionState::Starting => "starting",
        AgentSessionState::Handshaking => "handshaking",
        AgentSessionState::Ready => "ready",
        AgentSessionState::Degraded => "degraded",
        AgentSessionState::Incompatible => "incompatible",
    }
}

#[cfg(test)]
mod tests {
    use agent_protocol::{CapabilityInfo, PROTOCOL_VERSION, PermissionInfo};

    use crate::db::Db;
    use crate::services::agent_artifact::AgentArtifactResolver;
    use crate::services::config_service::ConfigService;
    use crate::services::device_service::MockAdbRunner;

    use super::*;

    fn manager(runner: Arc<dyn AdbRunner>) -> Arc<AgentManager> {
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        Arc::new(AgentManager::new(
            runner,
            Arc::new(AgentArtifactResolver::new(config, None)),
        ))
    }

    fn ready_status(available: bool) -> AgentSessionStatus {
        AgentSessionStatus {
            serial: "serial-a".into(),
            state: AgentSessionState::Ready,
            agent_version: Some("0.1.0".into()),
            protocol_version: Some(PROTOCOL_VERSION),
            permissions: Some(PermissionInfo {
                shell: true,
                root: false,
                selinux_enforcing: true,
            }),
            providers: Vec::new(),
            capabilities: vec![CapabilityInfo {
                method: "device.info".into(),
                version: 1,
                provider: "shell".into(),
                available,
                unavailable_reason: (!available).then(|| "shell provider faulted".into()),
            }],
            local_port: Some(1234),
            last_error: None,
        }
    }

    #[test]
    fn agent_availability_is_capability_specific() {
        assert_eq!(
            AgentBackend::availability_from_status(&ready_status(true), "device.info"),
            BackendAvailability::Available
        );
        assert_eq!(
            AgentBackend::availability_from_status(&ready_status(true), "package.list"),
            BackendAvailability::UnsupportedMethod
        );
        assert!(matches!(
            AgentBackend::availability_from_status(&ready_status(false), "device.info"),
            BackendAvailability::ProviderUnavailable(_)
        ));
    }

    #[test]
    fn routes_available_agent_without_fallback() {
        let runner: Arc<dyn AdbRunner> = Arc::new(MockAdbRunner::new(true));
        let router = CapabilityRouter::new(manager(runner.clone()), runner, []);
        let decision = router
            .select_from_agent_availability(
                "serial-a",
                "device.info",
                OperationKind::ReadOnlyIdempotent,
                BackendAvailability::Available,
            )
            .unwrap();
        assert_eq!(decision.backend, AndroidBackendSource::Agent);
        assert_eq!(decision.fallback_reason, None);
    }

    #[test]
    fn read_only_unavailable_agent_uses_registered_legacy_and_records_reason() {
        let runner: Arc<dyn AdbRunner> = Arc::new(MockAdbRunner::new(true));
        let router = CapabilityRouter::new(
            manager(runner.clone()),
            runner,
            [LegacyCapability::new("device.info", "AR12.1")],
        );

        let decision = router
            .select("serial-a", "device.info", OperationKind::ReadOnlyIdempotent)
            .unwrap();
        assert_eq!(decision.backend, AndroidBackendSource::LegacyAdb);
        assert_eq!(
            decision.fallback_reason,
            Some(FallbackReason::AgentUnavailable)
        );
        let record = router.last_route("serial-a", "device.info").unwrap();
        assert_eq!(record.backend, decision.backend);
        assert_eq!(record.fallback_reason.as_deref(), Some("agent_unavailable"));
        assert_eq!(router.legacy().removal_stage("device.info"), Some("AR12.1"));
    }

    #[test]
    fn fallback_is_blocked_for_mutations_provider_errors_and_unregistered_methods() {
        let runner: Arc<dyn AdbRunner> = Arc::new(MockAdbRunner::new(true));
        let router = CapabilityRouter::new(
            manager(runner.clone()),
            runner,
            [LegacyCapability::new("package.uninstall", "AR12.1")],
        );
        assert!(matches!(
            router.select("serial-a", "package.uninstall", OperationKind::Mutating),
            Err(RouteError::MutatingFallbackForbidden(_))
        ));
        assert!(matches!(
            router.select("serial-a", "device.info", OperationKind::ReadOnlyIdempotent),
            Err(RouteError::AgentUnavailable { .. })
        ));
        assert!(matches!(
            router.fallback_after_agent_error(
                "serial-a",
                "package.uninstall",
                OperationKind::ReadOnlyIdempotent,
                &AgentBackendError::ProviderUnavailable("provider faulted".into())
            ),
            Err(RouteError::AgentFailure(
                AgentBackendError::ProviderUnavailable(_)
            ))
        ));
        assert!(matches!(
            router.fallback_after_agent_error(
                "serial-a",
                "package.uninstall",
                OperationKind::ReadOnlyIdempotent,
                &AgentBackendError::TransportLost("USB disconnected".into())
            ),
            Err(RouteError::AgentFailure(AgentBackendError::TransportLost(
                _
            )))
        ));
        assert!(matches!(
            router.fallback_after_agent_error(
                "serial-a",
                "package.uninstall",
                OperationKind::ReadOnlyIdempotent,
                &AgentBackendError::Business(AgentError::new(
                    ErrorCode::PermissionDenied,
                    "permission denied"
                ))
            ),
            Err(RouteError::AgentFailure(AgentBackendError::Business(_)))
        ));
    }

    /// AR5.5 矩阵腿：旧版 Agent 没有 package.list capability 时，只读列表允许回退 Legacy；
    /// 而 package.list_localized（Zygisk 专属）不在 Legacy 能力表里，必须拒绝回退。
    #[test]
    fn package_list_may_fall_back_but_localized_list_must_not() {
        let runner: Arc<dyn AdbRunner> = Arc::new(MockAdbRunner::new(true));
        let router = CapabilityRouter::new(
            manager(runner.clone()),
            runner,
            default_legacy_capabilities(),
        );

        let decision = router
            .fallback_after_agent_error(
                "serial-a",
                agent_protocol::method::PACKAGE_LIST,
                OperationKind::ReadOnlyIdempotent,
                &AgentBackendError::UnsupportedMethod(agent_protocol::method::PACKAGE_LIST.into()),
            )
            .unwrap();
        assert_eq!(decision.backend, AndroidBackendSource::LegacyAdb);
        assert_eq!(
            decision.fallback_reason,
            Some(FallbackReason::UnsupportedMethod)
        );

        for method in [
            agent_protocol::method::PACKAGE_LIST_LOCALIZED,
            agent_protocol::method::ZYGISK_STATUS,
            agent_protocol::method::PACKAGE_EXPORT_APK,
        ] {
            assert!(
                matches!(
                    router.fallback_after_agent_error(
                        "serial-a",
                        method,
                        OperationKind::ReadOnlyIdempotent,
                        &AgentBackendError::UnsupportedMethod(method.into()),
                    ),
                    // 这三个方法没有登记 Legacy 能力，路由层必须显式拒绝，
                    // 而不是悄悄换成 pm/Shell 的等价实现。
                    Err(RouteError::LegacyFallbackNotRegistered(_))
                ),
                "{method} 不得回退成 pm/Shell 结果"
            );
        }
    }

    #[test]
    fn unsupported_method_error_can_fallback_only_when_legacy_is_registered() {
        let runner: Arc<dyn AdbRunner> = Arc::new(MockAdbRunner::new(true));
        let router = CapabilityRouter::new(
            manager(runner.clone()),
            runner,
            [LegacyCapability::new("device.info", "AR12.1")],
        );
        let decision = router
            .fallback_after_agent_error(
                "serial-a",
                "device.info",
                OperationKind::ReadOnlyIdempotent,
                &AgentBackendError::UnsupportedMethod("device.info".into()),
            )
            .unwrap();
        assert_eq!(decision.backend, AndroidBackendSource::LegacyAdb);
        assert_eq!(
            decision.fallback_reason,
            Some(FallbackReason::UnsupportedMethod)
        );
    }

    #[test]
    fn default_legacy_capabilities_all_have_explicit_removal_conditions() {
        let capabilities = default_legacy_capabilities();
        assert!(!capabilities.is_empty());
        assert!(capabilities.iter().all(|capability| {
            !capability.method.is_empty() && capability.removal_stage.starts_with("AR12.1 after AR")
        }));
    }

    #[tokio::test]
    async fn legacy_backend_preserves_runner_output_and_adb_unavailable_error() {
        let runner = Arc::new(MockAdbRunner::new(true).with_script(
            &["devices"],
            MockAdbRunner::ok_output("List of devices attached\n"),
        ));
        let backend = LegacyAdbBackend::new(runner, []);
        let output = backend
            .run(&["devices".into()], Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(output.exit_code, Some(0));

        let unavailable = LegacyAdbBackend::new(Arc::new(MockAdbRunner::new(false)), []);
        assert!(
            unavailable
                .run(&["devices".into()], Duration::from_secs(1))
                .await
                .unwrap_err()
                .to_string()
                .contains("adb")
        );
    }
}

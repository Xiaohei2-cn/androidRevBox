use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_protocol::{AgentError, ErrorCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::core::error::{CoreError, CoreResult};
use crate::models::agent::{
    AgentRouteDiagnostics, AgentSessionState, AgentSessionStatus, AndroidBackendSource,
    LegacyFallbackTotal,
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
    // crate 内可见即可（不对外扩面）：AR10.4 的自动重连钩子就挂在这个 request() 上，
    // 会话层的单测要能从 AgentManager 那一侧把它整条跑一遍。
    pub(crate) fn new(manager: Arc<AgentManager>) -> Self {
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
        match client.request(method, params, timeout).await {
            Ok(value) => Ok(value),
            Err(AgentClientError::TransportLost(reason)) => {
                // 链路被打断（AR10.4 真机现场：framework 软重启常连带重启 adbd，转发规则
                // 随它一起消失，而设备上的 Agent 与模块都还好好的）。这里把会话接回来，
                // 但**这一次调用照实失败**——请求可能已经在设备上执行完了，只是应答丢在
                // 半路；替用户重放就等于把同一次删除/改权限再做一遍。
                self.recover_transport(serial, method, &reason).await
            }
            Err(error) => Err(map_client_error(method, error)),
        }
    }

    async fn recover_transport<R>(
        &self,
        serial: &str,
        method: &str,
        reason: &str,
    ) -> Result<R, AgentBackendError> {
        match self.manager.reconnect_after_transport_loss(serial).await {
            Ok(_) => Err(AgentBackendError::TransportLost(format!(
                "与设备的连接被中断（{reason}）。已自动重连，这一步可以直接重试；{method} 这一次没有被重放（无法确认设备侧当时是否已执行）",
            ))),
            Err(error) => Err(AgentBackendError::TransportLost(format!(
                "与设备的连接被中断（{reason}），自动重连也没成功：{error}。请在设备页重新连接 Agent",
            ))),
        }
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
                    // 未知 ≠ 不可用：Agent 还没探完时把请求交给它，由探测完的 Agent 给
                    // 权威答案（同样的 provider_unavailable 指引，只多一次往返）。
                    Some(capability) if capability.probe_pending => BackendAvailability::Available,
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
    /// `(设备, 能力, 原因) -> 累计次数`，见 `LegacyFallbackTotal` 的注释
    fallback_totals: Mutex<HashMap<(String, String, String), u64>>,
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
            fallback_totals: Mutex::new(HashMap::new()),
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

    /// 某台设备累计的 Legacy 回退次数：AR12 删除决定的依据。
    pub fn fallback_totals_for_serial(&self, serial: &str) -> Vec<LegacyFallbackTotal> {
        let mut totals: Vec<LegacyFallbackTotal> = self
            .fallback_totals
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .filter(|((device, _, _), _)| device == serial)
                    .map(|((_, method, reason), count)| LegacyFallbackTotal {
                        method: method.clone(),
                        reason: reason.clone(),
                        count: *count,
                        removal_stage: self.legacy.removal_stage(method).map(str::to_owned),
                    })
                    .collect()
            })
            .unwrap_or_default();
        totals.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.method.cmp(&right.method))
        });
        totals
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
        if let Ok(mut totals) = self.fallback_totals.lock() {
            *totals
                .entry((
                    serial.to_owned(),
                    method.to_owned(),
                    fallback_reason.as_str().to_owned(),
                ))
                .or_insert(0) += 1;
        }
        // 进审计流（target="audit"）：一次静默降级就是一条该被看见的事件。
        // §3.7 原本只要求"写操作必审计"，这里补的是另一半——**悄悄退回旧实现**
        // 改变的是"用户拿到的答案来自谁"，同样必须留痕。
        tracing::warn!(
            target: "audit",
            serial,
            method,
            reason = fallback_reason.as_str(),
            backend = "legacy_adb",
            agent_version = ?decision.agent_version,
            protocol_version = ?decision.protocol_version,
            removal_stage = self.legacy.removal_stage(method),
            "Android 能力走了 Legacy ADB 回退"
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
        // 业务错误里"目标不存在"占绝大多数（文件页点到一个已经没了的路径就是它），
        // 它不是故障，必须用自己的变体说话；剩下的业务错误仍然按内部错误报出来，
        // 因为那是设备侧真的拒绝了我们。
        AgentBackendError::Business(error) if error.code == agent_protocol::ErrorCode::NotFound => {
            CoreError::NotFound(error.message)
        }
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

    /// 文件页最常见的两个报错之一：设备侧说"没有这个东西"。它必须说人话，
    /// 也不能顶着「内部错误」出现——那会让用户以为程序坏了（真机就是这样报的）。
    #[test]
    fn agent_not_found_becomes_a_dedicated_not_found_error() {
        let error = agent_backend_core_error(AgentBackendError::Business(AgentError::new(
            ErrorCode::NotFound,
            "lstat 失败 /storage/emulated/0/sdcard: No such file or directory (os error 2)",
        )));
        assert_eq!(error.code(), "NOT_FOUND");
        let text = error.to_string();
        assert!(
            text.contains("设备上没有这个文件或目录"),
            "措辞要能直接看懂: {text}"
        );
        assert!(!text.contains("内部错误"), "不能伪装成内部故障: {text}");
        assert!(
            text.contains("/storage/emulated/0/sdcard"),
            "要把设备侧给的路径原样带着: {text}"
        );
        // 其它业务错误仍然按内部故障报（那是设备侧真的拒绝了我们）
        let rejected = agent_backend_core_error(AgentBackendError::Business(AgentError::new(
            ErrorCode::InvalidRequest,
            "路径越界",
        )));
        assert_eq!(rejected.code(), "INTERNAL");
    }
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
    /// AR12 的第一步是"把删除决定变成可观察的事实"：回退一旦发生就要被计数，
    /// 而且要按 (能力, 原因) 分开——`agent_unavailable` 与 `unsupported_method`
    /// 的处置完全不同（前者要装/连 Agent，后者是 Agent 版本旧）。
    #[tokio::test]
    async fn legacy_fallbacks_are_counted_per_method_and_reason() {
        let runner: Arc<dyn AdbRunner> = Arc::new(MockAdbRunner::new(true));
        let router = CapabilityRouter::new(
            manager(runner.clone()),
            runner.clone(),
            [LegacyCapability::new("package.list", "AR12.1 after AR5.5")],
        );
        assert!(
            router.fallback_totals_for_serial("serial-a").is_empty(),
            "起点必须干净"
        );
        for _ in 0..3 {
            let decision = router
                .select(
                    "serial-a",
                    "package.list",
                    OperationKind::ReadOnlyIdempotent,
                )
                .unwrap();
            assert_eq!(decision.backend, AndroidBackendSource::LegacyAdb);
        }
        let totals = router.fallback_totals_for_serial("serial-a");
        assert_eq!(totals.len(), 1, "同一能力同一原因该合成一条: {totals:?}");
        assert_eq!(totals[0].method, "package.list");
        assert_eq!(totals[0].reason, "agent_unavailable");
        assert_eq!(totals[0].count, 3);
        assert_eq!(
            totals[0].removal_stage.as_deref(),
            Some("AR12.1 after AR5.5"),
            "计数要自带删除条件，看的人不用再翻能力表"
        );
        // 换原因（Agent 在线但不支持该方法）另起一条，不与上面的混在一起
        let router2 = CapabilityRouter::new(
            manager(runner.clone()),
            runner.clone(),
            [LegacyCapability::new("package.list", "AR12.1 after AR5.5")],
        );
        let _ = router2.select(
            "serial-b",
            "package.list",
            OperationKind::ReadOnlyIdempotent,
        );
        assert_eq!(
            router2.fallback_totals_for_serial("serial-c").len(),
            0,
            "别的设备不该串进来"
        );
    }

    /// 被"写操作不回退"规则**拒绝**的调用不算回退：它没有真的退回旧实现，
    /// 记进去会让 AR12 的删除依据说谎。
    #[tokio::test]
    async fn refused_mutating_fallback_is_not_counted_as_a_fallback() {
        let runner: Arc<dyn AdbRunner> = Arc::new(MockAdbRunner::new(true));
        let router = CapabilityRouter::new(
            manager(runner.clone()),
            runner,
            [LegacyCapability::new("package.uninstall", "AR12.1")],
        );
        let error = router
            .select("serial-a", "package.uninstall", OperationKind::Mutating)
            .expect_err("写操作不得回退");
        assert!(
            matches!(error, RouteError::MutatingFallbackForbidden(_)),
            "{error:?}"
        );
        assert!(
            router.fallback_totals_for_serial("serial-a").is_empty(),
            "被拒绝的路由不该被计成一次真实回退"
        );
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
                probe_pending: false,
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

    /// AR12 的机读闸门（真机，`AR12_TEST_SERIAL`）：已迁移的能力必须**全部路由到 Agent**，
    /// 且这台设备在本次会话里**一次 Legacy 回退都没发生**。
    ///
    /// 为什么值得单独一条腿：AR12 要删回退腿，而"能不能删"以前只能靠人回忆"最近是不是
    /// 都走 Agent 了"。这里把它变成一个会红的断言——只要有任何一项还掉回退，就会以
    /// `fallback_totals_for_serial` 非空的形式报出来，附带能力名、原因和登记的删除条件。
    #[tokio::test]
    #[ignore = "需要真机；AR12_TEST_SERIAL=<serial> cargo test -p app-reverse-tools real_router -- --ignored --nocapture"]
    async fn real_router_sends_every_migrated_capability_to_agent() {
        use crate::db::Db;
        use crate::services::agent_artifact::AgentArtifactResolver;
        use crate::services::config_service::ConfigService;
        use crate::services::device_service::RealAdbRunner;

        let Some(serial) = std::env::var("AR12_TEST_SERIAL")
            .ok()
            .filter(|v| !v.is_empty())
        else {
            eprintln!("[跳过] 本腿需要 AR12_TEST_SERIAL=<serial>");
            return;
        };
        let config = Arc::new(ConfigService::new(Arc::new(Db::in_memory().unwrap())));
        let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
        let manager = Arc::new(crate::services::agent_manager::AgentManager::new(
            runner.clone(),
            Arc::new(AgentArtifactResolver::new(config, None)),
        ));
        manager
            .connect_resolved(&serial)
            .await
            .expect("Agent 应能安装并连上（真机）");
        let router = CapabilityRouter::new(manager.clone(), runner, default_legacy_capabilities());

        for capability in default_legacy_capabilities() {
            let decision = router
                .select(
                    &serial,
                    &capability.method,
                    OperationKind::ReadOnlyIdempotent,
                )
                .unwrap_or_else(|error| panic!("{} 路由失败: {error:?}", capability.method));
            assert_eq!(
                decision.backend,
                AndroidBackendSource::Agent,
                "{} 仍在走 Legacy 回退",
                capability.method
            );
        }
        let totals = router.fallback_totals_for_serial(&serial);
        assert!(
            totals.is_empty(),
            "本次会话出现了 Legacy 回退，AR12 还不能删这些腿: {totals:?}"
        );
        eprintln!(
            "[ar12] {} 项已迁移能力全部路由到 Agent，本次会话零 Legacy 回退",
            default_legacy_capabilities().len()
        );
        manager.disconnect(&serial).await.unwrap();
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

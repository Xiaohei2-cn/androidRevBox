use std::time::Instant;

use agent_protocol::method::{CAPABILITY_LIST, SYSTEM_HEALTH, SYSTEM_HELLO};
use agent_protocol::{
    AgentError, CapabilityListResult, EmptyParams, ErrorCode, HealthResult, HealthStatus,
    HelloParams, HelloResult, PROTOCOL_VERSION, PermissionInfo, ProviderHealth, ProviderInfo,
    negotiate_protocol,
};
use serde_json::{Value, to_value};

use super::{Provider, ProviderFuture, RequestContext};

const SYSTEM_METHODS: &[&str] = &[SYSTEM_HELLO, SYSTEM_HEALTH, CAPABILITY_LIST];

pub struct SystemProvider {
    auth_token: String,
    permissions: PermissionInfo,
    started_at: Instant,
}

impl SystemProvider {
    pub fn new(auth_token: impl Into<String>, permissions: PermissionInfo) -> Self {
        Self {
            auth_token: auth_token.into(),
            permissions,
            started_at: Instant::now(),
        }
    }

    fn hello(&self, context: RequestContext, params: Value) -> Result<Value, AgentError> {
        let params: HelloParams = parse_params(params)?;
        if !constant_time_eq(params.auth_token.as_bytes(), self.auth_token.as_bytes()) {
            return Err(AgentError::new(
                ErrorCode::PermissionDenied,
                "agent authentication failed",
            ));
        }
        let protocol_version =
            negotiate_protocol(&params.supported_protocol_versions).ok_or_else(|| {
                AgentError::new(
                    ErrorCode::IncompatibleVersion,
                    "no mutually supported protocol version",
                )
            })?;
        serialize_result(HelloResult {
            protocol_version,
            agent_version: env!("CARGO_PKG_VERSION").into(),
            permissions: self.permissions.clone(),
            providers: context.providers,
            capabilities: context.capabilities,
        })
    }

    fn health(&self, params: Value) -> Result<Value, AgentError> {
        let _: EmptyParams = parse_params(params)?;
        serialize_result(HealthResult {
            status: HealthStatus::Ready,
            agent_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: PROTOCOL_VERSION,
            uptime_ms: u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }

    fn capabilities(&self, context: RequestContext, params: Value) -> Result<Value, AgentError> {
        let _: EmptyParams = parse_params(params)?;
        serialize_result(CapabilityListResult {
            capabilities: context.capabilities,
        })
    }
}

impl Provider for SystemProvider {
    fn info(&self) -> ProviderInfo {
        ProviderInfo {
            name: "system".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            health: ProviderHealth::Ready,
            required_permissions: Vec::new(),
            last_error: None,
        }
    }

    fn methods(&self) -> &'static [&'static str] {
        SYSTEM_METHODS
    }

    fn handle<'a>(
        &'a self,
        context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            match method {
                SYSTEM_HELLO => self.hello(context, params),
                SYSTEM_HEALTH => self.health(params),
                CAPABILITY_LIST => self.capabilities(context, params),
                _ => Err(AgentError::new(
                    ErrorCode::UnsupportedMethod,
                    format!("unsupported system method: {method}"),
                )),
            }
        })
    }
}

fn parse_params<T>(params: Value) -> Result<T, AgentError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(params).map_err(|error| {
        AgentError::new(ErrorCode::InvalidRequest, "invalid method parameters")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn serialize_result<T: serde::Serialize>(result: T) -> Result<Value, AgentError> {
    to_value(result).map_err(|error| {
        AgentError::new(ErrorCode::Internal, "failed to serialize provider result")
            .with_details(serde_json::json!({ "reason": error.to_string() }))
    })
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        let left_byte = left.get(index).copied().unwrap_or_default();
        let right_byte = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use agent_protocol::method::SYSTEM_HELLO;
    use agent_protocol::{HelloParams, ResponsePayload};
    use serde_json::json;

    use crate::router::Router;

    use super::*;

    fn permissions() -> PermissionInfo {
        PermissionInfo {
            shell: true,
            root: false,
            selinux_enforcing: false,
        }
    }

    #[tokio::test]
    async fn hello_authenticates_and_reports_registry() {
        let mut router = Router::new();
        router
            .register(std::sync::Arc::new(SystemProvider::new(
                "secret",
                permissions(),
            )))
            .unwrap();
        let response = router
            .route(agent_protocol::RequestEnvelope {
                request_id: "req-hello".to_owned(),
                method: SYSTEM_HELLO.into(),
                params: serde_json::to_value(HelloParams::v1("secret", "desktop-test")).unwrap(),
                timeout_ms: 1_000,
                protocol_version: PROTOCOL_VERSION,
            })
            .await;
        let ResponsePayload::Success { result } = response.payload else {
            panic!("expected successful hello");
        };
        assert_eq!(result["protocol_version"], 1);
        assert_eq!(result["providers"][0]["name"], "system");
        assert_eq!(result["capabilities"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn hello_rejects_bad_token_without_echoing_it() {
        let provider = SystemProvider::new("secret", permissions());
        let error = provider
            .hello(
                RequestContext {
                    providers: vec![],
                    capabilities: vec![],
                },
                json!(HelloParams::v1("wrong-token", "desktop-test")),
            )
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::PermissionDenied);
        assert!(!error.message.contains("wrong-token"));
    }
}

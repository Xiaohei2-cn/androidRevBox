use std::collections::HashMap;
use std::sync::Arc;

use agent_protocol::{
    AgentError, CapabilityInfo, ErrorCode, ProviderInfo, RequestEnvelope, ResponseEnvelope,
};

use crate::provider::{Provider, RequestContext};

#[derive(Default)]
pub struct Router {
    providers: Vec<Arc<dyn Provider>>,
    methods: HashMap<&'static str, Arc<dyn Provider>>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn Provider>) -> Result<(), AgentError> {
        for method in provider.methods() {
            if self.methods.contains_key(method) {
                return Err(AgentError::new(
                    ErrorCode::Internal,
                    format!("duplicate provider method registration: {method}"),
                ));
            }
        }
        for method in provider.methods() {
            self.methods.insert(method, provider.clone());
        }
        self.providers.push(provider);
        Ok(())
    }

    pub async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, AgentError> {
        let provider = self.methods.get(method).ok_or_else(|| {
            AgentError::new(
                ErrorCode::UnsupportedMethod,
                format!("unsupported method: {method}"),
            )
        })?;
        provider.handle(self.context(), method, params).await
    }

    pub async fn route(&self, request: RequestEnvelope) -> ResponseEnvelope {
        let request_id = request.request_id;
        match self.dispatch(&request.method, request.params).await {
            Ok(result) => ResponseEnvelope::success(request_id, result),
            Err(error) => ResponseEnvelope::failure(request_id, error),
        }
    }

    pub fn provider_info(&self) -> Vec<ProviderInfo> {
        self.providers
            .iter()
            .map(|provider| provider.info())
            .collect()
    }

    pub fn capabilities(&self) -> Vec<CapabilityInfo> {
        let mut capabilities: Vec<_> = self
            .providers
            .iter()
            .flat_map(|provider| {
                let provider = Arc::clone(provider);
                let info = provider.info();
                provider.methods().iter().map(move |method| {
                    let unavailable_reason = provider.unavailable_reason(method);
                    CapabilityInfo {
                        method: (*method).into(),
                        version: 1,
                        provider: info.name.clone(),
                        available: unavailable_reason.is_none(),
                        unavailable_reason,
                    }
                })
            })
            .collect();
        capabilities.sort_by(|left, right| left.method.cmp(&right.method));
        capabilities
    }

    fn context(&self) -> RequestContext {
        RequestContext {
            providers: self.provider_info(),
            capabilities: self.capabilities(),
        }
    }
}

#[cfg(test)]
mod tests {
    use agent_protocol::ErrorCode;

    use super::*;

    #[tokio::test]
    async fn unknown_method_is_structured_error() {
        let router = Router::new();
        let error = router
            .dispatch("unknown.method", serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsupportedMethod);
    }
}

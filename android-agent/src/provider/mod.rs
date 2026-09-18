pub mod device;
pub mod system;

use std::future::Future;
use std::pin::Pin;

use agent_protocol::{AgentError, CapabilityInfo, ProviderInfo};
use serde_json::Value;

pub type ProviderFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, AgentError>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub providers: Vec<ProviderInfo>,
    pub capabilities: Vec<CapabilityInfo>,
}

pub trait Provider: Send + Sync {
    fn info(&self) -> ProviderInfo;

    fn methods(&self) -> &'static [&'static str];

    fn handle<'a>(
        &'a self,
        context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a>;
}

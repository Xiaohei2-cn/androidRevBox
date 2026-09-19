pub mod activity;
pub mod device;
pub mod filesystem;
pub mod hosted;
pub mod process;
pub mod system;
pub mod zygisk;

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

    /// 依赖外部组件（例如已安装的 Zygisk 模块）的 Provider 覆盖此方法，
    /// 让 `system.hello` / `capability.list` 报告真实可用性；返回 `None` 表示可用。
    fn unavailable_reason(&self, _method: &str) -> Option<String> {
        None
    }

    fn handle<'a>(
        &'a self,
        context: RequestContext,
        method: &'a str,
        params: Value,
    ) -> ProviderFuture<'a>;
}

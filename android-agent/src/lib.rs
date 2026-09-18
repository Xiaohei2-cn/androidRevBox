pub mod provider;
pub mod router;
pub mod server;

use std::sync::Arc;

use agent_protocol::PermissionInfo;
use provider::device::DeviceProvider;
use provider::system::SystemProvider;
use router::Router;

pub fn system_router(auth_token: impl Into<String>, permissions: PermissionInfo) -> Router {
    let mut router = Router::new();
    router
        .register(Arc::new(SystemProvider::new(auth_token, permissions)))
        .expect("system provider methods must be unique");
    router
        .register(Arc::new(DeviceProvider))
        .expect("device provider methods must be unique");
    router
}

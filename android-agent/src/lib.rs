pub mod provider;
pub mod router;
pub mod server;

use std::sync::Arc;

use agent_protocol::PermissionInfo;
use provider::activity::ActivityProvider;
use provider::device::DeviceProvider;
use provider::filesystem::FilesystemProvider;
use provider::process::ProcessesProvider;
use provider::system::SystemProvider;
use provider::zygisk::ZygiskProvider;
use router::Router;

pub fn system_router(auth_token: impl Into<String>, permissions: PermissionInfo) -> Router {
    system_router_with(auth_token, permissions, Arc::new(ZygiskProvider::new()))
}

/// 显式传入 Zygisk Provider，便于 Agent 启动前先做一次真实探测，
/// 让首个 `system.hello` 就带上准确的 capability 可用性。
pub fn system_router_with(
    auth_token: impl Into<String>,
    permissions: PermissionInfo,
    zygisk: Arc<ZygiskProvider>,
) -> Router {
    let mut router = Router::new();
    router
        .register(Arc::new(SystemProvider::new(auth_token, permissions)))
        .expect("system provider methods must be unique");
    router
        .register(Arc::new(DeviceProvider))
        .expect("device provider methods must be unique");
    router
        .register(zygisk)
        .expect("zygisk provider methods must be unique");
    router
        .register(Arc::new(ActivityProvider))
        .expect("activity provider methods must be unique");
    router
        .register(Arc::new(ProcessesProvider))
        .expect("process provider methods must be unique");
    router
        .register(Arc::new(FilesystemProvider))
        .expect("filesystem provider methods must be unique");
    router
}

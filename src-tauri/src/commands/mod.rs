//! IPC 命令层：只做参数校验与 Service 调用，禁止写业务逻辑。

pub mod config;
pub mod device;
pub mod plugins;
pub mod system;
pub mod task;

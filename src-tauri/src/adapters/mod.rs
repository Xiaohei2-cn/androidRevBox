//! 外部适配器（P2 进程执行 / P3 ADB 起填充）：
//! adb CLI、shell 适配、filesystem 等第三方工具封装。

pub mod adb;
pub mod agent_bootstrap;
pub mod agent_transport;
pub mod frida;

//! 业务服务层：Command 只转发，业务规则在这里。
//! 阶段规划：P1 config/log → P2 task/process → P3 device → P4 plugin → P5 crypto → P6 插件中心 → P7 环境探测。

pub mod config_service;
pub mod device_service;
pub mod env_service;
pub mod log_service;
pub mod plugin_service;
pub mod process_service;
pub mod task_service;

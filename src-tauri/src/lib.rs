//! 应用入口：分层结构见 docs/PHASES.md。
//! - commands: IPC 边界，只做参数校验与调用
//! - services: 业务服务层（P1: config/log）
//! - core:     通用核心类型/事件/错误
//! - adapters: adb / shell / filesystem 外部适配器（P2/P3 起填充）
//! - plugins:  插件发现、加载、ABI、生命周期（P4 起填充）
//! - db:       SQLite schema / repository
//! - models:   Rust domain models（DTO）

mod adapters;
mod commands;
mod core;
mod db;
mod models;
mod plugins;
mod services;

use std::sync::Arc;

use tauri::Manager;

use crate::db::Db;
use crate::services::config_service::ConfigService;
use crate::services::log_service::{self, LogService};
use crate::services::task_service::TaskService;

/// 全局共享状态：Service 实例（Arc 化，供各 command 经 tauri::State 取用）
pub struct AppState {
    pub config: Arc<ConfigService>,
    pub log: Arc<LogService>,
    pub task: Arc<TaskService>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // 1) 数据库：打开 app data 目录下的 SQLite 并执行增量 migration
            let data_dir = app.path().app_data_dir()?;
            let db = Arc::new(Db::connect(&data_dir)?);

            // 2) ConfigService：设置读写（键与值校验在 Service）
            let config = Arc::new(ConfigService::new(db.clone()));

            // 3) LogService：按持久化级别初始化 tracing（非法值回退 info）
            let log_dir = app.path().app_log_dir()?.join("logs");
            let initial_level = LogService::resolve_startup_level(&config);
            let log = log_service::init(&log_dir, &initial_level, config.clone())?;

            app.manage(AppState {
                config: config.clone(),
                log,
                task: Arc::new(TaskService::new(db.clone(), app.handle().clone())),
            });
            tracing::info!(
                version = env!("CARGO_PKG_VERSION"),
                log_level = initial_level.as_str(),
                "application started"
            );
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::system::system_ping,
            commands::config::config_snapshot,
            commands::config::config_get,
            commands::config::config_set,
            commands::config::log_set_level,
            commands::task::task_run,
            commands::task::task_cancel,
            commands::task::task_list,
            commands::task::task_logs
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

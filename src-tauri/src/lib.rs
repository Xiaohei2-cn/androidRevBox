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
pub mod plugins; // pub 供集成测试（tests/plugin_abi_e2e.rs）访问
mod services;

use std::sync::Arc;

use tauri::Manager;

use crate::db::Db;
use crate::services::config_service::ConfigService;
use crate::services::device_service::{AdbRunner, DeviceService, RealAdbRunner};
use crate::services::env_service::EnvService;
use crate::services::log_service::{self, LogService};
use crate::services::plugin_service::PluginService;
use crate::services::task_service::TaskService;

/// 全局共享状态：Service 实例（Arc 化，供各 command 经 tauri::State 取用）
pub struct AppState {
    pub config: Arc<ConfigService>,
    pub log: Arc<LogService>,
    pub task: Arc<TaskService>,
    pub device: Arc<DeviceService>,
    pub plugins: Arc<PluginService>,
    pub env: Arc<EnvService>,
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

            let task_service = Arc::new(TaskService::new(db.clone(), app.handle().clone()));

            // 4) DeviceService：adb 能力（P3）。runner 为 trait 对象，DeviceService 与
            //    EnvService（P7）共享同一实例（环境缓存只解析一次）
            let runner: Arc<dyn AdbRunner> = Arc::new(RealAdbRunner::new(config.clone()));
            let device = Arc::new(DeviceService::new(
                runner.clone(),
                task_service.clone(),
                db.clone(),
                app.handle().clone(),
            ));
            device.clone().start_watch();

            // 4.5) EnvService：仪表盘环境/工具探测（P7），复用同一 adb runner
            let env = Arc::new(EnvService::new(config.clone(), runner.clone()));

            // 5) PluginService：受控目录 app_data/plugins，启动即扫描加载（P4/P6）
            let plugin_root = data_dir.join("plugins");
            let plugins = Arc::new(PluginService::new(
                db.clone(),
                config.clone(),
                plugin_root,
                Arc::new(services::plugin_service::TauriPluginEventSink(
                    app.handle().clone(),
                )),
            ));
            match plugins.scan() {
                Ok(report) => {
                    tracing::info!(
                        loaded = ?report.loaded,
                        failed = report.errors.len(),
                        "plugin scan finished"
                    );
                    for e in &report.errors {
                        tracing::warn!(dir = %e.dir, error = %e.error, "插件加载失败");
                    }
                }
                Err(e) => tracing::error!(error = %e, "插件扫描异常"),
            }

            app.manage(AppState {
                config: config.clone(),
                log,
                task: task_service,
                device,
                plugins,
                env,
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
            commands::task::task_logs,
            commands::device::adb_environment,
            commands::device::adb_set_path,
            commands::device::devices_list,
            commands::device::devices_watch_now,
            commands::device::device_info,
            commands::device::device_ls,
            commands::device::device_packages,
            commands::device::device_shell,
            commands::device::device_install,
            commands::device::device_uninstall,
            commands::device::device_launch,
            commands::device::device_force_stop,
            commands::device::device_push,
            commands::device::device_pull,
            commands::device::device_logcat,
            commands::env::env_python,
            commands::env::env_node,
            commands::env::env_frida,
            commands::env::env_ida_mcp,
            commands::env::env_jadx_mcp,
            commands::env::env_foreground,
            commands::env::env_overview,
            commands::plugins::plugins_list,
            commands::plugins::plugins_scan,
            commands::plugins::plugins_call,
            commands::plugins::plugins_install,
            commands::plugins::plugins_rollback,
            commands::plugins::plugins_uninstall,
            commands::plugins::plugins_set_enabled
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // 插件 shutdown 先于动态库关闭（loaded 表随 state drop 也兜底）
                if let Some(state) = app.try_state::<AppState>() {
                    state.plugins.unload_all();
                }
            }
        });
}

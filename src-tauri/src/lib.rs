//! 应用入口：当前重构计划与分层边界见 docs/android_agent_refactor_phases.md。
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

pub use adapters::agent_transport;
pub use models::agent;
pub use services::agent_client;
pub use services::agent_manager;
pub use services::android_backend;

use std::sync::Arc;

use tauri::Manager;

use crate::db::Db;
use crate::services::agent_artifact::{AGENT_RESOURCE_ARM64, AgentArtifactResolver};
use crate::services::agent_auto::AgentAutoService;
use crate::services::agent_manager::AgentManager;
use crate::services::ai_service;
use crate::services::android_backend::{CapabilityRouter, default_legacy_capabilities};
use crate::services::config_service::ConfigService;
use crate::services::device_service::{AdbRunner, DeviceService, RealAdbRunner};
use crate::services::env_service::EnvService;
use crate::services::hook_service::HookService;
use crate::services::log_service::{self, LogService};
use crate::services::plugin_service::PluginService;
use crate::services::pty_service::PtyService;
use crate::services::task_service::TaskService;
use crate::services::zygisk_applist::ZygiskApplistService;

/// 全局共享状态：Service 实例（Arc 化，供各 command 经 tauri::State 取用）
pub struct AppState {
    pub config: Arc<ConfigService>,
    pub log: Arc<LogService>,
    pub task: Arc<TaskService>,
    pub device: Arc<DeviceService>,
    pub plugins: Arc<PluginService>,
    pub env: Arc<EnvService>,
    pub hook: Arc<HookService>,
    /// 真终端会话（FT0.2）：本机 PTY 与它的原样字节流；一个桌面一个会话表
    pub pty: Arc<PtyService>,
    pub agent: Arc<AgentManager>,
    pub android: Arc<CapabilityRouter>,
    /// 设备上线自动连接 Agent（AR12.5）：探测结论 + 一次性授权表都在这层
    pub agent_auto: Arc<AgentAutoService>,
    pub zygisk_applist: Arc<ZygiskApplistService>,
    /// 翻译接口（只在本机发起，key 不出桌面）
    pub ai: Arc<ai_service::AiService>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            let agent_resource = app
                .path()
                .resolve(AGENT_RESOURCE_ARM64, tauri::path::BaseDirectory::Resource)
                .ok();
            let agent_artifacts =
                Arc::new(AgentArtifactResolver::new(config.clone(), agent_resource));
            let agent = Arc::new(AgentManager::new(runner.clone(), agent_artifacts));
            let android = Arc::new(CapabilityRouter::new(
                agent.clone(),
                runner.clone(),
                default_legacy_capabilities(),
            ));
            // AR12.5：自动连接。它要 AgentManager（探测/连接）与 ConfigService
            // （总开关 + 每台设备的一次性授权），所以只能排在两者之后。
            let agent_auto = Arc::new(AgentAutoService::new(agent.clone(), config.clone()));
            let device = Arc::new(DeviceService::new(
                runner.clone(),
                android.clone(),
                task_service.clone(),
                db.clone(),
                app.handle().clone(),
            ));
            // 接线必须在 start_watch 之前：watch 一起来就会把当前在线的设备当成
            // "刚上线"，晚一步接的话，第一次插线的自动连接就丢了。
            device.set_agent_auto(agent_auto.clone());
            device.clone().start_watch();
            let zygisk_applist =
                Arc::new(ZygiskApplistService::new(android.clone(), runner.clone()));

            // 翻译服务：配置与调用都留在桌面，key 不下发设备（UI-6 第二层）
            let ai = Arc::new(ai_service::AiService::new(config.clone()));

            // 4.5) EnvService：仪表盘环境/工具探测（P7），复用同一 adb runner
            let env = Arc::new(EnvService::new(
                config.clone(),
                runner.clone(),
                android.clone(),
            ));

            // 4.6) HookService：Frida 会话工作台（P10）——js 扫描/runner 组装/前置检查
            let hook = Arc::new(HookService::new(
                config.clone(),
                env.clone(),
                task_service.clone(),
                app.handle().clone(),
            ));

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

            let pty = Arc::new(PtyService::new());

            // 陈旧 adb forward 清扫（AR12.3 补齐 + AR12.7 配套）：adb server 比 App 活得久，
            // 被 kill 掉的那一任留下的规则一直挂在 `adb forward --list` 里。启动时扫一次，
            // 不阻塞 setup，也不因为"没扫干净"让任何流程失败。
            {
                let swept = agent.clone();
                tauri::async_runtime::spawn(async move {
                    let removed = swept.sweep_stale_forwards().await;
                    if !removed.is_empty() {
                        tracing::info!(count = removed.len(), rules = ?removed, "清掉了上一任桌面进程留下的 Agent 转发");
                    }
                });
            }

            app.manage(AppState {
                config: config.clone(),
                log,
                task: task_service,
                device,
                plugins,
                env,
                hook,
                pty,
                agent,
                android,
                agent_auto,
                zygisk_applist,
                ai,
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
            commands::window::window_pointer_state,
            commands::window::window_set_click_through,
            commands::agent::agent_status,
            commands::agent::agent_statuses,
            commands::agent::agent_install,
            commands::agent::agent_restart,
            commands::agent::agent_diagnostics,
            commands::agent::agent_probe,
            commands::config::config_snapshot,
            commands::config::config_get,
            commands::config::config_set,
            commands::config::log_set_level,
            commands::task::task_run,
            commands::task::task_cancel,
            commands::task::task_list,
            commands::task::task_logs,
            commands::zygisk::zygisk_status,
            commands::zygisk::package_list_localized,
            commands::zygisk::package_export_apk,
            commands::device::adb_environment,
            commands::device::adb_set_path,
            commands::device::devices_list,
            commands::device::devices_watch_now,
            commands::device::device_info,
            commands::device::device_ip,
            commands::device::device_root_check,
            commands::device::adb_forward_setup,
            commands::device::adb_forward_list,
            commands::device::adb_forward_remove,
            commands::device::device_binaries,
            commands::device::device_binary_su_check,
            commands::device::device_binary_chmod,
            commands::ai::ai_config_get,
            commands::ai::ai_config_set,
            commands::ai::ai_translate,
            commands::device::device_binary_probe,
            commands::device::device_binary_run,
            commands::device::device_hosted_write,
            commands::device::device_binary_kill,
            commands::device::device_binary_ports,
            commands::device::device_proc_ports,
            commands::device::device_proc_by_port,
            commands::device::device_pkg_lib_dir,
            commands::device::device_proc_read,
            commands::device::device_frida_server_status,
            commands::device::device_frida_server_start,
            commands::device::device_frida_server_stop,
            commands::device::device_so_replace,
            commands::device::device_ls,
            commands::device::device_file_stat,
            commands::device::device_fs_mkdir,
            commands::device::device_fs_rename,
            commands::device::device_fs_remove,
            commands::device::device_fs_chmod,
            commands::device::device_file_preview,
            commands::device::device_hosted_runs,
            commands::device::device_hosted_stop,
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
            commands::env::env_resolve_interpreter,
            commands::env::env_python_start_dir,
            commands::plugins::plugins_list,
            commands::plugins::plugins_scan,
            commands::plugins::plugins_call,
            commands::plugins::plugins_install,
            commands::plugins::plugins_rollback,
            commands::plugins::plugins_uninstall,
            commands::plugins::plugins_set_enabled,
            commands::hook::hook_js_list,
            commands::hook::hook_preflight,
            commands::hook::hook_session_start,
            commands::hook::hook_export_save,
            commands::pty::pty_open,
            commands::pty::pty_write,
            commands::pty::pty_resize,
            commands::pty::pty_close,
            commands::pty::pty_count,
            commands::pty::pty_default_program,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // 先放掉自己的 adb forward（AR12.7 配套）。不停 Agent：它会在宽限期后自己走，
                // 桌面抢着 kill 它就把「重启工具」变成「打断正在分析的目标进程」。
                // 退出路径上不能无限等：adb 卡住时给 2 s 预算，超时就放弃（规则留着，
                // 下一次启动的清扫会收掉——那才是它的归宿）。
                if let Some(state) = app.try_state::<AppState>() {
                    let manager = state.agent.clone();
                    let _ = tauri::async_runtime::block_on(tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        manager.release_forwards(),
                    ));
                }
                // 插件 shutdown 先于动态库关闭（loaded 表随 state drop 也兜底）
                if let Some(state) = app.try_state::<AppState>() {
                    state.plugins.unload_all();
                }
            }
        });
}

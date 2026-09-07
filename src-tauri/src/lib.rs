//! 应用入口：分层结构见 docs/PHASES.md P0。
//! - commands: IPC 边界，只做参数校验与调用
//! - services: 业务服务层（P1 起填充）
//! - core:     通用核心类型/事件/错误
//! - adapters: adb / shell / filesystem 外部适配器（P2/P3 起填充）
//! - plugins:  插件发现、加载、ABI、生命周期（P4 起填充）
//! - db:       SQLite schema / repository（P1 起填充）
//! - models:   Rust domain models（P1 起填充）

mod adapters;
mod commands;
mod core;
mod db;
mod models;
mod plugins;
mod services;

use tauri::Manager;
use tracing_subscriber::prelude::*;

fn init_tracing(app: &tauri::AppHandle) -> anyhow::Result<()> {
    let log_dir = app.path().app_log_dir()?.join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let file_writer = tracing_appender::rolling::never(&log_dir, "app.log");
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false),
        )
        .init();
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            init_tracing(app.handle())?;
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "application started");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::system::system_ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

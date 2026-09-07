//! 系统信息命令：P0 用于验证前端 → Tauri Command → 前端通路。

use serde::Serialize;

use crate::core::error::CoreResult;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub app_version: String,
    pub tauri_version: String,
    pub os: String,
    pub arch: String,
}

#[tauri::command]
pub fn system_ping(app: tauri::AppHandle) -> CoreResult<SystemInfo> {
    let app_version = app
        .config()
        .version
        .clone()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    Ok(SystemInfo {
        app_version,
        tauri_version: tauri::VERSION.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    })
}

//! 配置命令：前端设置页读写 SQLite（P1）。
//! Command 层只做 DTO 转换与转发，校验在 ConfigService/LogService。
//! 错误统一走 CoreError（序列化为 {code, message}，形状由 core::ipc::IpcError 约定）。

use serde::Deserialize;

use crate::AppState;
use crate::core::error::CoreResult;
use crate::models::config::AppSettingDto;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSetArgs {
    pub key: String,
    /// 值统一以字符串存取（语义校验在 Service）
    pub value: String,
}

#[tauri::command]
pub fn config_snapshot(state: tauri::State<'_, AppState>) -> CoreResult<Vec<AppSettingDto>> {
    state.config.snapshot()
}

#[tauri::command]
pub fn config_get(
    state: tauri::State<'_, AppState>,
    key: String,
    default: String,
) -> CoreResult<String> {
    state.config.get(&key, &default)
}

#[tauri::command]
pub fn config_set(state: tauri::State<'_, AppState>, args: ConfigSetArgs) -> CoreResult<()> {
    state.config.set(&args.key, &args.value)
}

/// 日志级别单独成命令：写入还要联动 LogService 的运行时重载。
#[tauri::command]
pub fn log_set_level(state: tauri::State<'_, AppState>, level: String) -> CoreResult<String> {
    state.log.set_level(&level)
}

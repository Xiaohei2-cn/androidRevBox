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
pub async fn config_set(state: tauri::State<'_, AppState>, args: ConfigSetArgs) -> CoreResult<()> {
    state.config.set(&args.key, &args.value)?;
    // AR12.5：自动连接开关一翻，**已经插着的那台**不该等到"下次插线"才生效。
    // `reprobe()` 清掉 watch 的已知快照，下一轮轮询就把在线设备重新当成"刚上线"，
    // 自动连接随即跟上——写入仍然受每台设备的一次性授权管，这里只是重新起探。
    if args.key == crate::services::config_service::KEY_AGENT_AUTO_CONNECT {
        state.device.reprobe().await;
    }
    Ok(())
}

/// 日志级别单独成命令：写入还要联动 LogService 的运行时重载。
#[tauri::command]
pub fn log_set_level(state: tauri::State<'_, AppState>, level: String) -> CoreResult<String> {
    state.log.set_level(&level)
}

//! 环境探测命令层（P7）：各环境卡独立刷新 + 聚合 overview。
//! 只做转发；探测、剪枝、解析全在 EnvService。
//! ADB 卡继续复用 commands/device.rs 的 adb_environment（同一 runner）。

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::env_service::{
    EnvOverview, ForegroundApp, FridaEnv, McpEnv, NodeEnv, PythonEnv,
};

#[tauri::command]
pub async fn env_python(state: tauri::State<'_, AppState>) -> CoreResult<PythonEnv> {
    Ok(state.env.python().await)
}

#[tauri::command]
pub async fn env_node(state: tauri::State<'_, AppState>) -> CoreResult<NodeEnv> {
    Ok(state.env.node().await)
}

#[tauri::command]
pub async fn env_frida(state: tauri::State<'_, AppState>) -> CoreResult<FridaEnv> {
    Ok(state.env.frida().await)
}

#[tauri::command]
pub async fn env_ida_mcp(state: tauri::State<'_, AppState>) -> CoreResult<McpEnv> {
    Ok(state.env.ida_mcp().await)
}

#[tauri::command]
pub async fn env_jadx_mcp(state: tauri::State<'_, AppState>) -> CoreResult<McpEnv> {
    Ok(state.env.jadx_mcp().await)
}

/// 安卓前台应用：按设备查询（P8 设备页）；serial 缺省自动选第一台在线设备
#[tauri::command]
pub async fn env_foreground(
    state: tauri::State<'_, AppState>,
    args: Option<ForegroundArgs>,
) -> CoreResult<ForegroundApp> {
    let serial = args.and_then(|a| a.serial).filter(|s| !s.is_empty());
    Ok(state.env.foreground_on(serial).await)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForegroundArgs {
    pub serial: Option<String>,
}

/// 仪表盘聚合：一次拿全部环境卡（前端各卡仍可独立刷新）
#[tauri::command]
pub async fn env_overview(state: tauri::State<'_, AppState>) -> CoreResult<EnvOverview> {
    Ok(state.env.overview().await)
}

/// 文件选择器解析：把选中的「错误入口」（pyenv shim / .app / symlink）解析成真解释器
#[tauri::command]
pub async fn env_resolve_interpreter(
    state: tauri::State<'_, AppState>,
    picked: String,
) -> CoreResult<crate::services::env_service::ResolvedInterpreter> {
    Ok(state.env.resolve_interpreter(&picked).await)
}

/// python 选择器建议起始目录（pyenv versions 优先）
#[tauri::command]
pub async fn env_python_start_dir(state: tauri::State<'_, AppState>) -> CoreResult<String> {
    Ok(state.env.python_start_dir().await)
}

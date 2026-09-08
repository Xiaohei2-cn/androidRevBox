//! 插件命令层（P6）：列表 / 扫描 / 调用 / 安装升级 / 回滚 / 卸载 / 启停。
//! 只做参数校验与 Service 转发；业务规则在 PluginService。

use serde::Serialize;

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::services::plugin_service::PluginView;

#[tauri::command]
pub fn plugins_list(state: tauri::State<'_, AppState>) -> CoreResult<Vec<PluginView>> {
    state.plugins.list()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginsScanResult {
    pub loaded: Vec<String>,
    pub errors: Vec<PluginScanError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginScanError {
    pub dir: String,
    pub error: String,
}

#[tauri::command]
pub fn plugins_scan(state: tauri::State<'_, AppState>) -> CoreResult<PluginsScanResult> {
    let report = state.plugins.scan()?;
    Ok(PluginsScanResult {
        loaded: report.loaded,
        errors: report
            .errors
            .into_iter()
            .map(|e| PluginScanError {
                dir: e.dir,
                error: e.error,
            })
            .collect(),
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCallResult {
    /// C ABI / 进程协议返回码（0=成功）
    pub code: i32,
    /// 输出按 UTF-8 文本呈现；插件输出为 JSON（协议见各插件 README）
    pub output: String,
}

#[tauri::command]
pub fn plugins_call(
    state: tauri::State<'_, AppState>,
    id: String,
    input: String,
) -> CoreResult<PluginCallResult> {
    let (code, bytes) = state.plugins.call(&id, input.as_bytes())?;
    match String::from_utf8(bytes) {
        Ok(output) => Ok(PluginCallResult { code, output }),
        Err(_) => Err(CoreError::Internal(format!("插件 {id} 输出不是合法 UTF-8"))),
    }
}

/// 安装 / 升级：source 为含 manifest.json 与当前平台产物的插件目录。
/// 已安装同 id 时按「升级」处理（要求更高版本，失败自动回滚）。
#[tauri::command]
pub fn plugins_install(
    state: tauri::State<'_, AppState>,
    source: String,
) -> CoreResult<PluginView> {
    let src = std::path::PathBuf::from(&source);
    if !src.is_dir() {
        return Err(CoreError::Internal(format!("插件源目录不存在: {source}")));
    }
    state.plugins.install(&src)
}

#[tauri::command]
pub fn plugins_rollback(state: tauri::State<'_, AppState>, id: String) -> CoreResult<PluginView> {
    state.plugins.rollback(&id)
}

#[tauri::command]
pub fn plugins_uninstall(state: tauri::State<'_, AppState>, id: String) -> CoreResult<()> {
    state.plugins.uninstall(&id)
}

#[tauri::command]
pub fn plugins_set_enabled(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> CoreResult<PluginView> {
    state.plugins.set_enabled(&id, enabled)
}

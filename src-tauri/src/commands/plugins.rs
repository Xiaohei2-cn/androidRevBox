//! 插件命令层（P4）：列表 / 扫描刷新 / 调用。
//! 安装/启停/升级 UI 属 P6；本阶段提供验证闭环所需最小命令集。

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
    /// C ABI 返回码（0=成功）
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

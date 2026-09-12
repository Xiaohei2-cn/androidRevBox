//! 插件命令层（P6）：列表 / 扫描 / 调用 / 安装升级 / 回滚 / 卸载 / 启停。
//! 只做参数校验与 Service 转发；业务规则在 PluginService。
//!
//! ⚠️ 全部 async + spawn_blocking：PluginService 是同步阻塞实现（dlopen / 文件 IO /
//! 插件调用最长可达配置超时 30s+，超时路径还可能等卡死的 worker），Tauri 同步命令
//! 在主线程执行会冻结 UI，绝不能直接跑。

use serde::Serialize;

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::services::plugin_service::PluginView;

/// spawn_blocking 桥接：await join 结果并把 JoinError 转成 CoreError
async fn join_core<T: Send + 'static>(
    handle: tauri::async_runtime::JoinHandle<CoreResult<T>>,
) -> CoreResult<T> {
    handle
        .await
        .map_err(|e| CoreError::Internal(format!("插件任务调度失败: {e}")))?
}

#[tauri::command]
pub async fn plugins_list(state: tauri::State<'_, AppState>) -> CoreResult<Vec<PluginView>> {
    let plugins = state.plugins.clone();
    join_core(tauri::async_runtime::spawn_blocking(move || plugins.list())).await
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
pub async fn plugins_scan(state: tauri::State<'_, AppState>) -> CoreResult<PluginsScanResult> {
    let plugins = state.plugins.clone();
    let report =
        join_core(tauri::async_runtime::spawn_blocking(move || plugins.scan())).await?;
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
pub async fn plugins_call(
    state: tauri::State<'_, AppState>,
    id: String,
    input: String,
) -> CoreResult<PluginCallResult> {
    let plugins = state.plugins.clone();
    let id_for_err = id.clone();
    let (code, bytes) =
        join_core(tauri::async_runtime::spawn_blocking(move || {
            plugins.call(&id, input.as_bytes())
        }))
        .await?;
    match String::from_utf8(bytes) {
        Ok(output) => Ok(PluginCallResult { code, output }),
        Err(_) => Err(CoreError::Internal(format!(
            "插件 {id_for_err} 输出不是合法 UTF-8"
        ))),
    }
}
/// 安装 / 升级：source 为含 manifest.json 与当前平台产物的插件目录。
/// 已安装同 id 时按「升级」处理（要求更高版本，失败自动回滚）。
#[tauri::command]
pub async fn plugins_install(
    state: tauri::State<'_, AppState>,
    source: String,
) -> CoreResult<PluginView> {
    if !std::path::Path::new(&source).is_dir() {
        return Err(CoreError::Internal(format!("插件源目录不存在: {source}")));
    }
    let plugins = state.plugins.clone();
    join_core(tauri::async_runtime::spawn_blocking(move || {
        plugins.install(std::path::Path::new(&source))
    }))
    .await
}

#[tauri::command]
pub async fn plugins_rollback(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CoreResult<PluginView> {
    let plugins = state.plugins.clone();
    join_core(tauri::async_runtime::spawn_blocking(move || plugins.rollback(&id))).await
}

#[tauri::command]
pub async fn plugins_uninstall(
    state: tauri::State<'_, AppState>,
    id: String,
) -> CoreResult<()> {
    let plugins = state.plugins.clone();
    join_core(tauri::async_runtime::spawn_blocking(move || plugins.uninstall(&id))).await
}

#[tauri::command]
pub async fn plugins_set_enabled(
    state: tauri::State<'_, AppState>,
    id: String,
    enabled: bool,
) -> CoreResult<PluginView> {
    let plugins = state.plugins.clone();
    join_core(tauri::async_runtime::spawn_blocking(move || {
        plugins.set_enabled(&id, enabled)
    }))
    .await
}

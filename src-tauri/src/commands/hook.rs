//! Hook 命令层（P10 Frida 会话工作台）：薄层——参数校验与转发，
//! 业务规则全在 HookService。设计 docs/frida-console-design.md §6.1。
//! 停止会话/日志回放复用 task_cancel / task_logs，不新增命令；
//! 工作目录读写并入现有 config_get/config_set 白名单（app.hook.workdir）。

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::hook_service::{HookSessionStartArgs, JsFileDto, PreflightDto};

/// 扫工作目录一层 *.js（非递归、忽略符号链接、上限 500）。
/// dir 空 = 读配置键 app.hook.workdir。
#[tauri::command]
pub async fn hook_js_list(
    state: tauri::State<'_, AppState>,
    dir: Option<String>,
) -> CoreResult<Vec<JsFileDto>> {
    let dir = dir.unwrap_or_default();
    let dir = if dir.trim().is_empty() {
        state.hook.workdir()
    } else {
        dir
    };
    state.hook.list_js(&dir)
}

/// 前置检查链聚合：adb / python / frida /（远程）TCP 可达 / runner 资源。
/// remote 非空（host:port）时追加端口探活。
#[tauri::command]
pub async fn hook_preflight(
    state: tauri::State<'_, AppState>,
    remote: Option<String>,
) -> CoreResult<PreflightDto> {
    let remote = remote
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    Ok(state.hook.preflight(remote.as_deref()).await)
}

/// 组 CommandSpec 起 frida runner 任务，返回 taskId（kind="frida"）。
#[tauri::command]
pub async fn hook_session_start(
    state: tauri::State<'_, AppState>,
    args: HookSessionStartArgs,
) -> CoreResult<String> {
    state.hook.start_session(args).await
}

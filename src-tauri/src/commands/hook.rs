//! Hook 命令层（P10 Frida 会话工作台）：薄层——参数校验与转发，
//! 业务规则全在 HookService。设计 docs/frida-console-design.md §6.1。
//! 停止会话/日志回放复用 task_cancel / task_logs，不新增命令；
//! 工作目录读写并入现有 config_get/config_set 白名单（app.hook.workdir）。

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
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
    serial: Option<String>,
) -> CoreResult<PreflightDto> {
    let remote = remote
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    let serial = serial
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    Ok(state
        .hook
        .preflight(remote.as_deref(), serial.as_deref())
        .await)
}

/// 组 CommandSpec 起 frida runner 任务，返回 taskId（kind="frida"）。
#[tauri::command]
pub async fn hook_session_start(
    state: tauri::State<'_, AppState>,
    args: HookSessionStartArgs,
) -> CoreResult<String> {
    state.hook.start_session(args).await
}

/// 把用户主动导出的文本写到**他自己在保存对话框里选的本机路径**（第七十四轮：控制台「保存日志」）。
/// 只有用户点过一次「保存日志」并确认过路径才会走到这里，程序自己不会把脚本正文落到任何地方
/// （重构文档 §3.7 那条"日志不得写脚本正文"约束管的是应用日志，不是用户主动导出）。
/// 写盘是同步阻塞 IO，放 blocking 线程：单次上限 64 MiB，占着 tokio worker 是会看得见的卡。
#[tauri::command]
pub async fn hook_export_save(path: String, content: String) -> CoreResult<u64> {
    tokio::task::spawn_blocking(move || crate::services::hook_service::save_export(&path, &content))
        .await
        .map_err(|error| CoreError::Internal(format!("保存任务被中断：{error}")))?
}

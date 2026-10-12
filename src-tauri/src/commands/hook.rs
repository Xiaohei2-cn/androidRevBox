//! Hook 命令层（P10 Frida 会话工作台）：薄层——参数校验与转发，
//! 业务规则全在 HookService。
//! 工作目录读写并入现有 config_get/config_set 白名单（app.hook.workdir）。
//!
//! 起会话**没有命令**：结构化终端与它的 runner 已整体退役，起会话走真终端页里的
//! 官方 frida REPL（PTY，前端 `lib/fridaSession` 一处判环境拼参数）。

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::services::hook_service::{JsFileDto, PreflightDto};

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

/// 前置检查链聚合：adb / python / frida-tools /（远程）TCP 可达 / 握手。
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

/// 读工作目录里的一个脚本正文（IDE 打开脚本时用）。
/// 边界（名字白名单、一层目录内、拒绝符号链接）全在 HookService。
#[tauri::command]
pub async fn hook_script_read(
    state: tauri::State<'_, AppState>,
    name: String,
) -> CoreResult<String> {
    state.hook.read_script(&name)
}

/// 写工作目录里的一个脚本（IDE 保存；返回落盘字节数，写完回读自证）。
/// `create_only`=true 是「新建」语义：撞名拒绝，不静默覆盖已有脚本。
/// 读写是同步阻塞 IO，但上限 4 MiB 且目标是本机小文件，不值得为它开 blocking 线程；
/// 真正的大文件路径（导出）另有一条。
#[tauri::command]
pub async fn hook_script_write(
    state: tauri::State<'_, AppState>,
    name: String,
    content: String,
    create_only: Option<bool>,
) -> CoreResult<u64> {
    state
        .hook
        .write_script(&name, &content, create_only.unwrap_or(false))
}

/// 删除工作目录里的一个脚本（IDE 的「删除」；只删普通 .js，符号链接拒绝）。
#[tauri::command]
pub async fn hook_script_delete(state: tauri::State<'_, AppState>, name: String) -> CoreResult<()> {
    state.hook.delete_script(&name)
}

/// 把用户主动导出的文本写到**他自己在保存对话框里选的本机路径**（真终端页「保存日志」）。
/// 只有用户点过一次「保存日志」并确认过路径才会走到这里，程序自己不会把脚本正文落到任何地方
/// （重构文档 §3.7 那条"日志不得写脚本正文"约束管的是应用日志，不是用户主动导出）。
/// 写盘是同步阻塞 IO，放 blocking 线程：单次上限 64 MiB，占着 tokio worker 是会看得见的卡。
#[tauri::command]
pub async fn hook_export_save(path: String, content: String) -> CoreResult<u64> {
    tokio::task::spawn_blocking(move || crate::services::hook_service::save_export(&path, &content))
        .await
        .map_err(|error| CoreError::Internal(format!("保存任务被中断：{error}")))?
}

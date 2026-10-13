//! Hook 命令层（P10 Frida 会话工作台）：薄层——参数校验与转发，
//! 业务规则全在 HookService。
//! 工作目录读写并入现有 config_get/config_set 白名单（app.hook.workdir）。
//!
//! 起会话**没有命令**：结构化终端与它的 runner 已整体退役，起会话走真终端页里的
//! 官方 frida REPL（PTY，前端 `lib/fridaSession` 一处判环境拼参数）。

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::services::hook_service::{FridaTypeDirDto, JsFileDto, PreflightDto};

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

/// 读用户指定的**项目目录**里的 frida 类型库（IDE 智能提示的"提示内容"来源）。
///
/// 为什么不从"选中的 Python/Node 环境"读：**那里没有 TS 类型**。实测 frida 的 PyPI 包里
/// 只有 `_frida.abi3.so` 与 `.pyi`（Python 自身类型），`frida-tools` 零个 `.d.ts`；
/// 类型库只存在于 npm 生态。用户凭直觉给的是**项目根**（PyCharm 认的就是项目里
/// `node_modules/@types/frida-gum`），所以这里收目录、按 node 解析规则去找。
/// 只读不改，与脚本读写那套"仅限工作目录"的写边界互不相干。
#[tauri::command]
pub async fn hook_frida_types(
    state: tauri::State<'_, AppState>,
    dir: Option<String>,
) -> CoreResult<crate::services::hook_service::FridaTypesDto> {
    // 空 = 读配置键 app.hook.frida_types_dir（设置页填的）
    let dir = dir
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| {
            state
                .config
                .get(crate::services::config_service::KEY_FRIDA_TYPES_DIR, "")
                .unwrap_or_default()
                .trim()
                .to_string()
        });
    if dir.is_empty() {
        return Err(CoreError::InvalidInput(
            "还没选择 frida 类型库目录（设置 → 工具环境）".into(),
        ));
    }
    state.hook.read_frida_types(&dir)
}

/// 自动发现本机上装了 `@types/frida-gum` 的项目（IDE 智能提示的来源候选）。
///
/// 为什么要自动发现：让用户手填项目路径是**把内部结构推给他**——机器上常有多份
/// `frida-agent-example` 副本（实测这台机器两个），填哪个不是他能答对的问题。
/// 前端拿到列表后：只有一份就直接用，多份让他挑，一份没有才提示手填。
#[tauri::command]
pub async fn hook_frida_types_dirs() -> CoreResult<Vec<FridaTypeDirDto>> {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.trim().is_empty() {
        return Ok(Vec::new());
    }
    // 目录扫描是同步 IO，但只在 IDE 打开时跑一次、范围有限，不值得开 blocking 线程
    Ok(crate::services::hook_service::discover_frida_type_dirs(
        std::path::Path::new(&home),
    ))
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

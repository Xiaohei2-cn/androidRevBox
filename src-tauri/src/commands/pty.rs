//! 真终端命令层（FT0.2）：薄层——参数转形状、字节出口接上 tauri Channel。
//! 业务规则（校验、会话表、尺寸下限、收摊顺序）全在 `services::pty_service`。
//!
//! 字节为什么走 `ipc::Channel` 而不是全局事件：
//! - 一个会话一条通道，前端关掉 tab 时通道被 drop，`send()` 立刻失败——
//!   读线程因此能感知"没人收了"（R-04 的背压就靠这个，不靠我们自己数队列）；
//! - 载荷是 `InvokeResponseBody::Raw`（原始字节），不用 base64、不用 JSON 数组。
//!   逐字节 JSON 化是 R-05 里"打爆主线程"的那条路，这里从一开始就不走。

use std::sync::Arc;

use tauri::Emitter;
use tauri::ipc::{Channel, InvokeResponseBody};

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::pty_service::{self, PtyExit, PtyOpenArgs, PtyOpened, PtySink};

/// 起一个本机 PTY 会话；输出按块送回 `on_data`，退出走事件 `pty://exit`。
#[tauri::command]
pub async fn pty_open(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    args: PtyOpenArgs,
    on_data: Channel<InvokeResponseBody>,
) -> CoreResult<PtyOpened> {
    let sink = {
        Arc::new(move |bytes: &[u8]| -> bool {
            // 送失败 = 前端那条通道没了（tab 关了/窗口销毁），读线程据此收摊
            on_data
                .send(InvokeResponseBody::Raw(bytes.to_vec()))
                .is_ok()
        }) as Arc<dyn PtySink>
    };
    let on_exit = {
        let app = app.clone();
        Arc::new(move |exit: PtyExit| {
            if let Err(error) = app.emit("pty://exit", &exit) {
                tracing::warn!(error = %error, "终端退出事件没送出去（界面可能已关）");
            }
        }) as Arc<dyn Fn(PtyExit) + Send + Sync>
    };
    state.pty.open(args, sink, on_exit)
}

/// 把键盘字节送进会话（原样，不加工）。
#[tauri::command]
pub async fn pty_write(
    state: tauri::State<'_, AppState>,
    id: String,
    bytes: Vec<u8>,
) -> CoreResult<()> {
    state.pty.write(&id, &bytes)
}

/// 改尺寸：真 `TIOCSWINSZ` + `SIGWINCH`（C-05）。
#[tauri::command]
pub async fn pty_resize(
    state: tauri::State<'_, AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> CoreResult<()> {
    state.pty.resize(&id, cols, rows)
}

/// 关会话：杀子进程、收线程。留不得半个（R-08）。
#[tauri::command]
pub async fn pty_close(state: tauri::State<'_, AppState>, id: String) -> CoreResult<()> {
    state.pty.close(&id)
}

/// 当前活动会话数（分页上限要用，也让"我是不是开了 8 个"是一眼可见的事实）。
#[tauri::command]
pub async fn pty_count(state: tauri::State<'_, AppState>) -> CoreResult<usize> {
    Ok(state.pty.len())
}

/// 界面预填用的默认程序（`$SHELL` / `COMSPEC`）。
#[tauri::command]
pub async fn pty_default_program() -> CoreResult<String> {
    Ok(pty_service::default_program())
}

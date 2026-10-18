//! 工具市场命令层（P13）：只做参数透传。
//!
//! 为什么下载与推送放在服务层而不是这里：它们是"多步且有校验"的动作
//! （下载 → 摘要校验 → push → chmod → 回读复核），任何一步失败都要能说清在哪一步断的。
//! 命令层只保留 Tauri 的签名。

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::tool_market::{ToolEntry, ToolInstallReport};

/// 内置工具目录。**不读网络**：界面看到的就是代码里登记的那些条目。
#[tauri::command]
pub async fn market_tools(state: tauri::State<'_, AppState>) -> CoreResult<Vec<ToolEntry>> {
    Ok(state.tool_market.catalog().to_vec())
}

/// 设备主 ABI：前端用它判断"这个工具在这台机器上能不能跑"。
/// 返回 `None` = 读不到（未知），不是"不兼容"——两者的界面处置完全不同。
#[tauri::command]
pub async fn market_device_abi(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<Option<String>> {
    Ok(state.tool_market.device_abi(&serial).await)
}

/// 一键安装：下载（或命中本机缓存）→ 校验 → 推送到 `/data/local/tmp/` → `chmod +x` → 回读复核。
#[tauri::command]
pub async fn market_install_tool(
    state: tauri::State<'_, AppState>,
    serial: String,
    id: String,
) -> CoreResult<ToolInstallReport> {
    state.tool_market.install(&serial, &id).await
}

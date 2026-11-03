//! 工具市场命令层（P13）：只做参数透传。
//!
//! 为什么下载与推送放在服务层而不是这里：它们是"多步且有校验"的动作
//! （下载 → 摘要校验 → push → chmod → 回读复核），任何一步失败都要能说清在哪一步断的。
//! 命令层只保留 Tauri 的签名。

use crate::AppState;
use crate::core::error::CoreResult;
use crate::services::tool_market::{FridaVersionDto, MarketToolDto, ToolInstallReport};

/// 工具目录：固定登记的工具 + 动态版本的 frida-server。
/// 这里**不主动联网**——frida 的版本列表由界面按需拉（`market_frida_releases`），
/// 打开市场页不该因为 GitHub 慢就整页空着。
#[tauri::command]
pub async fn market_tools(state: tauri::State<'_, AppState>) -> CoreResult<Vec<MarketToolDto>> {
    Ok(state.tool_market.catalog())
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

/// 一键安装（固定版本工具）：下载（或命中本机缓存）→ 校验 → 推送到 `/data/local/tmp/`
/// → `chmod +x` → 回读复核。
#[tauri::command]
pub async fn market_install_tool(
    state: tauri::State<'_, AppState>,
    serial: String,
    id: String,
) -> CoreResult<ToolInstallReport> {
    state.tool_market.install(&serial, &id).await
}

/// frida-server 的版本列表（实时取自 frida 官方 tags，16.x 及以后，按版本从新到旧）。
/// `refresh = true` 跳过服务端 10 分钟缓存；GitHub 匿名限额 60 次/小时。
#[tauri::command]
pub async fn market_frida_releases(
    state: tauri::State<'_, AppState>,
    refresh: Option<bool>,
) -> CoreResult<Vec<FridaVersionDto>> {
    state
        .tool_market
        .frida_releases(refresh.unwrap_or(false))
        .await
}

/// 安装指定版本的 frida-server：解析资产 → 下载 + 摘要校验 → 本机解压（.xz）
/// → 推送 `/data/local/tmp/frida-server` → `chmod +x` → 回读复核。
#[tauri::command]
pub async fn market_install_frida(
    state: tauri::State<'_, AppState>,
    serial: String,
    version: String,
) -> CoreResult<ToolInstallReport> {
    state.tool_market.install_frida(&serial, &version).await
}

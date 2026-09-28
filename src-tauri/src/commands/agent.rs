//! Agent 会话诊断命令：只做参数转发与稳定错误码映射。

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::models::agent::{AgentDiagnostics, AgentProbe, AgentSessionStatus};
use crate::services::agent_client::AgentClientError;
use crate::services::agent_manager::AgentManagerError;
use agent_protocol::ErrorCode;

#[tauri::command]
pub fn agent_status(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<AgentSessionStatus> {
    Ok(state.agent.status(serial.trim()))
}

#[tauri::command]
pub fn agent_statuses(state: tauri::State<'_, AppState>) -> CoreResult<Vec<AgentSessionStatus>> {
    Ok(state.agent.statuses())
}

#[tauri::command]
pub async fn agent_install(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<AgentSessionStatus> {
    let serial = serial.trim();
    // 这一次点击本身就是授权（AR12.5 / D086）。它顺手把这台设备记进授权表，从此
    // 插线全自动——D028 反对的是"静默"装 Agent，不是反对装 Agent。
    //
    // 为什么不在这里弹二次确认：按钮旁边的文案已经写明会往 /data/local/tmp 写
    // 产物并起常驻进程；再叠一层确认是拿用户的点击次数换我的免责。
    //
    // 注意这条授权**不因为连接失败而撤销**：她点的是"我要这台能用"，失败多半是
    // 产物、网络或设备临时状态；下次插线继续自动重试才是她想要的。别把它"修"成
    // 失败就撤回——那会变成每次失败都要她再点一次。
    state.agent_auto.set_consent(serial, true)?;
    state
        .agent
        .connect_resolved(serial)
        .await
        .map_err(map_agent_error)
}

#[tauri::command]
pub async fn agent_restart(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<AgentSessionStatus> {
    let serial = serial.trim();
    // 同 agent_install：显式点击 = 授权。这一条还兼作「重启并接管」——界面上那个
    // 按钮点的就是它，所以 D063 那条"不擅自重启别人的 Agent"到这里才允许破：
    // 破的人是她，不是程序。
    state.agent_auto.set_consent(serial, true)?;
    state
        .agent
        .disconnect(serial)
        .await
        .map_err(map_agent_error)?;
    state
        .agent
        .connect_resolved(serial)
        .await
        .map_err(map_agent_error)
}

/// 只读探一次「这台设备的 Agent 现在缺哪一步」（AR12.5）。
///
/// 命令层不自己判：结论与动作都由 `AgentAutoService::decide` 出，界面读的是同一个
/// 结果，所以不会出现"网页觉得该弹按钮、后端觉得不该动"这种两套话。
/// 这条永远不写设备：push / chmod / kill / forward 一个都不会发生。
#[tauri::command]
pub async fn agent_probe(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<AgentProbe> {
    Ok(state.agent_auto.probe(serial.trim()).await)
}

#[tauri::command]
pub async fn agent_diagnostics(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<AgentDiagnostics> {
    let serial = serial.trim();
    let mut diagnostics = state.agent.diagnostics(serial).await;
    diagnostics.routes = state.android.routes_for_serial(serial);
    diagnostics.legacy_fallbacks = state.android.fallback_totals_for_serial(serial);
    // 缓存里那份探测结论顺带下发：界面每 10s 轮询诊断，不该每次都为了这一行去
    // 发四条 adb 命令（那是骚扰设备，不是诊断）。
    diagnostics.auto_probe = state.agent_auto.cached_probe(serial);
    Ok(diagnostics)
}

fn map_agent_error(error: AgentManagerError) -> CoreError {
    match error {
        AgentManagerError::StartupFailed { cause, rollback } => {
            let context = format!("{}; rollback: {rollback}", cause);
            match map_agent_error(*cause) {
                CoreError::AgentIncompatible(_) => CoreError::AgentIncompatible(context),
                CoreError::AgentTransportLost(_) => CoreError::AgentTransportLost(context),
                CoreError::AgentUnavailable(_) => CoreError::AgentUnavailable(context),
                _ => CoreError::Internal(context),
            }
        }
        AgentManagerError::IncompatibleProtocol { .. }
        | AgentManagerError::IncompatibleAgentVersion { .. } => {
            CoreError::AgentIncompatible(error.to_string())
        }
        AgentManagerError::Client(AgentClientError::Remote(remote))
            if remote.code == ErrorCode::IncompatibleVersion =>
        {
            CoreError::AgentIncompatible(format!("{}: {}", remote.code, remote.message))
        }
        AgentManagerError::Client(AgentClientError::TransportLost(_)) => {
            CoreError::AgentTransportLost(error.to_string())
        }
        AgentManagerError::Bootstrap(_)
        | AgentManagerError::Artifact(_)
        | AgentManagerError::Connect { .. }
        | AgentManagerError::InvalidSerial => CoreError::AgentUnavailable(error.to_string()),
        _ => CoreError::Internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use agent_protocol::{AgentError, ErrorCode};

    use super::*;

    #[test]
    fn preserves_incompatible_and_transport_codes_through_startup_rollback() {
        let incompatible = AgentManagerError::StartupFailed {
            cause: Box::new(AgentManagerError::Client(AgentClientError::Remote(
                AgentError::new(ErrorCode::IncompatibleVersion, "protocol mismatch"),
            ))),
            rollback: "restored".into(),
        };
        assert_eq!(map_agent_error(incompatible).code(), "AGENT_INCOMPATIBLE");

        let transport = AgentManagerError::StartupFailed {
            cause: Box::new(AgentManagerError::Client(AgentClientError::TransportLost(
                "closed".into(),
            ))),
            rollback: "not required".into(),
        };
        assert_eq!(map_agent_error(transport).code(), "AGENT_TRANSPORT_LOST");
    }
}

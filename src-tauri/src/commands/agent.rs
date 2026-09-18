//! Agent 会话诊断命令：只做参数转发与稳定错误码映射。

use crate::AppState;
use crate::core::error::{CoreError, CoreResult};
use crate::models::agent::{AgentDiagnostics, AgentSessionStatus};
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
    state
        .agent
        .connect_resolved(serial.trim())
        .await
        .map_err(map_agent_error)
}

#[tauri::command]
pub async fn agent_restart(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<AgentSessionStatus> {
    let serial = serial.trim();
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

#[tauri::command]
pub async fn agent_diagnostics(
    state: tauri::State<'_, AppState>,
    serial: String,
) -> CoreResult<AgentDiagnostics> {
    let serial = serial.trim();
    let mut diagnostics = state.agent.diagnostics(serial).await;
    diagnostics.routes = state.android.routes_for_serial(serial);
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

use agent_protocol::{CapabilityInfo, HealthResult, PermissionInfo, ProviderInfo};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionState {
    Disconnected,
    AdbOnline,
    Installing,
    Starting,
    Handshaking,
    Ready,
    Degraded,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionStatus {
    pub serial: String,
    pub state: AgentSessionState,
    pub agent_version: Option<String>,
    pub protocol_version: Option<u32>,
    pub permissions: Option<PermissionInfo>,
    pub providers: Vec<ProviderInfo>,
    pub capabilities: Vec<CapabilityInfo>,
    pub local_port: Option<u16>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDiagnostics {
    pub status: AgentSessionStatus,
    pub health: Option<HealthResult>,
    pub health_error: Option<String>,
    pub routes: Vec<AgentRouteDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AndroidBackendSource {
    Agent,
    LegacyAdb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRouteDiagnostics {
    pub serial: String,
    pub method: String,
    pub backend: AndroidBackendSource,
    pub fallback_reason: Option<String>,
    pub agent_version: Option<String>,
    pub protocol_version: Option<u32>,
    pub recorded_at: i64,
}

impl AgentSessionStatus {
    pub fn disconnected(serial: impl Into<String>) -> Self {
        Self {
            serial: serial.into(),
            state: AgentSessionState::Disconnected,
            agent_version: None,
            protocol_version: None,
            permissions: None,
            providers: Vec::new(),
            capabilities: Vec::new(),
            local_port: None,
            last_error: None,
        }
    }

    pub(crate) fn transition(&mut self, state: AgentSessionState) {
        self.state = state;
        self.last_error = None;
    }

    pub(crate) fn fail(&mut self, state: AgentSessionState, error: impl Into<String>) {
        self.state = state;
        self.local_port = None;
        self.last_error = Some(error.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontend_state_uses_stable_case_and_hides_no_error_context() {
        let mut status = AgentSessionStatus::disconnected("serial-1");
        status.fail(AgentSessionState::Incompatible, "protocol mismatch");
        let value = serde_json::to_value(status).unwrap();
        assert_eq!(value["state"], "incompatible");
        assert_eq!(value["serial"], "serial-1");
        assert_eq!(value["lastError"], "protocol mismatch");
    }
}

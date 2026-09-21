use serde::{Deserialize, Serialize};

use crate::PROTOCOL_VERSION;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloParams {
    pub auth_token: String,
    pub desktop_version: String,
    pub supported_protocol_versions: Vec<u32>,
}

impl HelloParams {
    pub fn v1(auth_token: impl Into<String>, desktop_version: impl Into<String>) -> Self {
        Self {
            auth_token: auth_token.into(),
            desktop_version: desktop_version.into(),
            supported_protocol_versions: vec![PROTOCOL_VERSION],
        }
    }
}

pub fn negotiate_protocol(peer_supported: &[u32]) -> Option<u32> {
    peer_supported
        .contains(&PROTOCOL_VERSION)
        .then_some(PROTOCOL_VERSION)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    Ready,
    Degraded,
    Unavailable,
    Incompatible,
    Faulted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionInfo {
    pub shell: bool,
    pub root: bool,
    pub selinux_enforcing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub name: String,
    pub version: String,
    pub health: ProviderHealth,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityInfo {
    pub method: String,
    pub version: u32,
    pub provider: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// 「还没探测完」而不是「探测过、不可用」。
    ///
    /// 为什么需要单独一个布尔：Agent 现在先监听再后台预热（AR4.2 修启动顺序），
    /// 所以第一次 hello 可能赶上探测还没结束。没有这个字段，桌面端会把"未知"
    /// 当成"不可用"，在本地就把一个其实能服务的请求拦掉（今天真实踩到：status 已
    /// 经很快，list 却被拒 "zygisk bridge 尚未探测完成"）。有它才能把判断交还给 Agent。
    #[serde(default)]
    pub probe_pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol_version: u32,
    pub agent_version: String,
    pub permissions: PermissionInfo,
    pub providers: Vec<ProviderInfo>,
    pub capabilities: Vec<CapabilityInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ready,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResult {
    pub status: HealthStatus,
    pub agent_version: String,
    pub protocol_version: u32,
    pub uptime_ms: u64,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn hello_defaults_to_v1_and_negotiates_explicitly() {
        let hello = HelloParams::v1("secret", "0.1.0");
        assert_eq!(hello.supported_protocol_versions, [PROTOCOL_VERSION]);
        assert_eq!(negotiate_protocol(&[1]), Some(1));
        assert_eq!(negotiate_protocol(&[3, 2, 1]), Some(1));
        assert_eq!(negotiate_protocol(&[3, 2]), None);
        assert_eq!(negotiate_protocol(&[]), None);
    }

    #[test]
    fn provider_and_capability_json_are_stable() {
        let result = HelloResult {
            protocol_version: 1,
            agent_version: "0.1.0".into(),
            permissions: PermissionInfo {
                shell: true,
                root: false,
                selinux_enforcing: true,
            },
            providers: vec![ProviderInfo {
                name: "shell".into(),
                version: "1.0.0".into(),
                health: ProviderHealth::Degraded,
                required_permissions: vec!["shell".into()],
                last_error: None,
            }],
            capabilities: vec![CapabilityInfo {
                method: "device.info".into(),
                version: 1,
                provider: "shell".into(),
                available: true,
                unavailable_reason: None,
                probe_pending: false,
            }],
        };
        let value = serde_json::to_value(result).unwrap();
        assert_eq!(value["providers"][0]["health"], "degraded");
        assert_eq!(value["capabilities"][0]["method"], "device.info");
        assert_eq!(
            value["permissions"],
            json!({
                "shell": true,
                "root": false,
                "selinux_enforcing": true
            })
        );
    }
}

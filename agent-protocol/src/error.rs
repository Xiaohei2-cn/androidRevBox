use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Stable machine-readable error code. Unknown future v1 codes remain readable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    InvalidRequest,
    UnsupportedMethod,
    PermissionDenied,
    NotFound,
    Busy,
    DeadlineExceeded,
    Cancelled,
    TransportLost,
    IncompatibleVersion,
    ProviderUnavailable,
    Internal,
    Unknown(String),
}

impl ErrorCode {
    pub fn as_str(&self) -> &str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnsupportedMethod => "unsupported_method",
            Self::PermissionDenied => "permission_denied",
            Self::NotFound => "not_found",
            Self::Busy => "busy",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Cancelled => "cancelled",
            Self::TransportLost => "transport_lost",
            Self::IncompatibleVersion => "incompatible_version",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::Internal => "internal",
            Self::Unknown(code) => code,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let code = String::deserialize(deserializer)?;
        Ok(match code.as_str() {
            "invalid_request" => Self::InvalidRequest,
            "unsupported_method" => Self::UnsupportedMethod,
            "permission_denied" => Self::PermissionDenied,
            "not_found" => Self::NotFound,
            "busy" => Self::Busy,
            "deadline_exceeded" => Self::DeadlineExceeded,
            "cancelled" => Self::Cancelled,
            "transport_lost" => Self::TransportLost,
            "incompatible_version" => Self::IncompatibleVersion,
            "provider_unavailable" => Self::ProviderUnavailable,
            "internal" => Self::Internal,
            _ => Self::Unknown(code),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl AgentError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn stable_codes_use_snake_case_json() {
        let cases = [
            (ErrorCode::InvalidRequest, "invalid_request"),
            (ErrorCode::UnsupportedMethod, "unsupported_method"),
            (ErrorCode::PermissionDenied, "permission_denied"),
            (ErrorCode::DeadlineExceeded, "deadline_exceeded"),
            (ErrorCode::TransportLost, "transport_lost"),
            (ErrorCode::IncompatibleVersion, "incompatible_version"),
            (ErrorCode::ProviderUnavailable, "provider_unavailable"),
        ];
        for (code, expected) in cases {
            assert_eq!(serde_json::to_value(code).unwrap(), expected);
        }
    }

    #[test]
    fn unknown_code_and_additional_fields_are_forward_compatible() {
        let error: AgentError = serde_json::from_value(json!({
            "code": "future_provider_error",
            "message": "new agent response",
            "future_field": true
        }))
        .unwrap();
        assert_eq!(
            error.code,
            ErrorCode::Unknown("future_provider_error".into())
        );
        assert_eq!(error.code.to_string(), "future_provider_error");
    }
}

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::AgentError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub request_id: String,
    pub method: String,
    pub params: Value,
    pub timeout_ms: u64,
    pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResponseEnvelope {
    pub request_id: String,
    #[serde(flatten)]
    pub payload: ResponsePayload,
}

impl ResponseEnvelope {
    pub fn success(request_id: impl Into<String>, result: Value) -> Self {
        Self {
            request_id: request_id.into(),
            payload: ResponsePayload::Success { result },
        }
    }

    pub fn failure(request_id: impl Into<String>, error: AgentError) -> Self {
        Self {
            request_id: request_id.into(),
            payload: ResponsePayload::Failure { error },
        }
    }

    pub fn is_response_to(&self, request: &RequestEnvelope) -> bool {
        self.request_id == request.request_id
    }
}

impl<'de> Deserialize<'de> for ResponseEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut object = Map::<String, Value>::deserialize(deserializer)?;
        let request_id = object
            .remove("request_id")
            .ok_or_else(|| D::Error::missing_field("request_id"))
            .and_then(|value| serde_json::from_value(value).map_err(D::Error::custom))?;
        let result = object.remove("result");
        let error = object.remove("error");
        let payload = match (result, error) {
            (Some(result), None) => ResponsePayload::Success { result },
            (None, Some(error)) => ResponsePayload::Failure {
                error: serde_json::from_value(error).map_err(D::Error::custom)?,
            },
            (Some(_), Some(_)) => {
                return Err(D::Error::custom(
                    "response must not contain both result and error",
                ));
            }
            (None, None) => {
                return Err(D::Error::custom(
                    "response must contain exactly one of result or error",
                ));
            }
        };
        Ok(Self {
            request_id,
            payload,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ResponsePayload {
    Success { result: Value },
    Failure { error: AgentError },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub subscription_id: String,
    pub seq: u64,
    pub event: String,
    pub payload: Value,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{ErrorCode, PROTOCOL_VERSION};

    fn request() -> RequestEnvelope {
        RequestEnvelope {
            request_id: "req-1".into(),
            method: "system.health".into(),
            params: json!({}),
            timeout_ms: 2_000,
            protocol_version: PROTOCOL_VERSION,
        }
    }

    #[test]
    fn request_json_shape_and_unknown_fields_are_compatible() {
        let value = serde_json::to_value(request()).unwrap();
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["timeout_ms"], 2_000);
        assert_eq!(value["protocol_version"], 1);

        let decoded: RequestEnvelope = serde_json::from_value(json!({
            "request_id": "req-2",
            "method": "system.health",
            "params": {},
            "timeout_ms": 500,
            "protocol_version": 1,
            "future_field": { "accepted": true }
        }))
        .unwrap();
        assert_eq!(decoded.request_id, "req-2");
    }

    #[test]
    fn response_requires_exactly_one_payload_and_accepts_null_result() {
        let success: ResponseEnvelope = serde_json::from_value(json!({
            "request_id": "req-1",
            "result": null,
            "future_field": 1
        }))
        .unwrap();
        assert!(matches!(
            success.payload,
            ResponsePayload::Success {
                result: Value::Null
            }
        ));

        let failure: ResponseEnvelope = serde_json::from_value(json!({
            "request_id": "req-1",
            "error": { "code": "busy", "message": "try later" }
        }))
        .unwrap();
        assert!(matches!(
            failure.payload,
            ResponsePayload::Failure {
                error: AgentError {
                    code: ErrorCode::Busy,
                    ..
                }
            }
        ));

        assert!(
            serde_json::from_value::<ResponseEnvelope>(json!({
                "request_id": "req-1",
                "result": {},
                "error": { "code": "internal", "message": "bad" }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<ResponseEnvelope>(json!({ "request_id": "req-1" })).is_err()
        );
    }

    #[test]
    fn response_id_and_event_sequence_are_preserved() {
        let request = request();
        let response = ResponseEnvelope::success("req-1", json!({ "ok": true }));
        assert!(response.is_response_to(&request));
        assert!(!ResponseEnvelope::success("req-2", Value::Null).is_response_to(&request));

        let first = EventEnvelope {
            subscription_id: "sub-1".into(),
            seq: 41,
            event: "task.output".into(),
            payload: json!({ "line": "one" }),
        };
        let second = EventEnvelope {
            seq: 42,
            payload: json!({ "line": "two" }),
            ..first.clone()
        };
        let decoded: EventEnvelope =
            serde_json::from_value(serde_json::to_value(&second).unwrap()).unwrap();
        assert_eq!(decoded.seq, first.seq + 1);
        assert_eq!(decoded.subscription_id, "sub-1");
    }
}

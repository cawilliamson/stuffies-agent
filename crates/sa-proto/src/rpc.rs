//! json-rpc 2.0 wire types for the `sa` protocol.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::error::RpcError;

/// protocol version carried by every `hello`. bumped on any breaking change.
pub const PROTOCOL_VERSION: u32 = 1;

/// version reported by `ping` and `hello`.
pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

/// the mandatory `"jsonrpc": "2.0"` member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonRpc;

impl Serialize for JsonRpc {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("2.0")
    }
}

impl<'de> Deserialize<'de> for JsonRpc {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw == "2.0" {
            Ok(JsonRpc)
        } else {
            Err(de::Error::custom(format!(
                "expected jsonrpc \"2.0\", got {raw:?}"
            )))
        }
    }
}

/// a request identifier. absent on notifications.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: JsonRpc,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    #[must_use]
    pub fn new(id: Id, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JsonRpc,
            id: Some(id),
            method: method.into(),
            params,
        }
    }

    #[must_use]
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: JsonRpc,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    #[must_use]
    pub fn ok(id: Option<Id>, result: Value) -> Self {
        Self {
            jsonrpc: JsonRpc,
            id,
            result: Some(result),
            error: None,
        }
    }

    #[must_use]
    pub fn err(id: Option<Id>, error: RpcError) -> Self {
        Self {
            jsonrpc: JsonRpc,
            id,
            result: None,
            error: Some(error),
        }
    }

    /// the successful result, or the carried error.
    pub fn into_result(self) -> Result<Value, RpcError> {
        match (self.result, self.error) {
            (_, Some(error)) => Err(error),
            (Some(result), None) => Ok(result),
            (None, None) => Err(RpcError::new(
                crate::error::code::INTERNAL_ERROR,
                "response carried neither result nor error",
            )),
        }
    }
}

/// parameters of the mandatory first message on every connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: u32,
    pub client: String,
    pub client_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub version: String,
}

/// `ping` result. `uptime_seconds` is monotonic daemon uptime, not wall clock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingResult {
    pub version: String,
    pub uptime_seconds: u64,
    pub sessions: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trips_a_request() {
        let req = Request::new(Id::Number(7), "ping", Some(json!({"nested": [1, 2, 3]})));
        let line = serde_json::to_string(&req).expect("serialise");
        let back: Request = serde_json::from_str(&line).expect("deserialise");
        assert_eq!(back.method, "ping");
        assert_eq!(back.id, Some(Id::Number(7)));
        assert!(!back.is_notification());
    }

    #[test]
    fn a_request_without_an_id_is_a_notification() {
        let req: Request = serde_json::from_str(r#"{"jsonrpc":"2.0","method":"events.subscribe"}"#)
            .expect("deserialise");
        assert!(req.is_notification());
    }

    #[test]
    fn rejects_a_wrong_jsonrpc_member() {
        let err = serde_json::from_str::<Request>(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#)
            .expect_err("must reject");
        assert!(err.to_string().contains("2.0"), "{err}");
    }

    #[test]
    fn rejects_a_missing_jsonrpc_member() {
        assert!(serde_json::from_str::<Request>(r#"{"id":1,"method":"ping"}"#).is_err());
    }

    #[test]
    fn an_error_response_yields_the_error() {
        let resp = Response::err(
            Some(Id::String("a".into())),
            RpcError::new(crate::error::code::VERSION_MISMATCH, "nope"),
        );
        let line = serde_json::to_string(&resp).expect("serialise");
        assert!(!line.contains("result"), "result must be omitted: {line}");
        let back: Response = serde_json::from_str(&line).expect("deserialise");
        let err = back.into_result().expect_err("must be an error");
        assert_eq!(err.code, crate::error::code::VERSION_MISMATCH);
    }

    #[test]
    fn an_ok_response_yields_the_result() {
        let resp = Response::ok(Some(Id::Number(1)), json!({"ok": true}));
        let back: Response =
            serde_json::from_str(&serde_json::to_string(&resp).expect("serialise"))
                .expect("deserialise");
        assert_eq!(back.into_result().expect("result"), json!({"ok": true}));
    }
}

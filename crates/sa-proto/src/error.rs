//! json-rpc error codes and the wire error type.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// json-rpc 2.0 reserved codes, plus the application range from -32000.
pub mod code {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;

    pub const VERSION_MISMATCH: i32 = -32000;
    pub const PEER_UID_MISMATCH: i32 = -32001;
    pub const LINE_TOO_LONG: i32 = -32002;
    pub const HANDSHAKE_REQUIRED: i32 = -32003;
    pub const SESSION_BUSY: i32 = -32004;
    pub const SESSION_NOT_FOUND: i32 = -32005;
    pub const STALE_OWNER: i32 = -32006;
    pub const WORKSPACE_DIVERGED: i32 = -32007;
    pub const WORKSPACE_UNREACHABLE: i32 = -32008;
    pub const CONTEXT_EXHAUSTED: i32 = -32009;
    pub const BUNDLE_INVALID: i32 = -32011;
    pub const KNOWLEDGE_DENIED: i32 = -32012;
    pub const KNOWLEDGE_UNAVAILABLE: i32 = -32013;
    pub const NOT_IMPLEMENTED: i32 = -32014;
}

/// an error carried in a json-rpc `error` member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    #[must_use]
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    #[must_use]
    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            code::METHOD_NOT_FOUND,
            format!("method not found: {method}"),
        )
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn method_not_found_carries_the_reserved_code() {
        let err = RpcError::method_not_found("nope");
        assert_eq!(err.code, code::METHOD_NOT_FOUND);
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("nope"));
    }

    #[test]
    fn error_round_trips_without_a_data_member_when_absent() {
        let err = RpcError::new(code::HANDSHAKE_REQUIRED, "hello first");
        let json = serde_json::to_string(&err).expect("serialise");
        assert!(!json.contains("data"), "data must be omitted: {json}");
        let back: RpcError = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, err);
    }

    #[test]
    fn application_codes_do_not_collide() {
        let codes = [
            code::PARSE_ERROR,
            code::INVALID_REQUEST,
            code::METHOD_NOT_FOUND,
            code::INVALID_PARAMS,
            code::INTERNAL_ERROR,
            code::VERSION_MISMATCH,
            code::PEER_UID_MISMATCH,
            code::LINE_TOO_LONG,
            code::HANDSHAKE_REQUIRED,
            code::SESSION_BUSY,
            code::SESSION_NOT_FOUND,
            code::STALE_OWNER,
            code::WORKSPACE_DIVERGED,
            code::WORKSPACE_UNREACHABLE,
            code::CONTEXT_EXHAUSTED,
            code::BUNDLE_INVALID,
            code::KNOWLEDGE_DENIED,
            code::KNOWLEDGE_UNAVAILABLE,
            code::NOT_IMPLEMENTED,
        ];
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), codes.len(), "duplicate error code");
        for c in codes {
            assert!(
                (-32099..=-32000).contains(&c) || c <= -32600,
                "{c} out of range"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod display_tests {
    use super::*;

    #[test]
    fn display_includes_the_code_and_message() {
        let err = RpcError::new(code::HANDSHAKE_REQUIRED, "hello first");
        let text = err.to_string();
        assert!(text.contains("-32003"), "{text}");
        assert!(text.contains("hello first"), "{text}");
    }
}

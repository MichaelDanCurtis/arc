//! JSON-RPC 2.0 error types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard JSON-RPC 2.0 error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum RpcErrorCode {
    ParseError     = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams  = -32602,
    InternalError  = -32603,
    /// Application-defined errors: -32000 to -32099.
    ServerError    = -32000,
}

impl RpcErrorCode {
    pub fn as_i32(self) -> i32 { self as i32 }

    pub fn from_i32(code: i32) -> Option<Self> {
        match code {
            -32700 => Some(Self::ParseError),
            -32600 => Some(Self::InvalidRequest),
            -32601 => Some(Self::MethodNotFound),
            -32602 => Some(Self::InvalidParams),
            -32603 => Some(Self::InternalError),
            -32099..=-32000 => Some(Self::ServerError),
            _ => None,
        }
    }
}

/// JSON-RPC 2.0 error object (appears in the `error` field of a [`Response`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("JSON-RPC error {code}: {message}")]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn with_data(mut self, data: impl Into<Value>) -> Self {
        self.data = Some(data.into());
        self
    }

    pub fn parse_error(msg: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::ParseError as i32, msg)
    }

    pub fn invalid_request(msg: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::InvalidRequest as i32, msg)
    }

    pub fn method_not_found(method: impl std::fmt::Display) -> Self {
        Self::new(RpcErrorCode::MethodNotFound as i32, format!("Method not found: {method}"))
    }

    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::InvalidParams as i32, msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::InternalError as i32, msg)
    }

    pub fn server_error(msg: impl Into<String>) -> Self {
        Self::new(RpcErrorCode::ServerError as i32, msg)
    }

    pub fn error_code(&self) -> Option<RpcErrorCode> {
        RpcErrorCode::from_i32(self.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_codes_correct() {
        assert_eq!(RpcErrorCode::ParseError as i32, -32700);
        assert_eq!(RpcErrorCode::MethodNotFound as i32, -32601);
        assert_eq!(RpcErrorCode::InvalidParams as i32, -32602);
    }

    #[test]
    fn rpc_error_roundtrip() {
        let e = RpcError::invalid_params("missing field 'command'")
            .with_data(json!({"field": "command"}));
        let s = serde_json::to_string(&e).unwrap();
        let d: RpcError = serde_json::from_str(&s).unwrap();
        assert_eq!(d.code, -32602);
        assert_eq!(d.data.unwrap()["field"], "command");
    }

    #[test]
    fn method_not_found_message() {
        let e = RpcError::method_not_found("unknown.op");
        assert!(e.message.contains("unknown.op"));
        assert_eq!(e.error_code(), Some(RpcErrorCode::MethodNotFound));
    }
}

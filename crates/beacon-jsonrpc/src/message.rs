//! JSON-RPC 2.0 message types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A JSON-RPC 2.0 request id — string, number, or null.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Str(String),
    Num(i64),
    Null,
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Id::Str(s) => write!(f, "{s}"),
            Id::Num(n) => write!(f, "{n}"),
            Id::Null => write!(f, "null"),
        }
    }
}

/// JSON-RPC 2.0 request (expects a response).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    pub id: Id,
}

impl Request {
    pub fn new(method: impl Into<String>, params: impl Into<Option<Value>>, id: impl Into<Id>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params: params.into(),
            id: id.into(),
        }
    }

    /// Validate that `jsonrpc == "2.0"`.
    pub fn validate(&self) -> Result<(), crate::RpcError> {
        if self.jsonrpc != "2.0" {
            return Err(crate::RpcError::invalid_request("jsonrpc field must be \"2.0\""));
        }
        Ok(())
    }
}

/// JSON-RPC 2.0 notification (no response expected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: impl Into<Option<Value>>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params: params.into(),
        }
    }
}

/// JSON-RPC 2.0 response (success or error).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Id,
    #[serde(flatten)]
    pub outcome: ResponseOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseOutcome {
    Result(Value),
    Error(crate::RpcError),
}

impl Response {
    pub fn success(id: impl Into<Id>, result: impl Into<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            outcome: ResponseOutcome::Result(result.into()),
        }
    }

    pub fn error(id: impl Into<Id>, err: crate::RpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            outcome: ResponseOutcome::Error(err),
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self.outcome, ResponseOutcome::Result(_))
    }

    pub fn result(&self) -> Option<&Value> {
        match &self.outcome {
            ResponseOutcome::Result(v) => Some(v),
            _ => None,
        }
    }

    pub fn err(&self) -> Option<&crate::RpcError> {
        match &self.outcome {
            ResponseOutcome::Error(e) => Some(e),
            _ => None,
        }
    }
}

// Allow i64 literals as Id
impl From<i64> for Id {
    fn from(n: i64) -> Self { Id::Num(n) }
}
impl From<i32> for Id {
    fn from(n: i32) -> Self { Id::Num(n as i64) }
}
impl From<u32> for Id {
    fn from(n: u32) -> Self { Id::Num(n as i64) }
}
impl From<String> for Id {
    fn from(s: String) -> Self { Id::Str(s) }
}
impl From<&str> for Id {
    fn from(s: &str) -> Self { Id::Str(s.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_roundtrip() {
        let req = Request::new("command.describe", json!({"command": "git"}), 1i64);
        let json = serde_json::to_string(&req).unwrap();
        let decoded: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);
        assert_eq!(decoded.method, "command.describe");
    }

    #[test]
    fn request_validate_ok() {
        let req = Request::new("foo", None, 1i64);
        assert!(req.validate().is_ok());
    }

    #[test]
    fn request_validate_bad_version() {
        let req = Request { jsonrpc: "1.0".to_string(), method: "foo".to_string(), params: None, id: Id::Num(1) };
        assert!(req.validate().is_err());
    }

    #[test]
    fn notification_roundtrip() {
        let n = Notification::new("session.started", json!({"session_id": "abc"}));
        let s = serde_json::to_string(&n).unwrap();
        let d: Notification = serde_json::from_str(&s).unwrap();
        assert_eq!(d.method, "session.started");
    }

    #[test]
    fn response_success_roundtrip() {
        let r = Response::success(1i64, json!({"description": "flags: --version"}));
        assert!(r.is_success());
        let s = serde_json::to_string(&r).unwrap();
        let d: Response = serde_json::from_str(&s).unwrap();
        assert!(d.is_success());
        assert_eq!(d.result().unwrap()["description"], "flags: --version");
    }

    #[test]
    fn response_error_roundtrip() {
        let err = crate::RpcError::method_not_found("unknown.method");
        let r = Response::error(1i64, err);
        assert!(!r.is_success());
        let s = serde_json::to_string(&r).unwrap();
        let d: Response = serde_json::from_str(&s).unwrap();
        assert!(!d.is_success());
        assert_eq!(d.err().unwrap().code, crate::RpcErrorCode::MethodNotFound as i32);
    }

    #[test]
    fn id_variants() {
        let s: Id = "abc".into();
        let n: Id = 42i64.into();
        assert_eq!(format!("{s}"), "abc");
        assert_eq!(format!("{n}"), "42");
    }
}

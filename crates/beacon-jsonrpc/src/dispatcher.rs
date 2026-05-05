//! Method dispatcher: registers [`RpcHandler`] implementations and routes
//! incoming [`Request`]s to the correct handler.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use crate::{Id, Request, Response, RpcError};

/// Boxed future returned by [`RpcHandler::handle`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A handler for a single JSON-RPC method.
///
/// Implement this trait and register it with [`RpcDispatcher::register`].
///
/// ```rust,ignore
/// struct PingHandler;
/// impl RpcHandler for PingHandler {
///     fn method_name(&self) -> &'static str { "ping" }
///     fn handle<'a>(&'a self, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
///         Box::pin(async { Ok(serde_json::json!("pong")) })
///     }
/// }
/// ```
pub trait RpcHandler: Send + Sync {
    /// The JSON-RPC method name this handler serves (e.g. `"command.describe"`).
    fn method_name(&self) -> &'static str;

    /// Handle the request. `params` is `Value::Null` when the request had no params field.
    fn handle(&self, params: Value) -> BoxFuture<'_, Result<Value, RpcError>>;
}

/// Central dispatcher: holds a registry of [`RpcHandler`]s and routes requests.
///
/// The dispatcher is constructed unconditionally; gating via [`beacon_features::FeatureFlag::JsonRpcSpine`]
/// is the caller's responsibility (i.e. call [`RpcDispatcher::dispatch`] only when the flag is enabled).
#[derive(Default)]
pub struct RpcDispatcher {
    handlers: HashMap<String, Arc<dyn RpcHandler>>,
}

impl RpcDispatcher {
    /// Create an empty dispatcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a handler. Panics if a handler for the same method is already registered.
    pub fn register<H: RpcHandler + 'static>(&mut self, handler: H) {
        let name = handler.method_name().to_string();
        assert!(
            !self.handlers.contains_key(&name),
            "RpcDispatcher: duplicate handler for method '{name}'"
        );
        self.handlers.insert(name, Arc::new(handler));
    }

    /// Register a handler; returns `false` (without panicking) if already registered.
    pub fn try_register<H: RpcHandler + 'static>(&mut self, handler: H) -> bool {
        let name = handler.method_name().to_string();
        if self.handlers.contains_key(&name) {
            return false;
        }
        self.handlers.insert(name, Arc::new(handler));
        true
    }

    /// Dispatch a [`Request`] to the registered handler.
    ///
    /// Returns a JSON-RPC [`Response`] with either the handler's result or a standard
    /// `-32601 Method Not Found` error.
    pub async fn dispatch(&self, req: Request) -> Response {
        // Validate version
        if let Err(e) = req.validate() {
            return Response::error(req.id, e);
        }

        let Some(handler) = self.handlers.get(&req.method) else {
            return Response::error(req.id.clone(), RpcError::method_not_found(&req.method));
        };

        let params = req.params.unwrap_or(Value::Null);
        match handler.handle(params).await {
            Ok(result) => Response::success(req.id, result),
            Err(e) => Response::error(req.id, e),
        }
    }

    /// Dispatch from a raw JSON string. Returns a JSON string response.
    /// Useful for transport layers that work with raw bytes.
    pub async fn dispatch_raw(&self, json: &str) -> String {
        let req: Request = match serde_json::from_str(json) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response::error(
                    Id::Null,
                    RpcError::parse_error(format!("Failed to parse request: {e}")),
                );
                return serde_json::to_string(&resp).unwrap_or_default();
            }
        };
        let resp = self.dispatch(req).await;
        serde_json::to_string(&resp).unwrap_or_default()
    }

    /// Number of registered handlers.
    pub fn handler_count(&self) -> usize {
        self.handlers.len()
    }

    /// List registered method names (sorted).
    pub fn registered_methods(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.handlers.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RpcErrorCode;
    use serde_json::json;

    struct EchoHandler;
    impl RpcHandler for EchoHandler {
        fn method_name(&self) -> &'static str { "echo" }
        fn handle<'a>(&'a self, params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
            Box::pin(async move { Ok(params) })
        }
    }

    struct FailHandler;
    impl RpcHandler for FailHandler {
        fn method_name(&self) -> &'static str { "fail" }
        fn handle<'a>(&'a self, _params: Value) -> BoxFuture<'a, Result<Value, RpcError>> {
            Box::pin(async { Err(RpcError::internal("always fails")) })
        }
    }

    fn dispatcher() -> RpcDispatcher {
        let mut d = RpcDispatcher::new();
        d.register(EchoHandler);
        d.register(FailHandler);
        d
    }

    #[tokio::test]
    async fn dispatch_happy_path() {
        let d = dispatcher();
        let req = Request::new("echo", json!({"key": "val"}), 1i64);
        let resp = d.dispatch(req).await;
        assert!(resp.is_success());
        assert_eq!(resp.result().unwrap()["key"], "val");
    }

    #[tokio::test]
    async fn dispatch_unknown_method() {
        let d = dispatcher();
        let req = Request::new("unknown.method", None, 2i64);
        let resp = d.dispatch(req).await;
        assert!(!resp.is_success());
        let e = resp.err().unwrap();
        assert_eq!(e.code, RpcErrorCode::MethodNotFound as i32);
    }

    #[tokio::test]
    async fn dispatch_handler_error() {
        let d = dispatcher();
        let req = Request::new("fail", None, 3i64);
        let resp = d.dispatch(req).await;
        assert!(!resp.is_success());
        assert_eq!(resp.err().unwrap().code, RpcErrorCode::InternalError as i32);
    }

    #[tokio::test]
    async fn dispatch_raw_roundtrip() {
        let d = dispatcher();
        let raw = r#"{"jsonrpc":"2.0","method":"echo","params":{"x":1},"id":4}"#;
        let out = d.dispatch_raw(raw).await;
        let resp: Response = serde_json::from_str(&out).unwrap();
        assert!(resp.is_success());
        assert_eq!(resp.result().unwrap()["x"], 1);
    }

    #[tokio::test]
    async fn dispatch_raw_parse_error() {
        let d = dispatcher();
        let out = d.dispatch_raw("not json at all {{{").await;
        let resp: Response = serde_json::from_str(&out).unwrap();
        assert!(!resp.is_success());
        assert_eq!(resp.err().unwrap().code, RpcErrorCode::ParseError as i32);
    }

    #[tokio::test]
    async fn dispatch_invalid_version() {
        let d = dispatcher();
        let raw = r#"{"jsonrpc":"1.0","method":"echo","id":5}"#;
        let out = d.dispatch_raw(raw).await;
        let resp: Response = serde_json::from_str(&out).unwrap();
        assert!(!resp.is_success());
        assert_eq!(resp.err().unwrap().code, RpcErrorCode::InvalidRequest as i32);
    }

    #[test]
    fn registered_methods_sorted() {
        let d = dispatcher();
        let methods = d.registered_methods();
        assert_eq!(methods, vec!["echo", "fail"]);
    }

    #[test]
    fn try_register_duplicate_returns_false() {
        let mut d = RpcDispatcher::new();
        assert!(d.try_register(EchoHandler));
        assert!(!d.try_register(EchoHandler));
        assert_eq!(d.handler_count(), 1);
    }
}

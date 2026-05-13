//! GraphQL-compat surface for the Beam client (`beam.Ailuxis.com/graphql/v2`).
//!
//! Beam OSS still issues a few GraphQL queries against the hosted
//! backend even in local-only mode — most notably `FreeAvailableModels`,
//! which the client uses to populate the pre-login model picker.
//! Returning DNS failure for those queries spams the client log on
//! every launch (see WarpMod ROADMAP C7.2).
//!
//! This module gives the Beam client a real endpoint to hit. The
//! handler dispatches on the `op` query parameter, which the Beam
//! GraphQL client embeds in the URL for telemetry. Today we serve
//! just the queries the local-mode client actually issues; new ops
//! return a structured error pointing the user at the proper Arc
//! API path for that capability.
//!
//! Wire shape (mirrors what Beam expects from the legacy
//! `app.warp.dev/graphql/v2` endpoint):
//!
//! - Method: `POST`
//! - Query string: `?op=<OperationName>` (telemetry-only, but used
//!   here for cheap op routing without parsing the GraphQL body)
//! - Body: JSON `{ "query": "...", "variables": {...} }` (currently
//!   ignored — the op name in the query string is sufficient to
//!   route the stub responses)
//! - Response: JSON `{ "data": ... }` or `{ "errors": [...] }`
//!
//! Once the user wants Beam-Pro / Beacon to actually drive model
//! selection, this handler can grow real GraphQL parsing + dispatch
//! against `state.arc_ai_backend`.

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use super::AppState;

/// Query-string params on every GraphQL request. The `op` parameter
/// is what the Beam client tags onto its requests for telemetry; we
/// repurpose it for cheap op routing.
#[derive(Debug, Deserialize)]
pub struct GraphqlQuery {
    #[serde(default)]
    pub op: Option<String>,
}

/// Top-level `/graphql/v2` handler. Routes on `?op=...` to a
/// per-operation stub. Unknown operations return a GraphQL-shape
/// error so the client can surface a useful message instead of
/// spinning on a connection failure.
pub async fn graphql_handler(
    State(_state): State<AppState>,
    Query(params): Query<GraphqlQuery>,
    Json(_body): Json<Value>,
) -> Json<Value> {
    match params.op.as_deref() {
        Some("FreeAvailableModels") => Json(free_available_models_stub()),
        Some(op) => Json(json!({
            "errors": [{
                "message": format!(
                    "operation `{op}` is not yet implemented on the Arc relay's /graphql/v2 endpoint"
                ),
                "extensions": { "code": "NOT_IMPLEMENTED" }
            }]
        })),
        None => Json(json!({
            "errors": [{
                "message": "missing ?op=<OperationName> query parameter; bare GraphQL parsing isn't wired yet",
                "extensions": { "code": "BAD_REQUEST" }
            }]
        })),
    }
}

/// Empty `FreeAvailableModels` response. Beam consumes this on first
/// launch to populate the pre-login picker; an empty list there falls
/// back to the local-provider catalog (BYOK + env-var discovery),
/// which is the intended Beam OSS experience anyway.
///
/// The exact shape mirrors the upstream GraphQL response so the
/// client's deserializer doesn't choke:
/// `{"data": {"freeAvailableModels": {"FreeAvailableModelsOutput": {"models": []}}}}`
fn free_available_models_stub() -> Value {
    json!({
        "data": {
            "freeAvailableModels": {
                "__typename": "FreeAvailableModelsOutput",
                "models": []
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper for unit-testing handler logic without spinning the
    /// full Axum router. We build the response directly by calling
    /// the inner function rather than constructing a router.
    #[test]
    fn free_available_models_stub_shape_is_well_formed() {
        let resp = free_available_models_stub();
        // Top-level shape: { "data": { "freeAvailableModels": {...} } }
        let outer = resp.get("data").expect("data");
        let inner = outer.get("freeAvailableModels").expect("freeAvailableModels");
        assert_eq!(
            inner.get("__typename"),
            Some(&Value::String("FreeAvailableModelsOutput".to_owned())),
        );
        let models = inner.get("models").expect("models");
        assert!(models.is_array(), "models must be an array");
        assert!(
            models.as_array().unwrap().is_empty(),
            "stub returns no models"
        );
    }
}

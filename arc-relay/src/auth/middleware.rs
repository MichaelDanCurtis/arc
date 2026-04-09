use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

use crate::auth::jwt::{decode_token, TokenKind};

// ---------------------------------------------------------------------------
// JwtSecret extension
// ---------------------------------------------------------------------------

/// Newtype wrapper around the JWT HMAC secret string.
///
/// Stored in request extensions (inserted by [`jwt_extension_layer`]) so that
/// [`AuthUser`] can retrieve it without depending on a concrete `AppState` type.
#[derive(Clone)]
pub struct JwtSecret(pub String);

// ---------------------------------------------------------------------------
// Middleware layer
// ---------------------------------------------------------------------------

/// Axum middleware that inserts a [`JwtSecret`] extension into every request.
///
/// Mount this on the router **before** any route that uses [`AuthUser`]:
///
/// ```ignore
/// use axum::middleware;
/// use crate::auth::middleware::inject_jwt_secret;
///
/// Router::new()
///     .route("/protected", get(handler))
///     .layer(middleware::from_fn_with_state(secret, inject_jwt_secret));
/// ```
///
/// The secret value is passed at router-build time and cloned per-request.
pub async fn inject_jwt_secret(
    axum::extract::State(secret): axum::extract::State<String>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    req.extensions_mut().insert(JwtSecret(secret));
    next.run(req).await
}

// ---------------------------------------------------------------------------
// AuthUser extractor
// ---------------------------------------------------------------------------

/// Axum extractor that validates a Bearer JWT and yields the authenticated user ID.
///
/// Requires [`JwtSecret`] to have been inserted into request extensions
/// (use [`inject_jwt_secret`] middleware). Only accepts tokens whose `kind`
/// is [`TokenKind::Access`]; refresh tokens are rejected with a 401.
pub struct AuthUser(pub Uuid);

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // ── 1. Extract the JwtSecret from extensions ──────────────────────
        let secret = parts
            .extensions
            .get::<JwtSecret>()
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "JWT secret not configured".to_string(),
                )
            })?
            .0
            .clone();

        // ── 2. Extract the Authorization header ───────────────────────────
        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    "Missing Authorization header".to_string(),
                )
            })?;

        // ── 3. Parse "Bearer <token>" ──────────────────────────────────────
        let token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
            (
                StatusCode::UNAUTHORIZED,
                "Authorization header must use Bearer scheme".to_string(),
            )
        })?;

        // ── 4. Decode and validate JWT ─────────────────────────────────────
        let claims = decode_token(token, &secret)
            .map_err(|e| (StatusCode::UNAUTHORIZED, format!("Invalid token: {e}")))?;

        // ── 5. Ensure it's an access token (not refresh) ──────────────────
        if claims.kind != TokenKind::Access {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Refresh tokens cannot be used for API access".to_string(),
            ));
        }

        // ── 6. Parse the user UUID ────────────────────────────────────────
        let user_id = Uuid::parse_str(&claims.sub).map_err(|e| {
            (
                StatusCode::UNAUTHORIZED,
                format!("Invalid user ID in token: {e}"),
            )
        })?;

        Ok(AuthUser(user_id))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::{generate_token_pair, Claims, TokenKind};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use std::time::{SystemTime, UNIX_EPOCH};
    use tower::util::ServiceExt; // for oneshot
    use uuid::Uuid;

    const SECRET: &str = "test-secret-that-is-long-enough-32x";

    fn test_user_id() -> Uuid {
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    async fn test_handler(AuthUser(user_id): AuthUser) -> String {
        user_id.to_string()
    }

    /// Build a minimal test router that has the JWT secret injected.
    fn test_app() -> Router {
        Router::new()
            .route("/protected", get(test_handler))
            .layer(middleware::from_fn_with_state(
                SECRET.to_string(),
                inject_jwt_secret,
            ))
    }

    fn bearer(token: &str) -> String {
        format!("Bearer {token}")
    }

    // ── helpers ─────────────────────────────────────────────────────────────

    async fn get_protected(app: Router, auth: Option<&str>) -> axum::response::Response {
        let mut req = Request::builder().uri("/protected");
        if let Some(header) = auth {
            req = req.header(axum::http::header::AUTHORIZATION, header);
        }
        app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
    }

    // ── tests ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn valid_bearer_returns_user_id() {
        let user_id = test_user_id();
        let pair = generate_token_pair(user_id, SECRET, 900, 86400).unwrap();

        let resp = get_protected(test_app(), Some(&bearer(&pair.access_token))).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body, user_id.to_string().as_bytes());
    }

    #[tokio::test]
    async fn missing_authorization_header_returns_401() {
        let resp = get_protected(test_app(), None).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn invalid_token_returns_401() {
        let resp = get_protected(test_app(), Some("Bearer not.a.valid.token")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn refresh_token_rejected_with_401() {
        let pair = generate_token_pair(test_user_id(), SECRET, 900, 86400).unwrap();
        // Use the refresh token instead of the access token.
        let resp = get_protected(test_app(), Some(&bearer(&pair.refresh_token))).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_secret_returns_401() {
        let pair = generate_token_pair(
            test_user_id(),
            "different-secret-that-is-long-32xx",
            900,
            86400,
        )
        .unwrap();
        let resp = get_protected(test_app(), Some(&bearer(&pair.access_token))).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn expired_token_returns_401() {
        // Use an exp clearly in the past (1 hour ago) to exceed any leeway.
        let past = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize
            - 3600;

        let claims = Claims {
            sub: test_user_id().to_string(),
            iat: past,
            exp: past, // already expired
            kind: TokenKind::Access,
        };
        let key = EncodingKey::from_secret(SECRET.as_bytes());
        let token = encode(&Header::new(Algorithm::HS256), &claims, &key).unwrap();

        let resp = get_protected(test_app(), Some(&bearer(&token))).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn no_bearer_prefix_returns_401() {
        let pair = generate_token_pair(test_user_id(), SECRET, 900, 86400).unwrap();
        // Send token without the "Bearer " prefix.
        let resp = get_protected(test_app(), Some(&pair.access_token)).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}

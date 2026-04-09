use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    Access,
    Refresh,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// User ID as a string (UUID).
    pub sub: String,
    /// Expiry — Unix timestamp (seconds).
    pub exp: usize,
    /// Issued-at — Unix timestamp (seconds).
    pub iat: usize,
    /// Whether this is an access token or a refresh token.
    pub kind: TokenKind,
}

/// A freshly-minted access/refresh pair.
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Generate a new access/refresh token pair for the given user.
///
/// * `access_ttl_secs`  — lifetime of the access token in seconds.
/// * `refresh_ttl_secs` — lifetime of the refresh token in seconds.
pub fn generate_token_pair(
    user_id: Uuid,
    jwt_secret: &str,
    access_ttl_secs: u64,
    refresh_ttl_secs: u64,
) -> Result<TokenPair, jsonwebtoken::errors::Error> {
    let now = unix_now();
    let sub = user_id.to_string();

    let access_claims = Claims {
        sub: sub.clone(),
        iat: now,
        exp: now + access_ttl_secs as usize,
        kind: TokenKind::Access,
    };

    let refresh_claims = Claims {
        sub,
        iat: now,
        exp: now + refresh_ttl_secs as usize,
        kind: TokenKind::Refresh,
    };

    let key = EncodingKey::from_secret(jwt_secret.as_bytes());
    let header = Header::new(Algorithm::HS256);

    let access_token = encode(&header, &access_claims, &key)?;
    let refresh_token = encode(&header, &refresh_claims, &key)?;

    Ok(TokenPair {
        access_token,
        refresh_token,
    })
}

/// Decode and validate a token, returning its `Claims` on success.
///
/// Validation checks: algorithm must be HS256, expiry must not have passed.
pub fn decode_token(token: &str, jwt_secret: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
    let key = DecodingKey::from_secret(jwt_secret.as_bytes());
    let mut validation = Validation::new(Algorithm::HS256);
    // Validate expiry explicitly.
    validation.validate_exp = true;
    // Set leeway to 0 so there is no clock-skew grace period.
    validation.leeway = 0;

    let data = decode::<Claims>(token, &key, &validation)?;
    Ok(data.claims)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unix_now() -> usize {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time is before Unix epoch")
        .as_secs() as usize
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-that-is-long-enough-32x";

    fn test_user_id() -> Uuid {
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    #[test]
    fn generate_and_decode_access_token() {
        let user_id = test_user_id();
        let pair = generate_token_pair(user_id, SECRET, 900, 86400).unwrap();

        let claims = decode_token(&pair.access_token, SECRET).unwrap();
        assert_eq!(claims.sub, user_id.to_string());
        assert_eq!(claims.kind, TokenKind::Access);
    }

    #[test]
    fn generate_and_decode_refresh_token() {
        let user_id = test_user_id();
        let pair = generate_token_pair(user_id, SECRET, 900, 86400).unwrap();

        let claims = decode_token(&pair.refresh_token, SECRET).unwrap();
        assert_eq!(claims.sub, user_id.to_string());
        assert_eq!(claims.kind, TokenKind::Refresh);
    }

    #[test]
    fn access_token_has_correct_kind() {
        let pair = generate_token_pair(test_user_id(), SECRET, 900, 86400).unwrap();
        let claims = decode_token(&pair.access_token, SECRET).unwrap();
        assert_eq!(claims.kind, TokenKind::Access);
    }

    #[test]
    fn refresh_token_has_correct_kind() {
        let pair = generate_token_pair(test_user_id(), SECRET, 900, 86400).unwrap();
        let claims = decode_token(&pair.refresh_token, SECRET).unwrap();
        assert_eq!(claims.kind, TokenKind::Refresh);
    }

    #[test]
    fn expired_token_returns_error() {
        // Build a token whose exp is clearly in the past (1 hour ago).
        // jsonwebtoken has a default leeway of 60 seconds, so we go well beyond that.
        let past_exp = unix_now().saturating_sub(3600); // 1 hour in the past
        let claims = Claims {
            sub: test_user_id().to_string(),
            iat: past_exp,
            exp: past_exp,
            kind: TokenKind::Access,
        };
        let key = EncodingKey::from_secret(SECRET.as_bytes());
        let token = encode(&Header::new(Algorithm::HS256), &claims, &key).unwrap();

        let err = decode_token(&token, SECRET).unwrap_err();
        assert!(
            err.to_string().contains("ExpiredSignature"),
            "expected ExpiredSignature, got: {err}"
        );
    }

    #[test]
    fn wrong_secret_returns_error() {
        let pair = generate_token_pair(test_user_id(), SECRET, 900, 86400).unwrap();
        let err =
            decode_token(&pair.access_token, "wrong-secret-that-is-long-enough-32x").unwrap_err();
        assert!(
            err.to_string().contains("InvalidSignature"),
            "expected InvalidSignature, got: {err}"
        );
    }

    #[test]
    fn access_and_refresh_tokens_are_different_strings() {
        let pair = generate_token_pair(test_user_id(), SECRET, 900, 86400).unwrap();
        assert_ne!(pair.access_token, pair.refresh_token);
    }
}

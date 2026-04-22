use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Redirect,
};
use oauth2::{
    basic::BasicClient, reqwest::async_http_client, AuthUrl, AuthorizationCode, ClientId,
    ClientSecret, CsrfToken, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde::Deserialize;

use crate::api::AppState;
use crate::auth::jwt;
use crate::models::{organization, user};

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: String,
    pub state: String,
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfo {
    email: String,
    name: Option<String>,
    picture: Option<String>,
    sub: String,
}

#[derive(Debug, Deserialize)]
struct GitHubUserInfo {
    email: Option<String>,
    name: Option<String>,
    avatar_url: Option<String>,
    id: i64,
}

#[derive(Debug, Deserialize)]
struct GitHubEmail {
    email: String,
    primary: bool,
    verified: bool,
}

#[derive(Debug, Deserialize)]
struct MicrosoftUserInfo {
    mail: Option<String>,
    #[serde(rename = "userPrincipalName")]
    user_principal_name: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    id: String,
}

fn build_oauth_client(
    provider: &str,
    client_id: &str,
    client_secret: &str,
    redirect_url: &str,
) -> Result<BasicClient, (StatusCode, String)> {
    let (auth_url_str, token_url_str) = match provider {
        "google" => (
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
        ),
        "microsoft" => (
            "https://login.microsoftonline.com/common/oauth2/v2/authorize",
            "https://login.microsoftonline.com/common/oauth2/v2/token",
        ),
        "github" => (
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
        ),
        _ => return Err((StatusCode::BAD_REQUEST, format!("Unknown provider: {provider}"))),
    };

    let auth_url = AuthUrl::new(auth_url_str.to_string())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid auth URL: {e}")))?;
    let token_url = TokenUrl::new(token_url_str.to_string())
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid token URL: {e}")))?;

    let client = BasicClient::new(
        ClientId::new(client_id.to_string()),
        Some(ClientSecret::new(client_secret.to_string())),
        auth_url,
        Some(token_url),
    )
    .set_redirect_uri(
        RedirectUrl::new(redirect_url.to_string())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid redirect URL: {e}")))?,
    );

    Ok(client)
}

fn get_scopes(provider: &str) -> Vec<Scope> {
    match provider {
        "google" => vec![
            Scope::new("openid".to_string()),
            Scope::new("email".to_string()),
            Scope::new("profile".to_string()),
        ],
        "microsoft" => vec![
            Scope::new("openid".to_string()),
            Scope::new("email".to_string()),
            Scope::new("profile".to_string()),
            Scope::new("User.Read".to_string()),
        ],
        "github" => vec![
            Scope::new("user:email".to_string()),
            Scope::new("read:user".to_string()),
        ],
        _ => vec![],
    }
}

fn get_provider_credentials(
    state: &AppState,
    provider: &str,
) -> Result<(String, String), (StatusCode, String)> {
    let (id, secret) = match provider {
        "google" => (
            state.config.oauth_google_client_id.as_deref(),
            state.config.oauth_google_client_secret.as_deref(),
        ),
        "microsoft" => (
            state.config.oauth_microsoft_client_id.as_deref(),
            state.config.oauth_microsoft_client_secret.as_deref(),
        ),
        "github" => (
            state.config.oauth_github_client_id.as_deref(),
            state.config.oauth_github_client_secret.as_deref(),
        ),
        _ => return Err((StatusCode::BAD_REQUEST, format!("Unknown provider: {provider}"))),
    };
    let id = id.ok_or_else(|| {
        (
            StatusCode::NOT_IMPLEMENTED,
            format!("{provider} OAuth not configured"),
        )
    })?;
    let secret = secret.ok_or_else(|| {
        (
            StatusCode::NOT_IMPLEMENTED,
            format!("{provider} OAuth not configured"),
        )
    })?;
    Ok((id.to_string(), secret.to_string()))
}

/// GET /api/auth/oauth/:provider
///
/// Redirects the user to the OAuth2 provider's authorization page.
pub async fn oauth_redirect(
    State(state): State<AppState>,
    Path(provider): Path<String>,
) -> Result<Redirect, (StatusCode, String)> {
    let (client_id, client_secret) = get_provider_credentials(&state, &provider)?;
    let redirect_url = format!(
        "{}/api/auth/oauth/{}/callback",
        state.config.oauth_redirect_base, provider
    );

    let client = build_oauth_client(&provider, &client_id, &client_secret, &redirect_url)?;
    let scopes = get_scopes(&provider);

    let mut auth_request = client.authorize_url(CsrfToken::new_random);
    for scope in scopes {
        auth_request = auth_request.add_scope(scope);
    }
    let (auth_url, _csrf_token) = auth_request.url();

    Ok(Redirect::temporary(auth_url.as_str()))
}

/// GET /api/auth/oauth/:provider/callback
///
/// Handles the OAuth2 callback, exchanges the auth code for tokens,
/// fetches user info, upserts user, and redirects with JWT.
pub async fn oauth_callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
) -> Result<Redirect, (StatusCode, String)> {
    let (client_id, client_secret) = get_provider_credentials(&state, &provider)?;
    let redirect_url = format!(
        "{}/api/auth/oauth/{}/callback",
        state.config.oauth_redirect_base, provider
    );

    let client = build_oauth_client(&provider, &client_id, &client_secret, &redirect_url)?;

    let token_result = client
        .exchange_code(AuthorizationCode::new(query.code.clone()))
        .request_async(async_http_client)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token exchange failed: {e}")))?;

    let access_token = token_result.access_token().secret().to_string();

    let (email, name, avatar, provider_id) = match provider.as_str() {
        "google" => fetch_google_user(&access_token).await?,
        "microsoft" => fetch_microsoft_user(&access_token).await?,
        "github" => fetch_github_user(&access_token).await?,
        _ => return Err((StatusCode::BAD_REQUEST, "Unknown provider".to_string())),
    };

    let db_user = user::upsert_oauth_user(
        &state.pool,
        &email,
        name.as_deref(),
        avatar.as_deref(),
        &provider,
        &provider_id,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB: {e}")))?;

    // Domain-based org auto-association
    if let Some(domain) = email.split('@').nth(1) {
        if let Ok(Some(org)) = organization::find_org_by_domain(&state.pool, domain).await {
            let _ = organization::add_org_member(&state.pool, db_user.id, org.id, "member").await;
        }
    }

    let pair = jwt::generate_token_pair(
        db_user.id,
        &state.config.jwt_secret,
        state.config.access_token_ttl_secs,
        state.config.refresh_token_ttl_secs,
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("JWT: {e}")))?;

    state
        .audit
        .record(
            &db_user.id.to_string(),
            &format!("user.oauth.{provider}"),
            &format!("user/{}", db_user.id),
            &email,
        )
        .await;

    // Redirect to frontend with tokens as query params (frontend stores them)
    let redirect_to = format!(
        "{}/auth/callback?access_token={}&refresh_token={}&user_id={}",
        state.config.oauth_redirect_base,
        pair.access_token,
        pair.refresh_token,
        db_user.id,
    );

    Ok(Redirect::temporary(&redirect_to))
}

async fn fetch_google_user(
    access_token: &str,
) -> Result<(String, Option<String>, Option<String>, String), (StatusCode, String)> {
    let client = reqwest::Client::new();
    let info: GoogleUserInfo = client
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Google API: {e}")))?
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Google parse: {e}")))?;
    Ok((info.email, info.name, info.picture, info.sub))
}

async fn fetch_github_user(
    access_token: &str,
) -> Result<(String, Option<String>, Option<String>, String), (StatusCode, String)> {
    let client = reqwest::Client::new();

    let info: GitHubUserInfo = client
        .get("https://api.github.com/user")
        .bearer_auth(access_token)
        .header("User-Agent", "arc-relay")
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("GitHub API: {e}")))?
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("GitHub parse: {e}")))?;

    let email = if let Some(email) = info.email {
        email
    } else {
        let emails: Vec<GitHubEmail> = client
            .get("https://api.github.com/user/emails")
            .bearer_auth(access_token)
            .header("User-Agent", "arc-relay")
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("GitHub emails: {e}")))?
            .json()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("GitHub emails parse: {e}")))?;

        emails
            .into_iter()
            .find(|e| e.primary && e.verified)
            .map(|e| e.email)
            .ok_or_else(|| {
                (
                    StatusCode::BAD_GATEWAY,
                    "No verified primary email on GitHub".to_string(),
                )
            })?
    };

    Ok((email, info.name, info.avatar_url, info.id.to_string()))
}

async fn fetch_microsoft_user(
    access_token: &str,
) -> Result<(String, Option<String>, Option<String>, String), (StatusCode, String)> {
    let client = reqwest::Client::new();
    let info: MicrosoftUserInfo = client
        .get("https://graph.microsoft.com/v1.0/me")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Microsoft API: {e}")))?
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Microsoft parse: {e}")))?;

    let email = info
        .mail
        .or(info.user_principal_name)
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No email from Microsoft".to_string()))?;

    Ok((email, info.display_name, None, info.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_user_info_deserializes() {
        let json = r#"{"email":"a@b.com","name":"Alice","picture":"https://img","sub":"123"}"#;
        let info: GoogleUserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.email, "a@b.com");
        assert_eq!(info.sub, "123");
    }

    #[test]
    fn github_user_info_deserializes() {
        let json = r#"{"email":"a@b.com","name":"Alice","avatar_url":"https://img","id":42}"#;
        let info: GitHubUserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, 42);
    }

    #[test]
    fn microsoft_user_info_deserializes() {
        let json =
            r#"{"mail":"a@b.com","displayName":"Alice","id":"abc","userPrincipalName":null}"#;
        let info: MicrosoftUserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(info.id, "abc");
    }

    #[test]
    fn callback_query_deserializes() {
        let json = r#"{"code":"authcode123","state":"csrfstate"}"#;
        let q: OAuthCallbackQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.code, "authcode123");
    }

    #[test]
    fn get_scopes_returns_correct_scopes() {
        assert_eq!(get_scopes("google").len(), 3);
        assert_eq!(get_scopes("microsoft").len(), 4);
        assert_eq!(get_scopes("github").len(), 2);
        assert_eq!(get_scopes("unknown").len(), 0);
    }
}

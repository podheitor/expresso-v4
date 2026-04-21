//! CardDAV HTTP Basic authentication — MVP.
//!
//! Dev mode (`CONTACTS_DEV_AUTH=1`): username is treated as a user email; we
//! look up the user + tenant in DB. Password is NOT validated.
//! Production path (TODO): delegate to Keycloak token introspection.

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use uuid::Uuid;

use crate::state::AppState;

#[derive(Debug, Clone, Copy)]
pub struct CardDavPrincipal {
    pub tenant_id: Uuid,
    pub user_id:   Uuid,
}

#[async_trait]
impl FromRequestParts<AppState> for CardDavPrincipal {
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, AuthError> {
        let header_val = parts
            .headers
            .get(header::AUTHORIZATION)
            .ok_or(AuthError::Missing)?
            .to_str()
            .map_err(|_| AuthError::Malformed)?;

        let (user, _pass) = decode_basic(header_val).ok_or(AuthError::Malformed)?;

        // Dev path: resolve user by email, accept any password.
        if std::env::var("CONTACTS_DEV_AUTH").ok().as_deref() == Some("1") {
            let pool = state.db().ok_or(AuthError::Unavailable)?;
            let row: Option<(Uuid, Uuid)> = sqlx::query_as(
                r#"SELECT tenant_id, id FROM users WHERE email = $1 LIMIT 1"#,
            )
            .bind(&user)
            .fetch_optional(pool)
            .await
            .map_err(|_| AuthError::Unavailable)?;

            let (tenant_id, user_id) = row.ok_or(AuthError::Forbidden)?;
            return Ok(CardDavPrincipal { tenant_id, user_id });
        }

        // TODO: Keycloak token introspection / password validation.
        Err(AuthError::Forbidden)
    }
}

/// Decode `Basic <b64(user:pass)>` → (user, pass).
fn decode_basic(header: &str) -> Option<(String, String)> {
    let token = header.strip_prefix("Basic ").or_else(|| header.strip_prefix("basic "))?;
    let decoded = STANDARD.decode(token.trim()).ok()?;
    let s = String::from_utf8(decoded).ok()?;
    let (u, p) = s.split_once(':')?;
    Some((u.to_owned(), p.to_owned()))
}

#[derive(Debug)]
pub enum AuthError {
    Missing,
    Malformed,
    Forbidden,
    Unavailable,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let challenge = HeaderValue::from_static("Basic realm=\"Expresso CardDAV\", charset=\"UTF-8\"");
        let (status, msg) = match self {
            Self::Missing     => (StatusCode::UNAUTHORIZED, "authentication required"),
            Self::Malformed   => (StatusCode::BAD_REQUEST, "malformed Authorization header"),
            Self::Forbidden   => (StatusCode::UNAUTHORIZED, "invalid credentials"),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "auth backend unavailable"),
        };
        let mut resp = (status, msg).into_response();
        if matches!(status, StatusCode::UNAUTHORIZED) {
            resp.headers_mut().insert(header::WWW_AUTHENTICATE, challenge);
        }
        resp
    }
}

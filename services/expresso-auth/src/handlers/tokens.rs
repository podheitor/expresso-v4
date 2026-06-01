//! Personal access tokens (PATs) — phase 1: issuance + management.
//!
//! POST   /auth/tokens        — mint a token (cleartext returned ONCE)
//! GET    /auth/tokens        — list the caller's tokens (metadata only)
//! DELETE /auth/tokens/:id    — revoke a token the caller owns
//!
//! Only sha256(token) is stored. Accepting a PAT as a request credential lives
//! in a later phase — the shared auth-client validator is JWT-only today.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use uuid::Uuid;

use expresso_auth_client::Authenticated;

use crate::state::AppState;

/// Cap on a token's optional lifetime (1 year) and the per-user token count.
const MAX_TTL_SECONDS: i64 = 365 * 24 * 3600;
const MAX_TOKENS_PER_USER: i64 = 50;

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn pool(st: &AppState) -> Result<&sqlx::PgPool, ApiError> {
    st.pool
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "database not configured"))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    /// Optional lifetime; omit for a non-expiring token. 1..=1y.
    pub expires_in_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CreateResp {
    pub id: Uuid,
    pub name: String,
    /// Cleartext token — shown once. Store it now; only its hash is kept.
    pub token: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

/// POST /auth/tokens — mint a PAT for the authenticated user.
pub async fn create(
    State(st): State<Arc<AppState>>,
    Authenticated(ctx): Authenticated,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<CreateResp>), ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Err(err(StatusCode::BAD_REQUEST, "name must be 1..128 chars"));
    }
    let expires_at = match body.expires_in_seconds {
        None => None,
        Some(s) if (1..=MAX_TTL_SECONDS).contains(&s) => {
            Some(OffsetDateTime::now_utc() + time::Duration::seconds(s))
        }
        Some(_) => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "expires_in_seconds must be 1..=31536000",
            ))
        }
    };
    let pool = pool(&st)?;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM api_tokens \
          WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if count >= MAX_TOKENS_PER_USER {
        return Err(err(StatusCode::CONFLICT, "token limit reached"));
    }

    // 32 random bytes, base64url; only sha256 persists. Prefix marks it a PAT.
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = format!("ept_{}", URL_SAFE_NO_PAD.encode(raw));
    let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO api_tokens (tenant_id, user_id, name, token_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(name)
    .bind(&token_hash)
    .bind(expires_at)
    .fetch_one(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(CreateResp {
            id,
            name: name.to_owned(),
            token,
            expires_at,
        }),
    ))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TokenInfo {
    pub id: Uuid,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

/// GET /auth/tokens — list the caller's tokens (metadata only, never the secret).
pub async fn list(
    State(st): State<Arc<AppState>>,
    Authenticated(ctx): Authenticated,
) -> Result<Json<Vec<TokenInfo>>, ApiError> {
    let pool = pool(&st)?;
    let rows = sqlx::query_as::<_, TokenInfo>(
        "SELECT id, name, created_at, last_used_at, expires_at, revoked_at \
           FROM api_tokens \
          WHERE tenant_id = $1 AND user_id = $2 \
          ORDER BY created_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(rows))
}

/// DELETE /auth/tokens/:id — revoke a token. Only the owner may revoke; a
/// foreign/absent id is a 404 (no leak). Idempotent on an already-revoked token.
pub async fn revoke(
    State(st): State<Arc<AppState>>,
    Authenticated(ctx): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let pool = pool(&st)?;
    let res = sqlx::query(
        "UPDATE api_tokens SET revoked_at = now() \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if res.rows_affected() == 0 {
        // Either not found, or already revoked — distinguish for idempotency.
        let exists: Option<(Uuid,)> = sqlx::query_as(
            "SELECT id FROM api_tokens WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
        )
        .bind(id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        if exists.is_none() {
            return Err(err(StatusCode::NOT_FOUND, "token not found"));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

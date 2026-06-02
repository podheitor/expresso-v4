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

    // 32 random bytes, base64url; only sha256 persists. Prefix marks it a PAT.
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = format!("ept_{}", URL_SAFE_NO_PAD.encode(raw));
    let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));

    // Enforce the per-user cap without a TOCTOU race. A per-(tenant,user)
    // transaction-scoped advisory lock serialises concurrent create requests for
    // the same user, so the count-then-insert can't be raced past the cap (the
    // previous check-then-insert let concurrent requests exceed MAX_TOKENS_PER_USER).
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, $2))")
        .bind(format!("{}:{}", ctx.tenant_id, ctx.user_id))
        .bind(0x70_61_74_5f_63_61_70_i64) // "pat_cap" namespace salt
        .execute(&mut *tx)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM api_tokens \
          WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if count >= MAX_TOKENS_PER_USER {
        return Err(err(StatusCode::CONFLICT, "token limit reached"));
    }

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO api_tokens (tenant_id, user_id, name, token_hash, expires_at) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(name)
    .bind(&token_hash)
    .bind(expires_at)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    tx.commit()
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

#[derive(Debug, Deserialize)]
pub struct IntrospectBody {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct IntrospectResp {
    /// True only when the token is known, not revoked, and not expired.
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// POST /internal/tokens/introspect — resolve a PAT to its owner's context.
///
/// PHASE 2 of personal access tokens: a trusted LAN caller (gateway) exchanges a
/// `ept_…` token for the owning user's identity. Validates hash + not-revoked +
/// not-expired, joins `users` for email/role, and stamps `last_used_at`. This is
/// an `/internal/*` endpoint (network-trusted, no user auth), matching the
/// notify/process internal endpoints. An unknown/dead token returns
/// `{"active": false}` (never 401, so the caller can branch cleanly).
pub async fn introspect(
    State(st): State<Arc<AppState>>,
    Json(body): Json<IntrospectBody>,
) -> Result<Json<IntrospectResp>, ApiError> {
    let pool = pool(&st)?;
    let token = body.token.trim();
    if token.is_empty() {
        return Ok(Json(inactive()));
    }
    let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));

    // Resolve the live token to its owner, joining users for the full context.
    let row: Option<(Uuid, Uuid, Uuid, String, String, String)> = sqlx::query_as(
        "SELECT t.id, t.user_id, t.tenant_id, u.email, u.display_name, u.role \
           FROM api_tokens t \
           JOIN users u ON u.id = t.user_id AND u.tenant_id = t.tenant_id \
          WHERE t.token_hash = $1 \
            AND t.revoked_at IS NULL \
            AND (t.expires_at IS NULL OR t.expires_at > now())",
    )
    .bind(&token_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let Some((token_id, user_id, tenant_id, email, display_name, role)) = row else {
        return Ok(Json(inactive()));
    };

    // Best-effort usage stamp; a failure here must not fail the introspection.
    let _ = sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1")
        .bind(token_id)
        .execute(pool)
        .await;

    Ok(Json(IntrospectResp {
        active: true,
        user_id: Some(user_id),
        tenant_id: Some(tenant_id),
        email: Some(email),
        display_name: Some(display_name),
        role: Some(role),
    }))
}

fn inactive() -> IntrospectResp {
    IntrospectResp {
        active: false,
        user_id: None,
        tenant_id: None,
        email: None,
        display_name: None,
        role: None,
    }
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

//! Mailbox delegation grants (phase 1 — model + discovery).
//!
//! GET    /api/v1/mail/delegations        — grants the caller has GIVEN (owner)
//! POST   /api/v1/mail/delegations        — grant delegated access to a user
//! GET    /api/v1/mail/delegations/to-me  — grants GIVEN TO the caller (delegate)
//! DELETE /api/v1/mail/delegations/:id     — revoke a grant the caller owns
//!
//! A grant lets `delegate_id` act on `owner_id`'s mailbox at `READ` or `SEND`
//! level. This module manages the grants and exposes discovery; it does NOT yet
//! change the read/send paths — those still scope to the caller's own mailbox.
//! Enforcement (an on-behalf-of selector honoring these grants) is a later
//! phase, matching how drive ACL shipped capability before enforcement.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;
use expresso_core::begin_tenant_tx;

/// `to-me` is a static segment — register it before nothing param-shaped exists
/// here, but keep the ordering explicit for clarity against future `:id` routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/delegations", get(list_granted).post(grant))
        .route("/mail/delegations/to-me", get(list_to_me))
        .route("/mail/delegations/:id", axum::routing::delete(revoke))
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Delegation {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub owner_id: Uuid,
    pub delegate_id: Uuid,
    pub access: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct GrantBody {
    pub delegate_id: Uuid,
    pub access: String,
}

fn validate_access(raw: &str) -> Result<String> {
    let up = raw.trim().to_ascii_uppercase();
    match up.as_str() {
        "READ" | "SEND" => Ok(up),
        _ => Err(MailError::BadRequest(format!("invalid access: {raw}"))),
    }
}

/// POST /mail/delegations — grant `delegate_id` access to the caller's mailbox.
/// The caller is always the owner. Self-grants and unknown delegates are 400.
async fn grant(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<GrantBody>,
) -> Result<(StatusCode, Json<Delegation>)> {
    let pool = state.db();
    if body.delegate_id == ctx.user_id {
        return Err(MailError::BadRequest("cannot delegate to yourself".into()));
    }
    let access = validate_access(&body.access)?;

    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;
    // Confirm the delegate exists in the tenant — avoid a dangling grant.
    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE id = $1 AND tenant_id = $2")
            .bind(body.delegate_id)
            .bind(ctx.tenant_id)
            .fetch_optional(&mut *tx)
            .await?;
    if exists.is_none() {
        return Err(MailError::BadRequest(format!(
            "unknown delegate: {}",
            body.delegate_id
        )));
    }
    let row: Delegation = sqlx::query_as(
        "INSERT INTO mailbox_delegations (tenant_id, owner_id, delegate_id, access) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (tenant_id, owner_id, delegate_id) \
         DO UPDATE SET access = EXCLUDED.access \
         RETURNING id, tenant_id, owner_id, delegate_id, access, created_at",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(body.delegate_id)
    .bind(&access)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /mail/delegations — grants the caller has given (caller is owner).
async fn list_granted(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<Delegation>>> {
    let pool = state.db();
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;
    let rows = sqlx::query_as::<_, Delegation>(
        "SELECT id, tenant_id, owner_id, delegate_id, access, created_at \
           FROM mailbox_delegations \
          WHERE tenant_id = $1 AND owner_id = $2 ORDER BY created_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// GET /mail/delegations/to-me — grants given to the caller (caller is delegate).
async fn list_to_me(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<Delegation>>> {
    let pool = state.db();
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;
    let rows = sqlx::query_as::<_, Delegation>(
        "SELECT id, tenant_id, owner_id, delegate_id, access, created_at \
           FROM mailbox_delegations \
          WHERE tenant_id = $1 AND delegate_id = $2 ORDER BY created_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

/// DELETE /mail/delegations/:id — revoke a grant. Only the owner may revoke;
/// the `owner_id = caller` predicate makes a foreign id a 404 (no leak).
async fn revoke(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db();
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;
    let res = sqlx::query(
        "DELETE FROM mailbox_delegations \
          WHERE id = $1 AND tenant_id = $2 AND owner_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if res.rows_affected() == 0 {
        return Err(MailError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_access_normalizes_case() {
        assert_eq!(validate_access("read").unwrap(), "READ");
        assert_eq!(validate_access(" Send ").unwrap(), "SEND");
    }

    #[test]
    fn validate_access_rejects_unknown() {
        assert!(validate_access("admin").is_err());
        assert!(validate_access("").is_err());
    }

    #[test]
    fn grant_body_deserializes() {
        let b: GrantBody = serde_json::from_str(
            r#"{"delegate_id":"00000000-0000-0000-0000-000000000000","access":"READ"}"#,
        )
        .unwrap();
        assert_eq!(b.access, "READ");
    }
}

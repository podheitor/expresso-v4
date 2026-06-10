//! Safe senders — a per-user allow-list. Inbound mail whose From matches is
//! always delivered to the Inbox, overriding the blocked list and Sieve spam
//! filing (Outlook "safe senders").
//!
//! GET    /api/v1/mail/safe-senders            → list the caller's safe addresses
//! PUT    /api/v1/mail/safe-senders/:address   → mark an address safe (idempotent)
//! DELETE /api/v1/mail/safe-senders/:address   → remove an address
//!
//! The delivery path (ingest) consults this list first; see `is_safe`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/safe-senders", get(list_safe))
        .route(
            "/mail/safe-senders/:address",
            put(add_safe).delete(remove_safe),
        )
}

/// Normalize an address for storage/compare: trimmed + lowercased.
fn norm(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

async fn list_safe(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<String>>> {
    let pool = state.db();
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT address FROM mail_safe_senders \
         WHERE tenant_id = $1 AND user_id = $2 ORDER BY address",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

/// PUT /api/v1/mail/safe-senders/:address — mark an address safe (idempotent).
async fn add_safe(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(address): Path<String>,
) -> Result<StatusCode> {
    let addr = norm(&address);
    if !addr.contains('@') {
        return Err(MailError::BadRequest("address must be an email".into()));
    }
    let pool = state.db();
    sqlx::query(
        "INSERT INTO mail_safe_senders (tenant_id, user_id, address) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&addr)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/mail/safe-senders/:address — remove an address.
async fn remove_safe(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(address): Path<String>,
) -> Result<StatusCode> {
    let pool = state.db();
    sqlx::query(
        "DELETE FROM mail_safe_senders \
         WHERE tenant_id = $1 AND user_id = $2 AND address = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(norm(&address))
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Returns true when `from_addr` is on `user_id`'s safe list. Used by the
/// ingest delivery path to force a message to the Inbox. Runs in the delivery
/// transaction so it sees the same tenant RLS context.
pub async fn is_safe(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    user_id: Uuid,
    from_addr: &str,
) -> Result<bool> {
    let addr = norm(from_addr);
    if addr.is_empty() {
        return Ok(false);
    }
    let hit: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM mail_safe_senders \
         WHERE tenant_id = $1 AND user_id = $2 AND address = $3",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(&addr)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(hit.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_lowercases_and_trims() {
        assert_eq!(norm("  Foo@Bar.COM "), "foo@bar.com");
    }
}

//! Blocked senders — a per-user list of addresses whose inbound mail is routed
//! to Spam at delivery time (Outlook "blocked senders").
//!
//! GET    /api/v1/mail/blocked-senders            → list the caller's blocked addresses
//! PUT    /api/v1/mail/blocked-senders/:address   → block an address (idempotent)
//! DELETE /api/v1/mail/blocked-senders/:address   → unblock an address
//!
//! The delivery path (ingest) consults this list; see `blocked_senders_for`.

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
        .route("/mail/blocked-senders", get(list_blocked))
        .route(
            "/mail/blocked-senders/:address",
            put(add_blocked).delete(remove_blocked),
        )
}

/// Normalize an address for storage/compare: trimmed + lowercased.
fn norm(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

async fn list_blocked(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<String>>> {
    let pool = state.db();
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT address FROM mail_blocked_senders \
         WHERE tenant_id = $1 AND user_id = $2 ORDER BY address",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

/// PUT /api/v1/mail/blocked-senders/:address — block an address (idempotent).
async fn add_blocked(
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
        "INSERT INTO mail_blocked_senders (tenant_id, user_id, address) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&addr)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/mail/blocked-senders/:address — unblock an address.
async fn remove_blocked(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(address): Path<String>,
) -> Result<StatusCode> {
    let pool = state.db();
    sqlx::query(
        "DELETE FROM mail_blocked_senders \
         WHERE tenant_id = $1 AND user_id = $2 AND address = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(norm(&address))
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Returns true when `from_addr` is on `user_id`'s blocked list. Used by the
/// ingest delivery path to divert blocked mail to Spam. Runs inside the
/// delivery transaction so it sees the same tenant RLS context.
pub async fn is_blocked(
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
        "SELECT 1 FROM mail_blocked_senders \
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

//! Recent contacts — a per-user LRU of viewed contacts (People "recents").
//!
//! POST /api/v1/contacts/:book_id/:id/touch → record that the caller viewed it
//! GET  /api/v1/contact-recents             → the caller's recently-viewed contacts
//!
//! `touch` upserts `accessed_at = now()`; `recents` orders by it.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::contact::Contact;
use crate::error::Result;
use crate::state::AppState;

/// How many recents to return.
const RECENTS_LIMIT: i64 = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/contact-recents", get(list_recents))
        .route("/api/v1/contacts/:book_id/:id/touch", post(touch))
}

/// POST /api/v1/contacts/:book_id/:id/touch — record an access (upsert now()).
async fn touch(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_book_id, id)): Path<(String, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    // FK to contacts(id) ensures the contact exists; the conflict updates the
    // timestamp so the table holds one row per (user, contact).
    sqlx::query(
        "INSERT INTO contact_access (tenant_id, user_id, contact_id) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, user_id, contact_id) \
         DO UPDATE SET accessed_at = now()",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/contact-recents — the caller's recently-viewed contacts, most
/// recent first (joined to the live contact rows; deleted contacts drop out).
async fn list_recents(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<Contact>>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<Contact> = sqlx::query_as(
        "SELECT c.* FROM contacts c \
         JOIN contact_access a ON a.contact_id = c.id \
         WHERE a.tenant_id = $1 AND a.user_id = $2 \
         ORDER BY a.accessed_at DESC \
         LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(RECENTS_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

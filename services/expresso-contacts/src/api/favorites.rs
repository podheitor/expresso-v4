//! Contact favorites — a per-user "starred contacts" overlay.
//!
//! GET    /api/v1/contact-favorites              → list the caller's favorite contact ids
//! PUT    /api/v1/contacts/:book_id/:id/favorite → star a contact
//! DELETE /api/v1/contacts/:book_id/:id/favorite → unstar a contact
//!
//! Favorites are personal and do not touch the shared vCard. Star/unstar is
//! idempotent.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::Result;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/contact-favorites", get(list_favorites))
        .route(
            "/api/v1/contacts/:book_id/:id/favorite",
            put(add_favorite).delete(remove_favorite),
        )
}

/// GET /api/v1/contact-favorites — the caller's favorite contact ids.
async fn list_favorites(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<Uuid>>> {
    let pool = state.db_or_unavailable()?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT contact_id FROM contact_favorites \
         WHERE tenant_id = $1 AND user_id = $2 ORDER BY created_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(ids))
}

/// PUT /api/v1/contacts/:book_id/:id/favorite — star a contact (idempotent).
async fn add_favorite(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_book_id, id)): Path<(String, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    // The FK to contacts(id) enforces the contact exists; ON CONFLICT makes
    // re-starring a no-op.
    sqlx::query(
        "INSERT INTO contact_favorites (tenant_id, user_id, contact_id) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/contacts/:book_id/:id/favorite — unstar a contact.
async fn remove_favorite(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_book_id, id)): Path<(String, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    sqlx::query(
        "DELETE FROM contact_favorites \
         WHERE tenant_id = $1 AND user_id = $2 AND contact_id = $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

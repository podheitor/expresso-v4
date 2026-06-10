//! Mail labels — a per-user "categories" overlay on messages (Outlook
//! categories). Labels are personal and never modify the shared message.
//!
//! GET    /api/v1/mail/labels                       → { "<message_id>": ["importante", …], … }
//! PUT    /api/v1/mail/messages/:id/labels/:label   → add a label (idempotent)
//! DELETE /api/v1/mail/messages/:id/labels/:label   → remove a label

use std::collections::HashMap;

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

/// Cap on a single label's length — these are short category names, not notes.
const MAX_LABEL_LEN: usize = 64;

pub fn routes() -> Router<AppState> {
    Router::new().route("/mail/labels", get(list_labels)).route(
        "/mail/messages/:id/labels/:label",
        put(add_label).delete(remove_label),
    )
}

#[derive(sqlx::FromRow)]
struct LabelRow {
    message_id: Uuid,
    label: String,
}

/// GET /api/v1/mail/labels — every (message → labels) pair for the caller, as a
/// map keyed by message id so the inbox can render tags in one fetch.
async fn list_labels(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<HashMap<String, Vec<String>>>> {
    let pool = state.db();
    let rows: Vec<LabelRow> = sqlx::query_as(
        "SELECT message_id, label FROM mail_message_labels \
         WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for r in rows {
        map.entry(r.message_id.to_string())
            .or_default()
            .push(r.label);
    }
    Ok(Json(map))
}

fn validate_label(label: &str) -> Result<()> {
    if label.trim().is_empty() {
        return Err(MailError::BadRequest("label must not be empty".into()));
    }
    if label.len() > MAX_LABEL_LEN {
        return Err(MailError::BadRequest("label too long (max 64)".into()));
    }
    Ok(())
}

/// PUT /api/v1/mail/messages/:id/labels/:label — add a label (idempotent).
async fn add_label(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, label)): Path<(Uuid, String)>,
) -> Result<StatusCode> {
    validate_label(&label)?;
    let pool = state.db();
    sqlx::query(
        "INSERT INTO mail_message_labels (tenant_id, user_id, message_id, label) \
         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .bind(label.trim())
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/mail/messages/:id/labels/:label — remove a label.
async fn remove_label(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, label)): Path<(Uuid, String)>,
) -> Result<StatusCode> {
    let pool = state.db();
    sqlx::query(
        "DELETE FROM mail_message_labels \
         WHERE tenant_id = $1 AND user_id = $2 AND message_id = $3 AND label = $4",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .bind(label.trim())
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_label_bounds() {
        assert!(validate_label("importante").is_ok());
        assert!(validate_label("  ").is_err());
        assert!(validate_label(&"x".repeat(MAX_LABEL_LEN + 1)).is_err());
    }
}

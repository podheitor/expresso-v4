//! Sweep rules — per-user "auto-move old mail from a sender" rules (Outlook
//! Sweep). A background worker applies enabled rules: messages from
//! `sender_address` older than `older_than_days` are moved into `target_folder`.
//!
//! GET    /api/v1/mail/sweep-rules        — list the caller's rules
//! POST   /api/v1/mail/sweep-rules        — create a rule
//! DELETE /api/v1/mail/sweep-rules/:id    — delete a rule

use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/sweep-rules", get(list_rules).post(create_rule))
        .route("/mail/sweep-rules/:id", axum::routing::delete(delete_rule))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SweepRule {
    pub id: Uuid,
    pub sender_address: String,
    pub older_than_days: i32,
    pub target_folder: String,
    pub enabled: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct NewSweepRule {
    pub sender_address: String,
    #[serde(default = "default_days")]
    pub older_than_days: i32,
    #[serde(default = "default_folder")]
    pub target_folder: String,
}

fn default_days() -> i32 {
    7
}
fn default_folder() -> String {
    "Trash".to_string()
}

const SELECT_COLS: &str = "id, sender_address, older_than_days, target_folder, enabled, created_at";

async fn list_rules(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<SweepRule>>> {
    let pool = state.db();
    let rows: Vec<SweepRule> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM mail_sweep_rules \
         WHERE tenant_id = $1 AND user_id = $2 ORDER BY created_at DESC"
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

async fn create_rule(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewSweepRule>,
) -> Result<(StatusCode, Json<SweepRule>)> {
    let sender = body.sender_address.trim().to_ascii_lowercase();
    if !sender.contains('@') {
        return Err(MailError::BadRequest("sender must be an email".into()));
    }
    let days = body.older_than_days.clamp(0, 3650);
    let folder = body.target_folder.trim();
    if folder.is_empty() {
        return Err(MailError::BadRequest("target_folder required".into()));
    }
    let pool = state.db();
    let row: SweepRule = sqlx::query_as(&format!(
        "INSERT INTO mail_sweep_rules (tenant_id, user_id, sender_address, older_than_days, target_folder) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {SELECT_COLS}"
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&sender)
    .bind(days)
    .bind(folder)
    .fetch_one(pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn delete_rule(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db();
    let r = sqlx::query(
        "DELETE FROM mail_sweep_rules WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(MailError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Spawn the background sweep worker: every `interval_secs`, apply all enabled
/// rules. Each rule moves matching messages into its target folder's mailbox in
/// one UPDATE (no-op when the target folder doesn't exist for that user).
pub fn spawn_worker(pool: expresso_core::DbPool, interval_secs: u64) {
    let secs = interval_secs.max(30);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(secs));
        loop {
            tick.tick().await;
            match run_sweep(&pool).await {
                Ok(n) if n > 0 => tracing::info!(moved = n, "sweep cycle moved messages"),
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "sweep cycle failed"),
            }
        }
    });
}

/// Apply every enabled rule once; returns the total messages moved. Each rule's
/// move targets only the owning user's mailboxes (RLS-safe via explicit
/// tenant/user predicates) and skips messages already in the target folder.
async fn run_sweep(pool: &expresso_core::DbPool) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "UPDATE messages m \
            SET mailbox_id = tgt.id \
         FROM mail_sweep_rules r \
         JOIN mailboxes tgt \
              ON tgt.tenant_id = r.tenant_id AND tgt.user_id = r.user_id \
             AND tgt.folder_name = r.target_folder \
         JOIN mailboxes src \
              ON src.id = m.mailbox_id \
             AND src.tenant_id = r.tenant_id AND src.user_id = r.user_id \
         WHERE r.enabled \
           AND lower(m.from_addr) = r.sender_address \
           AND m.received_at < now() - make_interval(days => r.older_than_days) \
           AND m.mailbox_id <> tgt.id",
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        assert_eq!(default_days(), 7);
        assert_eq!(default_folder(), "Trash");
    }
}

//! IMAP mailbox/folder management endpoints.
//!
//! Tenant scoping: `list_folders` abre transação via `begin_tenant_tx` para
//! defense-in-depth — o SELECT usa `WHERE tenant_id = $1 AND user_id = $2`
//! explícitos, e RLS de `mailboxes` filtra junto. Sem essa combinação o
//! endpoint vazava mailboxes de todos os tenants (RLS no schema é NULL-bypass).

use axum::{Router, routing::get, extract::State, Json};
use expresso_core::begin_tenant_tx;
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{api::context::RequestCtx, error::Result, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/folders", get(list_folders))
}

#[derive(Debug, Serialize, FromRow)]
pub struct FolderDto {
    pub id:            Uuid,
    pub name:          String,
    pub special_use:   Option<String>,
    pub message_count: i32,
    pub unseen_count:  i32,
    pub subscribed:    bool,
}

/// GET /api/v1/mail/folders
async fn list_folders(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<FolderDto>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows: Vec<FolderDto> = sqlx::query_as(
        r#"
        SELECT
            id,
            folder_name AS name,
            special_use,
            message_count,
            unseen_count,
            subscribed
        FROM mailboxes
        WHERE tenant_id = $1
          AND user_id   = $2
          AND subscribed = true
        ORDER BY
            CASE special_use
                WHEN '\Inbox'  THEN 0
                WHEN '\Sent'   THEN 1
                WHEN '\Drafts' THEN 2
                WHEN '\Trash'  THEN 3
                WHEN '\Junk'   THEN 4
                ELSE 10
            END,
            folder_name
        "#
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(Json(rows))
}

//! IMAP mailbox/folder management endpoints

use axum::{Router, routing::get, extract::State, Json};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{error::Result, state::AppState};

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
) -> Result<Json<Vec<FolderDto>>> {
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
        WHERE subscribed = true
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
    .fetch_all(state.db())
    .await
    .map_err(expresso_core::CoreError::Database)?;

    Ok(Json(rows))
}

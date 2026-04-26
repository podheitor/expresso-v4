//! IMAP mailbox/folder management endpoints.
//!
//! Tenant scoping: `list_folders` abre transação via `begin_tenant_tx` para
//! defense-in-depth — o SELECT usa `WHERE tenant_id = $1 AND user_id = $2`
//! explícitos, e RLS de `mailboxes` filtra junto. Sem essa combinação o
//! endpoint vazava mailboxes de todos os tenants (RLS no schema é NULL-bypass).

use axum::{Router, routing::get, extract::{State, Path}, Json, http::StatusCode};
use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{api::context::RequestCtx, error::{MailError, Result}, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/folders",         get(list_folders).post(create_folder))
        .route("/mail/folders/:name",   axum::routing::patch(rename_folder).delete(delete_folder))
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

#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct RenameFolderRequest {
    pub name: String,
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

/// POST /api/v1/mail/folders — create a new folder
async fn create_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<CreateFolderRequest>,
) -> Result<(StatusCode, Json<FolderDto>)> {
    validate_folder_name(&body.name)?;

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mailboxes WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&body.name)
    .fetch_optional(&mut *tx)
    .await?;

    if existing.is_some() {
        return Err(MailError::BadRequest(format!("folder '{}' already exists", body.name)));
    }

    let row: FolderDto = sqlx::query_as(
        r#"INSERT INTO mailboxes
               (user_id, tenant_id, folder_name, uid_validity, next_uid, subscribed)
           VALUES ($1, $2, $3, EXTRACT(EPOCH FROM now())::BIGINT, 1, true)
           RETURNING
               id,
               folder_name AS name,
               special_use,
               message_count,
               unseen_count,
               subscribed"#,
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&body.name)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// PATCH /api/v1/mail/folders/:name — rename folder
async fn rename_folder(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Path(old_name): Path<String>,
    Json(body):    Json<RenameFolderRequest>,
) -> Result<Json<FolderDto>> {
    validate_folder_name(&body.name)?;

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Protect system folders from rename.
    let special: Option<String> = sqlx::query_scalar(
        "SELECT special_use FROM mailboxes WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&old_name)
    .fetch_optional(&mut *tx)
    .await?;

    match special {
        None => return Err(MailError::FolderNotFound { folder: old_name }),
        Some(Some(_)) => return Err(MailError::BadRequest("cannot rename a system folder".into())),
        Some(None) => {}
    }

    let row: Option<FolderDto> = sqlx::query_as(
        r#"UPDATE mailboxes
           SET folder_name = $1, updated_at = now()
           WHERE user_id = $2 AND tenant_id = $3 AND folder_name = $4
           RETURNING
               id,
               folder_name AS name,
               special_use,
               message_count,
               unseen_count,
               subscribed"#,
    )
    .bind(&body.name)
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&old_name)
    .fetch_optional(&mut *tx)
    .await?;

    tx.commit().await?;
    row.map(Json).ok_or(MailError::FolderNotFound { folder: old_name })
}

/// DELETE /api/v1/mail/folders/:name — delete folder and all its messages
async fn delete_folder(
    State(state):   State<AppState>,
    ctx:            RequestCtx,
    Path(name):     Path<String>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let special: Option<String> = sqlx::query_scalar(
        "SELECT special_use FROM mailboxes WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?;

    match special {
        None => return Err(MailError::FolderNotFound { folder: name }),
        Some(Some(_)) => return Err(MailError::BadRequest("cannot delete a system folder".into())),
        Some(None) => {}
    }

    // messages.mailbox_id has ON DELETE CASCADE, so deleting the mailbox row
    // removes all contained messages automatically.
    sqlx::query(
        "DELETE FROM mailboxes WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&name)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reject names that would confuse IMAP hierarchy or SQL injection via folder_name
/// interpolation in legacy code paths. Keeps folders safe for IMAP LIST patterns.
fn validate_folder_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 200 {
        return Err(MailError::BadRequest("folder name must be 1–200 chars".into()));
    }
    if name.contains('\0') || name.contains('\r') || name.contains('\n') {
        return Err(MailError::BadRequest("folder name contains invalid characters".into()));
    }
    Ok(())
}

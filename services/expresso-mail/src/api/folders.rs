//! IMAP mailbox/folder management endpoints.
//!
//! Tenant scoping: `list_folders` abre transação via `begin_tenant_tx` para
//! defense-in-depth — o SELECT usa `WHERE tenant_id = $1 AND user_id = $2`
//! explícitos, e RLS de `mailboxes` filtra junto. Sem essa combinação o
//! endpoint vazava mailboxes de todos os tenants (RLS no schema é NULL-bypass).

use axum::{Router, routing::get, extract::{State, Path}, http::{header, HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Response}, Json};
use time::OffsetDateTime;
use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::{api::context::RequestCtx, error::{MailError, Result}, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/folders",                    get(list_folders).post(create_folder))
        .route("/mail/folders/all",                get(list_all_folders))
        .route("/mail/folders/unread-summary",     get(unread_summary))
        .route("/mail/folders/:name",              axum::routing::patch(rename_folder).delete(delete_folder))
        .route("/mail/folders/:name/mark-read",    axum::routing::post(mark_folder_read))
        .route("/mail/folders/:name/subscribe",    axum::routing::post(subscribe_folder))
        .route("/mail/folders/:name/unsubscribe",  axum::routing::post(unsubscribe_folder))
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
    req_headers:  HeaderMap,
) -> Result<Response> {
    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM mailboxes WHERE tenant_id = $1 AND user_id = $2 AND subscribed = true",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mailboxes WHERE tenant_id = $1 AND user_id = $2 AND subscribed = true",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;
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

    let mut resp = (
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
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

/// GET /api/v1/mail/folders/all — list ALL folders including unsubscribed ones
async fn list_all_folders(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM mailboxes WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mailboxes WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;
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

    let mut resp = (
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// POST /api/v1/mail/folders/:name/subscribe — mark folder as subscribed
async fn subscribe_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(name):   Path<String>,
) -> Result<StatusCode> {
    set_subscribed(&state, &ctx, &name, true).await
}

/// POST /api/v1/mail/folders/:name/unsubscribe — mark folder as unsubscribed
async fn unsubscribe_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(name):   Path<String>,
) -> Result<StatusCode> {
    set_subscribed(&state, &ctx, &name, false).await
}

async fn set_subscribed(
    state:      &AppState,
    ctx:        &RequestCtx,
    name:       &str,
    subscribed: bool,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let affected = sqlx::query(
        "UPDATE mailboxes SET subscribed = $1, updated_at = now() \
         WHERE user_id = $2 AND tenant_id = $3 AND folder_name = $4",
    )
    .bind(subscribed)
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(name)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;

    if affected == 0 {
        Err(MailError::FolderNotFound { folder: name.to_string() })
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

/// POST /api/v1/mail/folders/:name/mark-read — mark all messages in folder as \Seen
///
/// Returns `{"marked": N}` — number of messages that had the flag added.
async fn mark_folder_read(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(name):   Path<String>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let mbox_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mailboxes WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?;

    let mbox_id = mbox_id.ok_or(MailError::FolderNotFound { folder: name })?;

    let res = sqlx::query(
        r#"UPDATE messages
           SET flags = array_append(flags, $1)
           WHERE mailbox_id = $2
             AND tenant_id  = $3
             AND NOT ($1 = ANY(flags))"#,
    )
    .bind(r"\Seen")
    .bind(mbox_id)
    .bind(ctx.tenant_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Json(serde_json::json!({ "marked": res.rows_affected() })))
}

/// GET /api/v1/mail/folders/unread-summary — live unread count per folder (not cached).
///
/// Returns `[{"folder": "INBOX", "unread": 5}, …]` computed via COUNT at query time.
/// Use this when you need accurate counts; `GET /mail/folders` returns the cached `unseen_count`.
async fn unread_summary(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<serde_json::Value>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT mb.folder_name,
               COUNT(m.id) FILTER (WHERE NOT ('\Seen' = ANY(m.flags))) AS unread
        FROM mailboxes mb
        LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1
        WHERE mb.tenant_id = $1
          AND mb.user_id   = $2
        GROUP BY mb.folder_name
        ORDER BY mb.folder_name
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let result = rows.into_iter()
        .map(|(folder, unread)| serde_json::json!({"folder": folder, "unread": unread}))
        .collect();
    Ok(Json(result))
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

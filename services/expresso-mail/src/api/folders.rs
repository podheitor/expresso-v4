//! IMAP mailbox/folder management endpoints.
//!
//! Tenant scoping: `list_folders` abre transação via `begin_tenant_tx` para
//! defense-in-depth — o SELECT usa `WHERE tenant_id = $1 AND user_id = $2`
//! explícitos, e RLS de `mailboxes` filtra junto. Sem essa combinação o
//! endpoint vazava mailboxes de todos os tenants (RLS no schema é NULL-bypass).

use axum::{Router, routing::get, extract::{State, Path, Query}, http::{header, HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Response}, Json};
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
        .route("/mail/folders/stats",              get(folders_stats))
        .route("/mail/folders/size-summary",       get(folders_size_summary))
        .route("/mail/folders/special-use/empty",       axum::routing::post(empty_special_use_folders_bulk))
        .route("/mail/folders/special-use/:slot/empty", axum::routing::post(empty_special_use_folder))
        .route("/mail/folders/special-use/mark-unread", axum::routing::post(mark_unread_special_use_folders_bulk))
        .route("/mail/folders/rename-history",     get(list_folder_rename_history))
        .route("/mail/folders/rename-history/revert-all", axum::routing::post(revert_all_folder_renames))
        .route("/mail/folders/rename-history/by-mailbox/:mailbox_id/undo", axum::routing::post(undo_folder_rename_by_mailbox))
        .route("/mail/folders/rename-history/:id/undo", axum::routing::post(undo_folder_rename))
        .route("/mail/folders/:id/stats",          get(folder_stats_by_id))
        .route("/mail/folders/:name",              axum::routing::patch(rename_folder).delete(delete_folder))
        .route("/mail/folders/:name/mark-read",    axum::routing::post(mark_folder_read))
        .route("/mail/folders/:name/mark-unread",  axum::routing::post(mark_folder_unread))
        .route("/mail/folders/:name/empty",        axum::routing::post(empty_folder))
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
///
/// Sprint #480: além do UPDATE, grava INSERT em mail_folder_rename_history na
/// mesma tx (begin_tenant_tx) — atomicidade garantida; se history falhar, todo
/// o rename roda rollback. Habilita audit trail e UI tipo "histórico de
/// renames" via GET /api/v1/mail/folders/rename-history.
async fn rename_folder(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Path(old_name): Path<String>,
    Json(body):    Json<RenameFolderRequest>,
) -> Result<Json<FolderDto>> {
    validate_folder_name(&body.name)?;

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Protect system folders from rename.
    let lookup: Option<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT id, special_use FROM mailboxes WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&old_name)
    .fetch_optional(&mut *tx)
    .await?;

    let mailbox_id = match lookup {
        None => return Err(MailError::FolderNotFound { folder: old_name }),
        Some((_, Some(_))) => return Err(MailError::BadRequest("cannot rename a system folder".into())),
        Some((id, None)) => id,
    };

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

    let dto = row.ok_or_else(|| MailError::FolderNotFound { folder: old_name.clone() })?;

    sqlx::query(
        "INSERT INTO mail_folder_rename_history \
            (tenant_id, user_id, mailbox_id, old_name, new_name, renamed_by) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(mailbox_id)
    .bind(&old_name)
    .bind(&body.name)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Json(dto))
}

#[derive(Debug, serde::Deserialize)]
struct FolderRenameHistoryQuery {
    limit:      Option<i64>,
    since:      Option<time::OffsetDateTime>,
    before:     Option<time::OffsetDateTime>,
    name:       Option<String>,
    mailbox_id: Option<Uuid>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct FolderRenameHistoryEntry {
    id:         Uuid,
    mailbox_id: Uuid,
    old_name:   String,
    new_name:   String,
    renamed_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    renamed_at: time::OffsetDateTime,
}

/// GET /api/v1/mail/folders/rename-history?limit=&since=&before=&name=&mailbox_id=
/// — audit trail dos renames de folder do user (sprint #480). Filtros: range
/// temporal, `name` matching old_name OR new_name (literal), `mailbox_id` pra
/// limitar a uma única mailbox (rastreia toda a história de renames daquela
/// mailbox). Limit padrão 50, cap 1..500. Path estático precede `/:name`
/// porque axum prefere static sobre `:capture` (lição #443/#448).
async fn list_folder_rename_history(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<FolderRenameHistoryQuery>,
) -> Result<Json<serde_json::Value>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);

    let entries: Vec<FolderRenameHistoryEntry> = sqlx::query_as(
        "SELECT id, mailbox_id, old_name, new_name, renamed_by, renamed_at \
           FROM mail_folder_rename_history \
          WHERE tenant_id = $1 AND user_id = $2 \
            AND ($3::timestamptz IS NULL OR renamed_at >= $3) \
            AND ($4::timestamptz IS NULL OR renamed_at <  $4) \
            AND ($5::text IS NULL OR old_name = $5 OR new_name = $5) \
            AND ($6::uuid IS NULL OR mailbox_id = $6) \
          ORDER BY renamed_at DESC \
          LIMIT $7",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(q.since)
    .bind(q.before)
    .bind(q.name)
    .bind(q.mailbox_id)
    .bind(limit)
    .fetch_all(state.db())
    .await?;

    Ok(Json(serde_json::json!({ "limit": limit, "entries": entries })))
}

/// POST /api/v1/mail/folders/rename-history/:id/undo — reverte um rename
/// específico (sprint #481). Lê entry da `mail_folder_rename_history` filtrando
/// por tenant_id+user_id, localiza a mailbox por `mailbox_id` (UUID estável
/// across renames), valida que `folder_name` atual == `new_name` da entry
/// (idempotência: 409 se já foi renomeada de novo). Aplica UPDATE inverso pra
/// `old_name` e grava nova linha de history com old/new invertidos (audit
/// trail completo do undo). Tudo em `begin_tenant_tx`. 404 se entry não
/// pertence ao user. System folders nunca aparecem no history (rename bloqueia
/// special_use IS NOT NULL), então não há check redundante.
async fn undo_folder_rename(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let entry: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT mailbox_id, old_name, new_name \
           FROM mail_folder_rename_history \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (mailbox_id, old_name, new_name) = entry.ok_or_else(|| MailError::FolderNotFound {
        folder: format!("rename-history:{id}"),
    })?;

    let current: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT folder_name, special_use FROM mailboxes \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(mailbox_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let current_name = match current {
        None => return Err(MailError::FolderNotFound { folder: format!("mailbox:{mailbox_id}") }),
        Some((_, Some(_))) => return Err(MailError::BadRequest("cannot undo rename of system folder".into())),
        Some((name, None)) => name,
    };

    if current_name != new_name {
        return Err(MailError::Conflict(format!(
            "folder current name '{current_name}' differs from history new_name '{new_name}'"
        )));
    }

    let conflict: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mailboxes \
          WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3 AND id <> $4",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&old_name)
    .bind(mailbox_id)
    .fetch_optional(&mut *tx)
    .await?;
    if conflict.is_some() {
        return Err(MailError::Conflict(format!(
            "folder '{old_name}' already exists; cannot undo rename"
        )));
    }

    sqlx::query(
        "UPDATE mailboxes SET folder_name = $1, updated_at = now() \
          WHERE id = $2 AND tenant_id = $3 AND user_id = $4",
    )
    .bind(&old_name)
    .bind(mailbox_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;

    let new_history_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mail_folder_rename_history \
            (tenant_id, user_id, mailbox_id, old_name, new_name, renamed_by) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(mailbox_id)
    .bind(&new_name)
    .bind(&old_name)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "undone_id":      id,
        "mailbox_id":     mailbox_id,
        "reverted_from":  new_name,
        "reverted_to":    old_name,
        "history_id":     new_history_id,
    })))
}

/// POST /api/v1/mail/folders/rename-history/by-mailbox/:mailbox_id/undo —
/// variante granular do revert-all: desfaz o rename MAIS RECENTE de uma
/// mailbox específica (sprint #568). Paralelo do #490 mas single-mailbox;
/// paralelo do #481 mas por mailbox_id em vez de history entry id.
/// Mesmas validações do #481: sistema-folder 400, not-found 404,
/// nome-atual-diferente 409, conflito-de-nome 409. Atomicidade via begin_tenant_tx.
/// Útil pra UX "desfazer último rename desta pasta" sem listar history e
/// escolher o id manualmente.
async fn undo_folder_rename_by_mailbox(
    State(state):    State<AppState>,
    ctx:             RequestCtx,
    Path(mailbox_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Fetch most recent rename history entry for this mailbox.
    let entry: Option<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, old_name, new_name \
           FROM mail_folder_rename_history \
          WHERE mailbox_id = $1 AND tenant_id = $2 AND user_id = $3 \
          ORDER BY renamed_at DESC \
          LIMIT 1",
    )
    .bind(mailbox_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (entry_id, old_name, new_name) = entry.ok_or_else(|| MailError::FolderNotFound {
        folder: format!("rename-history for mailbox:{mailbox_id}"),
    })?;

    let current: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT folder_name, special_use FROM mailboxes \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(mailbox_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let current_name = match current {
        None => return Err(MailError::FolderNotFound { folder: format!("mailbox:{mailbox_id}") }),
        Some((_, Some(_))) => return Err(MailError::BadRequest("cannot undo rename of system folder".into())),
        Some((name, None)) => name,
    };

    if current_name != new_name {
        return Err(MailError::Conflict(format!(
            "folder current name '{current_name}' differs from history new_name '{new_name}'"
        )));
    }

    let conflict: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mailboxes \
          WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3 AND id <> $4",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(&old_name)
    .bind(mailbox_id)
    .fetch_optional(&mut *tx)
    .await?;
    if conflict.is_some() {
        return Err(MailError::Conflict(format!(
            "folder '{old_name}' already exists; cannot undo rename"
        )));
    }

    sqlx::query(
        "UPDATE mailboxes SET folder_name = $1, updated_at = now() \
          WHERE id = $2 AND tenant_id = $3 AND user_id = $4",
    )
    .bind(&old_name)
    .bind(mailbox_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;

    let new_history_id: Uuid = sqlx::query_scalar(
        "INSERT INTO mail_folder_rename_history \
            (tenant_id, user_id, mailbox_id, old_name, new_name, renamed_by) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(mailbox_id)
    .bind(&new_name)
    .bind(&old_name)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "undone_id":      entry_id,
        "mailbox_id":     mailbox_id,
        "reverted_from":  new_name,
        "reverted_to":    old_name,
        "history_id":     new_history_id,
    })))
}

#[derive(Debug, Deserialize)]
struct RevertAllQuery {
    n:   Option<i64>,
    /// `?dry=true` retorna o plano sem aplicar (sprint #494). Default false.
    dry: Option<bool>,
}

/// POST /api/v1/mail/folders/rename-history/revert-all?n=N — UX simplificada
/// por cima do undo single (#481): pega o rename mais recente de cada uma das
/// últimas N mailboxes (default 1, cap 1..50) e aplica undo em todas numa
/// única transação atômica via begin_tenant_tx (sprint #490). Atomicidade
/// total: se uma reversion falhar (conflict de nome no destino, mailbox
/// removida, etc), TODA a operação roda rollback — cliente vê resultado
/// "tudo ou nada". Pula silenciosamente entries cujo `current_name` já
/// difere de `new_name` (renomeada de novo após a entry de history) — não é
/// erro porque o estado mudou desde então; reportado como skipped no
/// response. Retorna `{requested:N, reverted:[{history_id, mailbox_id,
/// from, to, new_history_id}], skipped:[{history_id, mailbox_id, reason}]}`.
/// Útil pra "desfazer rename em massa" depois de uma operação bulk
/// experimental. DISTINCT ON garante 1 entry por mailbox (mais recente).
async fn revert_all_folder_renames(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<RevertAllQuery>,
) -> Result<Json<serde_json::Value>> {
    let n = q.n.unwrap_or(1).clamp(1, 50);
    let dry = q.dry.unwrap_or(false);

    if dry {
        return revert_all_dry_run(&state, &ctx, n).await;
    }

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let entries: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (mailbox_id) id, mailbox_id, old_name, new_name \
           FROM mail_folder_rename_history \
          WHERE tenant_id = $1 AND user_id = $2 \
          ORDER BY mailbox_id, renamed_at DESC \
          LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(n)
    .fetch_all(&mut *tx)
    .await?;

    let mut reverted: Vec<serde_json::Value> = Vec::new();
    let mut skipped:  Vec<serde_json::Value> = Vec::new();

    for (entry_id, mailbox_id, old_name, new_name) in entries {
        let current: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT folder_name, special_use FROM mailboxes \
              WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
        )
        .bind(mailbox_id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_optional(&mut *tx)
        .await?;

        let current_name = match current {
            None => {
                skipped.push(serde_json::json!({
                    "history_id": entry_id, "mailbox_id": mailbox_id,
                    "reason":     "mailbox no longer exists",
                }));
                continue;
            }
            Some((_, Some(_))) => {
                skipped.push(serde_json::json!({
                    "history_id": entry_id, "mailbox_id": mailbox_id,
                    "reason":     "mailbox is now system folder",
                }));
                continue;
            }
            Some((name, None)) => name,
        };

        if current_name != new_name {
            skipped.push(serde_json::json!({
                "history_id":   entry_id,
                "mailbox_id":   mailbox_id,
                "current_name": current_name,
                "expected":     new_name,
                "reason":       "current name differs from history new_name",
            }));
            continue;
        }

        let conflict: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM mailboxes \
              WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3 AND id <> $4",
        )
        .bind(ctx.user_id)
        .bind(ctx.tenant_id)
        .bind(&old_name)
        .bind(mailbox_id)
        .fetch_optional(&mut *tx)
        .await?;
        if conflict.is_some() {
            return Err(MailError::Conflict(format!(
                "folder '{old_name}' already exists; cannot revert mailbox {mailbox_id}"
            )));
        }

        sqlx::query(
            "UPDATE mailboxes SET folder_name = $1, updated_at = now() \
              WHERE id = $2 AND tenant_id = $3 AND user_id = $4",
        )
        .bind(&old_name)
        .bind(mailbox_id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await?;

        let new_history_id: Uuid = sqlx::query_scalar(
            "INSERT INTO mail_folder_rename_history \
                (tenant_id, user_id, mailbox_id, old_name, new_name, renamed_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .bind(mailbox_id)
        .bind(&new_name)
        .bind(&old_name)
        .bind(ctx.user_id)
        .fetch_one(&mut *tx)
        .await?;

        reverted.push(serde_json::json!({
            "history_id":     entry_id,
            "mailbox_id":     mailbox_id,
            "from":           new_name,
            "to":             old_name,
            "new_history_id": new_history_id,
        }));
    }

    tx.commit().await?;
    Ok(Json(serde_json::json!({
        "requested": n,
        "reverted":  reverted,
        "skipped":   skipped,
    })))
}

/// Dry-run de `revert_all_folder_renames` (sprint #494). Mesmos checks
/// (mailbox exists, special_use, current_name match, conflict pre-check)
/// mas só com SELECT — nenhum UPDATE/INSERT, nenhuma transação. Retorna
/// `{dry:true, requested, planned:[...], skipped:[...], conflicts:[...]}`
/// onde `planned` lista revertions que SERIAM aplicadas, `skipped` os
/// pulados pelo mesmo critério do real, `conflicts` os que abortariam o tx
/// real (no real é um erro 409; no dry é um item informativo). Útil pra
/// preview antes de "Confirm" no UI bulk.
async fn revert_all_dry_run(
    state: &AppState,
    ctx:   &RequestCtx,
    n:     i64,
) -> Result<Json<serde_json::Value>> {
    let entries: Vec<(Uuid, Uuid, String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (mailbox_id) id, mailbox_id, old_name, new_name \
           FROM mail_folder_rename_history \
          WHERE tenant_id = $1 AND user_id = $2 \
          ORDER BY mailbox_id, renamed_at DESC \
          LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(n)
    .fetch_all(state.db())
    .await?;

    let mut planned:   Vec<serde_json::Value> = Vec::new();
    let mut skipped:   Vec<serde_json::Value> = Vec::new();
    let mut conflicts: Vec<serde_json::Value> = Vec::new();

    for (entry_id, mailbox_id, old_name, new_name) in entries {
        let current: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT folder_name, special_use FROM mailboxes \
              WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
        )
        .bind(mailbox_id)
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .fetch_optional(state.db())
        .await?;

        let current_name = match current {
            None => {
                skipped.push(serde_json::json!({
                    "history_id": entry_id, "mailbox_id": mailbox_id,
                    "reason":     "mailbox no longer exists",
                }));
                continue;
            }
            Some((_, Some(_))) => {
                skipped.push(serde_json::json!({
                    "history_id": entry_id, "mailbox_id": mailbox_id,
                    "reason":     "mailbox is now system folder",
                }));
                continue;
            }
            Some((name, None)) => name,
        };

        if current_name != new_name {
            skipped.push(serde_json::json!({
                "history_id":   entry_id,
                "mailbox_id":   mailbox_id,
                "current_name": current_name,
                "expected":     new_name,
                "reason":       "current name differs from history new_name",
            }));
            continue;
        }

        let conflict: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM mailboxes \
              WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3 AND id <> $4",
        )
        .bind(ctx.user_id)
        .bind(ctx.tenant_id)
        .bind(&old_name)
        .bind(mailbox_id)
        .fetch_optional(state.db())
        .await?;
        if let Some(other_id) = conflict {
            conflicts.push(serde_json::json!({
                "history_id":   entry_id,
                "mailbox_id":   mailbox_id,
                "from":         new_name,
                "to":           old_name,
                "conflict_with": other_id,
                "reason":       "destination folder name already exists",
            }));
            continue;
        }

        planned.push(serde_json::json!({
            "history_id": entry_id,
            "mailbox_id": mailbox_id,
            "from":       new_name,
            "to":         old_name,
        }));
    }

    Ok(Json(serde_json::json!({
        "dry":       true,
        "requested": n,
        "planned":   planned,
        "skipped":   skipped,
        "conflicts": conflicts,
    })))
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

/// POST /api/v1/mail/folders/:name/mark-unread — inverso do mark-read (sprint
/// #485). Remove `\Seen` de todas as messages na folder via
/// `array_remove(flags, '\Seen')`, condicionando ao predicate `'\Seen' =
/// ANY(flags)` pra contabilizar só as que de fato perdem o flag (idempotente:
/// já-não-lidas retornam `unmarked: 0`). Útil pra "marcar pasta inteira como
/// nova" (escape de notification fadigue ou pra forçar re-triagem). 404 se a
/// pasta não pertence ao user.
async fn mark_folder_unread(
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
           SET flags = array_remove(flags, $1)
           WHERE mailbox_id = $2
             AND tenant_id  = $3
             AND $1 = ANY(flags)"#,
    )
    .bind(r"\Seen")
    .bind(mbox_id)
    .bind(ctx.tenant_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Json(serde_json::json!({ "unmarked": res.rows_affected() })))
}

/// POST /api/v1/mail/folders/:name/empty — delete ALL messages in a folder, but
/// preserve the mailbox row itself (sprint #459). Útil pra esvaziar Trash/Spam
/// pós-purge ou limpar pasta de teste sem perder a configuração da mailbox
/// (subscribed, special_use, uid_validity). Retorna `{deleted: N}`. 404 se a
/// pasta não existe pro user. Diferente de `delete_folder` (que remove o
/// mailbox e tudo via CASCADE) — esvaziamento é não-destrutivo pra pastas
/// de sistema (`special_use` set), por isso não bloqueia em \Trash/\Junk
/// como delete_folder bloqueia em qualquer system folder.
async fn empty_folder(
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
        r#"DELETE FROM messages
           WHERE mailbox_id = $1
             AND tenant_id  = $2"#,
    )
    .bind(mbox_id)
    .bind(ctx.tenant_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Json(serde_json::json!({ "deleted": res.rows_affected() })))
}

/// POST /api/v1/mail/folders/special-use/:slot/empty — esvazia a pasta cujo
/// `special_use` corresponde ao slot RFC 6154, sem hardcode do nome local
/// (sprint #468). Slots aceitos: `trash` → `\Trash`, `junk` → `\Junk`,
/// `drafts` → `\Drafts`, `sent` → `\Sent`. Útil pra UI tipo "Esvaziar Lixeira"
/// que funciona independente do label local da pasta. Variante de #459 mas
/// localizada pelo papel (`special_use`) em vez de pelo nome. 400 se slot
/// desconhecido, 404 se o user não tem mailbox marcada com aquele
/// special_use. Idempotente: pasta já vazia retorna `{deleted: 0}`.
async fn empty_special_use_folder(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(slot):   Path<String>,
) -> Result<Json<serde_json::Value>> {
    let special_use = match slot.to_lowercase().as_str() {
        "trash"  => "\\Trash",
        "junk"   => "\\Junk",
        "drafts" => "\\Drafts",
        "sent"   => "\\Sent",
        _ => return Err(MailError::BadRequest(
            "slot must be one of: trash, junk, drafts, sent".into()
        )),
    };

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let mbox: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, folder_name FROM mailboxes \
         WHERE user_id = $1 AND tenant_id = $2 AND special_use = $3",
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(special_use)
    .fetch_optional(&mut *tx)
    .await?;

    let (mbox_id, folder_name) = mbox.ok_or_else(|| MailError::FolderNotFound {
        folder: format!("special-use:{slot}"),
    })?;

    let res = sqlx::query(
        r#"DELETE FROM messages
           WHERE mailbox_id = $1
             AND tenant_id  = $2"#,
    )
    .bind(mbox_id)
    .bind(ctx.tenant_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Json(serde_json::json!({
        "slot":        slot,
        "special_use": special_use,
        "folder":      folder_name,
        "deleted":     res.rows_affected(),
    })))
}

#[derive(Debug, Deserialize)]
struct EmptyBulkQuery {
    slots: String,
}

/// POST /api/v1/mail/folders/special-use/empty?slots=trash,junk — bulk variant
/// of `empty_special_use_folder` (sprint #473). Esvazia múltiplas mailboxes
/// identificadas pelo papel RFC 6154 numa única transação atômica via
/// `begin_tenant_tx` (RLS). Slots aceitos: `trash`, `junk`, `drafts`, `sent`
/// (case-insensitive, deduplicated, 1..4 entries). 400 se slot desconhecido.
/// Slots sem mailbox correspondente são silenciosamente ignorados (idempotente
/// — útil pra UI tipo "Limpar Lixeira+Spam" que não precisa saber se o user já
/// tem cada pasta criada). Retorna `[{slot, special_use, folder, deleted}]` só
/// para os slots que de fato mapearam pra mailboxes existentes.
async fn empty_special_use_folders_bulk(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<EmptyBulkQuery>,
) -> Result<Json<serde_json::Value>> {
    use std::collections::BTreeSet;

    let slots: BTreeSet<String> = q
        .slots
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if slots.is_empty() || slots.len() > 4 {
        return Err(MailError::BadRequest(
            "slots must be 1..4 entries".into(),
        ));
    }

    let mut mapped: Vec<(String, &'static str)> = Vec::with_capacity(slots.len());
    for s in &slots {
        let su = match s.as_str() {
            "trash"  => "\\Trash",
            "junk"   => "\\Junk",
            "drafts" => "\\Drafts",
            "sent"   => "\\Sent",
            _ => return Err(MailError::BadRequest(format!(
                "unknown slot '{s}': must be one of trash, junk, drafts, sent"
            ))),
        };
        mapped.push((s.clone(), su));
    }

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(mapped.len());

    for (slot, special_use) in mapped {
        let mbox: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, folder_name FROM mailboxes \
             WHERE user_id = $1 AND tenant_id = $2 AND special_use = $3",
        )
        .bind(ctx.user_id)
        .bind(ctx.tenant_id)
        .bind(special_use)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((mbox_id, folder_name)) = mbox else { continue };

        let res = sqlx::query(
            r#"DELETE FROM messages
               WHERE mailbox_id = $1
                 AND tenant_id  = $2"#,
        )
        .bind(mbox_id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await?;

        results.push(serde_json::json!({
            "slot":        slot,
            "special_use": special_use,
            "folder":      folder_name,
            "deleted":     res.rows_affected(),
        }));
    }

    tx.commit().await?;
    Ok(Json(serde_json::json!({ "results": results })))
}

/// POST /api/v1/mail/folders/special-use/mark-unread?slots=trash,junk — bulk
/// variant de mark-unread (sprint #487) que opera por papel RFC 6154 em vez de
/// nome local. Combina #485 (mark-unread single) com #473 (special-use empty
/// bulk): identifica mailboxes pelo `special_use` e roda
/// `array_remove(flags, '\Seen')` condicionado a `'\Seen' = ANY(flags)` numa
/// única tx atômica via `begin_tenant_tx` (RLS). Slots aceitos: `trash`,
/// `junk`, `drafts`, `sent` (case-insensitive, deduped, 1..4 entries). 400 se
/// slot desconhecido. Slots sem mailbox correspondente são silenciosamente
/// ignorados (idempotente). Retorna `[{slot, special_use, folder, unmarked}]`
/// só para os slots que mapearam pra mailboxes existentes. Útil pra UX
/// "marcar Trash+Junk como não lidos" sem precisar saber labels locais.
async fn mark_unread_special_use_folders_bulk(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<EmptyBulkQuery>,
) -> Result<Json<serde_json::Value>> {
    use std::collections::BTreeSet;

    let slots: BTreeSet<String> = q
        .slots
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if slots.is_empty() || slots.len() > 4 {
        return Err(MailError::BadRequest("slots must be 1..4 entries".into()));
    }

    let mut mapped: Vec<(String, &'static str)> = Vec::with_capacity(slots.len());
    for s in &slots {
        let su = match s.as_str() {
            "trash"  => "\\Trash",
            "junk"   => "\\Junk",
            "drafts" => "\\Drafts",
            "sent"   => "\\Sent",
            _ => return Err(MailError::BadRequest(format!(
                "unknown slot '{s}': must be one of trash, junk, drafts, sent"
            ))),
        };
        mapped.push((s.clone(), su));
    }

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(mapped.len());

    for (slot, special_use) in mapped {
        let mbox: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, folder_name FROM mailboxes \
             WHERE user_id = $1 AND tenant_id = $2 AND special_use = $3",
        )
        .bind(ctx.user_id)
        .bind(ctx.tenant_id)
        .bind(special_use)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((mbox_id, folder_name)) = mbox else { continue };

        let res = sqlx::query(
            r#"UPDATE messages
               SET flags = array_remove(flags, $1)
               WHERE mailbox_id = $2
                 AND tenant_id  = $3
                 AND $1 = ANY(flags)"#,
        )
        .bind(r"\Seen")
        .bind(mbox_id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await?;

        results.push(serde_json::json!({
            "slot":        slot,
            "special_use": special_use,
            "folder":      folder_name,
            "unmarked":    res.rows_affected(),
        }));
    }

    tx.commit().await?;
    Ok(Json(serde_json::json!({ "results": results })))
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

/// GET /api/v1/mail/folders/stats — agregados por mailbox (sprint #454).
/// Retorna `[{folder, special_use, total, unread, size_bytes}]` calculado live
/// (não usa cache `mailboxes.message_count`/`unseen_count`). Útil pra dashboard
/// "ocupação por pasta" — top 10 pastas pesando mais bytes, distribuição de
/// não-lidas, etc. Path estático `/folders/stats` precede `/folders/:name` por
/// preferência axum (lição #443/#448), sem necessidade de hífen.
async fn folders_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<serde_json::Value>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows: Vec<(String, Option<String>, i64, i64, i64)> = sqlx::query_as(
        r#"
        SELECT mb.folder_name,
               mb.special_use,
               COUNT(m.id)                                                         AS total,
               COUNT(m.id) FILTER (WHERE NOT ('\Seen' = ANY(m.flags)))             AS unread,
               COALESCE(SUM(m.size_bytes)::bigint, 0)                              AS size_bytes
        FROM mailboxes mb
        LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1
        WHERE mb.tenant_id = $1
          AND mb.user_id   = $2
        GROUP BY mb.folder_name, mb.special_use
        ORDER BY mb.folder_name
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let result = rows.into_iter()
        .map(|(folder, special_use, total, unread, size_bytes)| serde_json::json!({
            "folder":      folder,
            "special_use": special_use,
            "total":       total,
            "unread":      unread,
            "size_bytes":  size_bytes,
        }))
        .collect();
    Ok(Json(result))
}

/// GET /mail/folders/size-summary — agregado de tamanho por folder do user
/// (sprint #463). Foca SÓ em bytes (vs `folders/stats` #454 que mistura
/// total/unread/size). Retorna `{total_bytes, folders: [{folder, special_use,
/// size_bytes, message_count}]}` ordenado por size_bytes DESC pra UI tipo
/// "quem ocupa mais quota". Útil pra usuário identificar pastas pesadas antes
/// de cleanup. Path estático precede `/folders/:name` (lição #443/#448).
async fn folders_size_summary(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows: Vec<(String, Option<String>, i64, i64)> = sqlx::query_as(
        r#"
        SELECT mb.folder_name,
               mb.special_use,
               COALESCE(SUM(m.size_bytes)::bigint, 0) AS size_bytes,
               COUNT(m.id)                            AS message_count
        FROM mailboxes mb
        LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1
        WHERE mb.tenant_id = $1
          AND mb.user_id   = $2
        GROUP BY mb.folder_name, mb.special_use
        ORDER BY size_bytes DESC, mb.folder_name ASC
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let total_bytes: i64 = rows.iter().map(|(_, _, sz, _)| sz).sum();
    let folders: Vec<_> = rows.into_iter()
        .map(|(folder, special_use, size_bytes, message_count)| serde_json::json!({
            "folder":        folder,
            "special_use":   special_use,
            "size_bytes":    size_bytes,
            "message_count": message_count,
        }))
        .collect();

    Ok(Json(serde_json::json!({
        "total_bytes": total_bytes,
        "folders":     folders,
    })))
}

/// GET /mail/folders/:id/stats — stats de uma pasta por UUID.
///
/// Retorna `{folder_id, folder, special_use, total, unread, read, size_bytes}`.
/// 404 se o folder não pertence ao tenant/user. Paralelo de `/mail/folders/stats`
/// (#454) mas por UUID em vez de listar todos (sprint #581).
async fn folder_stats_by_id(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row: Option<(Uuid, String, Option<String>, i64, i64, i64)> = sqlx::query_as(
        r#"
        SELECT mb.id,
               mb.folder_name,
               mb.special_use,
               COUNT(m.id)                                                         AS total,
               COUNT(m.id) FILTER (WHERE NOT ('\Seen' = ANY(m.flags)))             AS unread,
               COALESCE(SUM(m.size_bytes)::bigint, 0)                              AS size_bytes
        FROM mailboxes mb
        LEFT JOIN messages m ON m.mailbox_id = mb.id AND m.tenant_id = $1
        WHERE mb.tenant_id = $1
          AND mb.user_id   = $2
          AND mb.id        = $3
        GROUP BY mb.id, mb.folder_name, mb.special_use
        "#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    let (fid, folder, special_use, total, unread, size_bytes) = row
        .ok_or(MailError::NotFound)?;

    Ok(Json(serde_json::json!({
        "folder_id":   fid,
        "folder":      folder,
        "special_use": special_use,
        "total":       total,
        "unread":      unread,
        "read":        total - unread,
        "size_bytes":  size_bytes,
    })))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_folder_name_passes() {
        assert!(validate_folder_name("Work Projects").is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        assert!(validate_folder_name("").is_err());
    }

    #[test]
    fn exactly_200_chars_passes() {
        assert!(validate_folder_name(&"a".repeat(200)).is_ok());
    }

    #[test]
    fn two_hundred_one_chars_rejected() {
        assert!(validate_folder_name(&"a".repeat(201)).is_err());
    }

    #[test]
    fn null_byte_rejected() {
        assert!(validate_folder_name("bad\0name").is_err());
    }

    #[test]
    fn carriage_return_rejected() {
        assert!(validate_folder_name("bad\rname").is_err());
    }

    #[test]
    fn newline_rejected() {
        assert!(validate_folder_name("bad\nname").is_err());
    }

    #[test]
    fn single_char_name_passes() {
        assert!(validate_folder_name("X").is_ok());
    }
}

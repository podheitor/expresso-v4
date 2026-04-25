//! Drive shared links API — criar/listar/revogar link + download público.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::fs;
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    domain::{FileRepo, Share, ShareRepo},
    error::{DriveError, Result},
    state::AppState,
};

/// Max TTL = 30 dias. Previne tokens eternos.
const MAX_TTL_SECONDS: i64 = 30 * 24 * 3600;
/// Default TTL = 7 dias quando expires_in_seconds omitido.
const DEFAULT_TTL_SECONDS: i64 = 7 * 24 * 3600;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drive/files/:id/shares",  get(list).post(create))
        .route("/api/v1/drive/shares/:id",        delete(revoke))
        .route("/api/v1/drive/share/:token",      get(public_download))
}

#[derive(Debug, Deserialize, Default)]
pub struct CreateBody {
    #[serde(default)]
    pub expires_in_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CreateResp {
    #[serde(flatten)]
    pub share: Share,
    /// Token cleartext — entregue uma única vez. Guarde com cuidado.
    pub token: String,
    /// URL relativa pronta p/ compartilhamento via gateway.
    pub url:   String,
}

async fn create(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(file_id): Path<Uuid>,
    Json(body):   Json<CreateBody>,
) -> Result<(StatusCode, Json<CreateResp>)> {
    let pool = state.db_or_unavailable()?;
    // Arquivo precisa existir + pertencer ao tenant do requisitante.
    let file = FileRepo::new(pool).get(ctx.tenant_id, file_id).await?;
    if file.kind != "file" {
        return Err(DriveError::BadRequest("can only share files".into()));
    }

    let ttl = body.expires_in_seconds.unwrap_or(DEFAULT_TTL_SECONDS);
    if ttl <= 0 || ttl > MAX_TTL_SECONDS {
        return Err(DriveError::BadRequest(format!(
            "expires_in_seconds must be 1..={MAX_TTL_SECONDS}"
        )));
    }
    let expires_at = OffsetDateTime::now_utc() + time::Duration::seconds(ttl);

    // Token = 32 bytes aleatórios base64url; somente sha256 persiste.
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = URL_SAFE_NO_PAD.encode(raw);
    let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));

    let share = ShareRepo::new(pool)
        .insert(ctx.tenant_id, file_id, &token_hash, ctx.user_id, expires_at)
        .await?;

    let url = format!("/api/v1/drive/share/{token}");
    tracing::info!(target: "audit",
        event = "drive.share.create",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id,
        file_id = %file_id, share_id = %share.id, ttl_s = ttl);
    Ok((StatusCode::CREATED, Json(CreateResp { share, token, url })))
}

async fn list(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(file_id): Path<Uuid>,
) -> Result<Json<Vec<Share>>> {
    let pool = state.db_or_unavailable()?;
    FileRepo::new(pool).get(ctx.tenant_id, file_id).await?;
    let rows = ShareRepo::new(pool).list_for_file(ctx.tenant_id, file_id).await?;
    Ok(Json(rows))
}

async fn revoke(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    let pool    = state.db_or_unavailable()?;
    let changed = ShareRepo::new(pool).revoke(ctx.tenant_id, id).await?;
    if changed == 0 { return Err(DriveError::NotFound(id)); }
    tracing::info!(target: "audit",
        event = "drive.share.revoke",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, share_id = %id);
    Ok(StatusCode::NO_CONTENT)
}

async fn public_download(
    State(state): State<AppState>,
    Path(token):  Path<String>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));
    let repo = ShareRepo::new(pool);
    let resolved = repo.resolve(&token_hash).await?
        .ok_or(DriveError::Forbidden)?;

    let now = OffsetDateTime::now_utc();
    if resolved.revoked_at.is_some() || resolved.expires_at < now {
        return Err(DriveError::Forbidden);
    }

    // Download via pool (bypass tenant — owner do blob é o tenant do share).
    // Usamos fetch direto por id + tenant já conhecidos.
    let file = FileRepo::new(pool).get(resolved.tenant_id, resolved.file_id).await?;
    if file.kind != "file" {
        return Err(DriveError::BadRequest("share target is a folder".into()));
    }
    let key = file.storage_key.as_deref()
        .ok_or_else(|| DriveError::BadRequest("file has no content".into()))?;
    let bytes = fs::read(state.data_root().join(key)).await?;

    tracing::info!(target: "audit",
        event = "drive.share.download",
        share_id = %resolved.id, tenant_id = %resolved.tenant_id, file_id = %resolved.file_id);

    Ok(crate::api::files::attachment_response(
        &file.name,
        file.mime_type.as_deref(),
        bytes,
    ))
}

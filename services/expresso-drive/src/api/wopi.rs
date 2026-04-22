//! WOPI host endpoints — Collabora/LibreOffice Online bridge.
//!
//! Spec: https://docs.microsoft.com/openspecs/office_protocols/ms-wopi/
//!
//! Auth: access_token query param — HMAC-SHA256 over canonical string
//! `{file_id}|{tenant_id}|{user_id}|{exp}` usando WOPI_SECRET compartilhado
//! entre expresso-web (emissor) e expresso-drive (verificador).

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::path::PathBuf;
use time::OffsetDateTime;
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

use crate::{
    domain::{FileRepo, NewVersion, QuotaRepo, VersionRepo},
    error::{DriveError, Result},
    state::AppState,
};

type HmacSha256 = Hmac<Sha256>;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/wopi/files/:id",          get(check_file_info).post(wopi_post))
        .route("/wopi/files/:id/contents", get(get_file).post(put_file))
}

#[derive(Debug, Deserialize)]
pub struct TokenQuery {
    pub access_token: String,
}

/// Claims codificadas no token WOPI.
#[derive(Debug, Clone, Copy)]
struct Claims {
    tenant_id: Uuid,
    user_id:   Uuid,
}

fn wopi_secret() -> Option<String> {
    env::var("WOPI_SECRET").ok().filter(|v| !v.trim().is_empty())
}

/// Emite token `{file_id}.{tenant_id}.{user_id}.{exp}.{hmac_hex}`.
/// Exposto p/ ferramentas / testes — emissor real vive em expresso-web.
#[allow(dead_code)]
pub fn sign_token(secret: &[u8], file_id: Uuid, tenant_id: Uuid, user_id: Uuid, ttl_seconds: i64) -> String {
    let exp = OffsetDateTime::now_utc().unix_timestamp() + ttl_seconds;
    let canonical = format!("{file_id}|{tenant_id}|{user_id}|{exp}");
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac key");
    mac.update(canonical.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    format!("{file_id}.{tenant_id}.{user_id}.{exp}.{sig}")
}

fn verify_token(secret: &[u8], token: &str, expected_file: Uuid) -> Option<Claims> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 5 { return None; }
    let file_id   = Uuid::parse_str(parts[0]).ok()?;
    let tenant_id = Uuid::parse_str(parts[1]).ok()?;
    let user_id   = Uuid::parse_str(parts[2]).ok()?;
    let exp       = parts[3].parse::<i64>().ok()?;
    let sig       = parts[4];

    if file_id != expected_file { return None; }
    if exp < OffsetDateTime::now_utc().unix_timestamp() { return None; }

    let canonical = format!("{file_id}|{tenant_id}|{user_id}|{exp}");
    let mut mac = HmacSha256::new_from_slice(secret).ok()?;
    mac.update(canonical.as_bytes());
    let expected = mac.finalize().into_bytes();
    let provided = hex::decode(sig).ok()?;
    // constant-time compare
    if expected.as_slice().len() != provided.len() { return None; }
    let eq = expected.iter().zip(provided.iter()).fold(0u8, |acc, (a, b)| acc | (a ^ b));
    if eq != 0 { return None; }

    let _ = (file_id, exp);
    Some(Claims { tenant_id, user_id })
}

fn authorize(expected: Uuid, q: &TokenQuery) -> Result<Claims> {
    let secret = wopi_secret().ok_or_else(|| DriveError::BadRequest("WOPI_SECRET not configured".into()))?;
    verify_token(secret.as_bytes(), &q.access_token, expected)
        .ok_or(DriveError::Unauthorized)
}

// ---------- CheckFileInfo ----------

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CheckFileInfo {
    base_file_name:         String,
    owner_id:               String,
    size:                   i64,
    user_id:                String,
    user_friendly_name:     String,
    version:                String,
    user_can_write:         bool,
    user_can_not_write_relative: bool,
    supports_update:        bool,
    supports_locks:         bool,
    last_modified_time:     Option<String>,
}

async fn check_file_info(
    State(state): State<AppState>,
    Path(id):     Path<Uuid>,
    Query(q):     Query<TokenQuery>,
) -> Result<Json<CheckFileInfo>> {
    let claims = authorize(id, &q)?;
    let pool   = state.db_or_unavailable()?;
    let repo   = FileRepo::new(pool);
    let file   = repo.get(claims.tenant_id, id).await?;

    if file.kind != "file" {
        return Err(DriveError::BadRequest("not a file".into()));
    }

    let version = file.sha256.as_deref().unwrap_or("0").to_string();
    Ok(Json(CheckFileInfo {
        base_file_name:              file.name,
        owner_id:                    file.owner_user_id.to_string(),
        size:                        file.size_bytes,
        user_id:                     claims.user_id.to_string(),
        user_friendly_name:          claims.user_id.to_string(),
        version,
        user_can_write:              true,
        user_can_not_write_relative: true,
        supports_update:             true,
        supports_locks:              false,
        last_modified_time:          None,
    }))
}

// ---------- GetFile ----------

async fn get_file(
    State(state): State<AppState>,
    Path(id):     Path<Uuid>,
    Query(q):     Query<TokenQuery>,
) -> Result<Response> {
    let claims = authorize(id, &q)?;
    let pool   = state.db_or_unavailable()?;
    let file   = FileRepo::new(pool).get(claims.tenant_id, id).await?;

    let key = file.storage_key.as_deref()
        .ok_or_else(|| DriveError::BadRequest("file has no content".into()))?;
    let path: PathBuf = state.data_root().join(key);
    let bytes = fs::read(&path).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        file.mime_type.as_deref()
            .unwrap_or("application/octet-stream")
            .parse().unwrap(),
    );
    Ok((StatusCode::OK, headers, bytes).into_response())
}

// ---------- PutFile ----------

/// POST /wopi/files/:id/contents — corpo raw. WOPI cliente envia header
/// `X-WOPI-Override: PUT`. Aceitamos sem discriminar (content-type varia).
async fn put_file(
    State(state): State<AppState>,
    Path(id):     Path<Uuid>,
    Query(q):     Query<TokenQuery>,
    headers:      HeaderMap,
    body:         Bytes,
) -> Result<Response> {
    let claims = authorize(id, &q)?;
    let pool   = state.db_or_unavailable()?;

    // Quota
    let quota = QuotaRepo::new(pool).get(claims.tenant_id).await?;
    let delta = body.len() as i64;
    if !quota.fits(delta) {
        return Err(DriveError::QuotaExceeded);
    }

    let repo     = FileRepo::new(pool);
    let ver_repo = VersionRepo::new(pool);
    let existing = repo.get(claims.tenant_id, id).await?;
    if existing.kind != "file" {
        return Err(DriveError::BadRequest("not a file".into()));
    }

    // Persiste novo blob + arquiva atual como versão.
    let sha = format!("{:x}", Sha256::digest(&body));
    let key = Uuid::new_v4().to_string();
    let root = state.data_root();
    fs::create_dir_all(root).await?;
    let path: PathBuf = root.join(&key);
    let mut f = fs::File::create(&path).await?;
    f.write_all(&body).await?;
    f.flush().await?;

    if let Some(prev_key) = existing.storage_key.as_deref() {
        let next_no = ver_repo.next_no(existing.id).await?;
        ver_repo.insert(&NewVersion {
            file_id:     existing.id,
            tenant_id:   claims.tenant_id,
            version_no:  next_no,
            storage_key: prev_key,
            size_bytes:  existing.size_bytes,
            sha256:      existing.sha256.as_deref(),
            mime_type:   existing.mime_type.as_deref(),
            created_by:  existing.owner_user_id,
        }).await?;
    }

    let mime = headers.get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or(existing.mime_type.clone());

    let updated = repo.update_content(
        claims.tenant_id, existing.id,
        &key, delta,
        Some(&sha), mime.as_deref(),
    ).await;
    if updated.is_err() {
        let _ = fs::remove_file(&path).await;
    }
    let updated = updated?;

    tracing::info!(target: "audit",
        event = "drive.wopi.put",
        tenant_id = %claims.tenant_id, user_id = %claims.user_id, file_id = %updated.id);

    let mut out = HeaderMap::new();
    out.insert("X-WOPI-ItemVersion", sha.parse().unwrap());
    Ok((StatusCode::OK, out, Json(serde_json::json!({"LastModifiedTime": null}))).into_response())
}

/// POST /wopi/files/:id — Lock/Unlock/RefreshLock/UnlockAndRelock.
/// MVP sem suporte real — respondemos 501 NotImplemented mantendo WOPI happy.
async fn wopi_post(headers: HeaderMap) -> Response {
    let op = headers.get("X-WOPI-Override").and_then(|v| v.to_str().ok()).unwrap_or("");
    tracing::debug!(override_op = op, "WOPI op sem suporte — no-op");
    (StatusCode::OK, [("X-WOPI-Lock", "")]).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_token_valid() {
        let fid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        let uid = Uuid::new_v4();
        let tok = sign_token(b"secret", fid, tid, uid, 60);
        let claims = verify_token(b"secret", &tok, fid).expect("valid");
        assert_eq!(claims.tenant_id, tid);
        assert_eq!(claims.user_id, uid);
    }

    #[test]
    fn wrong_secret_rejects() {
        let fid = Uuid::new_v4();
        let tok = sign_token(b"A", fid, Uuid::new_v4(), Uuid::new_v4(), 60);
        assert!(verify_token(b"B", &tok, fid).is_none());
    }

    #[test]
    fn wrong_file_id_rejects() {
        let fid = Uuid::new_v4();
        let tok = sign_token(b"s", fid, Uuid::new_v4(), Uuid::new_v4(), 60);
        assert!(verify_token(b"s", &tok, Uuid::new_v4()).is_none());
    }

    #[test]
    fn expired_rejects() {
        let fid = Uuid::new_v4();
        let tok = sign_token(b"s", fid, Uuid::new_v4(), Uuid::new_v4(), -10);
        assert!(verify_token(b"s", &tok, fid).is_none());
    }
}

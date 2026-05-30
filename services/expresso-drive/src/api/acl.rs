//! Drive file/folder ACL management — internal user-to-user sharing.
//!
//! GET    /api/v1/drive/files/:id/acl              — list grants on a node
//! POST   /api/v1/drive/files/:id/acl              — grant a user READ/WRITE/ADMIN
//! DELETE /api/v1/drive/files/:id/acl/:grantee_id  — revoke a user's grant
//!
//! Only the node's OWNER or an ADMIN grantee may manage its ACL. The grant
//! itself takes effect once the read/write paths consult `AclRepo::access_level`
//! (phased in separately); this module lands the management surface + the
//! ownership gate.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{AclRepo, FileAcl, FileRepo};
use crate::error::{DriveError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drive/files/:id/acl", get(list_acl).post(grant_acl))
        .route(
            "/api/v1/drive/files/:id/acl/:grantee_id",
            axum::routing::delete(revoke_acl),
        )
}

#[derive(Debug, Deserialize)]
struct GrantBody {
    grantee_id: Uuid,
    privilege: String,
}

/// Require the caller to be OWNER or hold ADMIN on the node; otherwise 403
/// (404 when the node doesn't exist / isn't visible to the tenant).
async fn require_admin(
    pool: &expresso_core::DbPool,
    tenant_id: Uuid,
    file_id: Uuid,
    user_id: Uuid,
) -> Result<()> {
    // Ensure the node exists in the tenant first (clean 404, not a bare 403).
    FileRepo::new(pool).get(tenant_id, file_id).await?;
    match AclRepo::new(pool)
        .access_level(tenant_id, file_id, user_id)
        .await?
        .as_deref()
    {
        Some("OWNER") | Some("ADMIN") => Ok(()),
        _ => Err(DriveError::Forbidden),
    }
}

async fn list_acl(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(file_id): Path<Uuid>,
) -> Result<Json<Vec<FileAcl>>> {
    let pool = state.db_or_unavailable()?;
    require_admin(pool, ctx.tenant_id, file_id, ctx.user_id).await?;
    let acls = AclRepo::new(pool).list(ctx.tenant_id, file_id).await?;
    Ok(Json(acls))
}

async fn grant_acl(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(file_id): Path<Uuid>,
    Json(body): Json<GrantBody>,
) -> Result<(StatusCode, Json<FileAcl>)> {
    let pool = state.db_or_unavailable()?;
    require_admin(pool, ctx.tenant_id, file_id, ctx.user_id).await?;
    if body.grantee_id == ctx.user_id {
        return Err(DriveError::BadRequest(
            "cannot grant to the owner/self".into(),
        ));
    }
    let acl = AclRepo::new(pool)
        .grant(ctx.tenant_id, file_id, body.grantee_id, &body.privilege)
        .await?;
    Ok((StatusCode::CREATED, Json(acl)))
}

async fn revoke_acl(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((file_id, grantee_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    require_admin(pool, ctx.tenant_id, file_id, ctx.user_id).await?;
    AclRepo::new(pool)
        .revoke(ctx.tenant_id, file_id, grantee_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

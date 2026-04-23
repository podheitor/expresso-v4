//! Address book sharing (ACL) endpoints. Owner-managed.

use axum::{
    extract::{Path, State},
    routing::{delete, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{ContactsError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/addressbooks/:book_id/acl",             get(list_acl).post(share))
        .route("/api/v1/addressbooks/:book_id/acl/:grantee_id", delete(revoke))
}

#[derive(Debug, Deserialize)]
pub struct ShareRequest {
    pub grantee_id: Uuid,
    pub privilege:  String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AclEntry {
    pub addressbook_id: Uuid,
    pub tenant_id:      Uuid,
    pub grantee_id:     Uuid,
    pub privilege:      String,
    #[sqlx(default)]
    #[serde(default)]
    pub email:          Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:     OffsetDateTime,
}

fn validate_priv(p: &str) -> Result<String> {
    let up = p.trim().to_ascii_uppercase();
    match up.as_str() {
        "READ" | "WRITE" | "ADMIN" => Ok(up),
        _ => Err(ContactsError::BadRequest(format!("invalid privilege: {p}"))),
    }
}

async fn assert_owner(
    pool: &expresso_core::DbPool,
    tenant_id: Uuid,
    book_id:   Uuid,
    user_id:   Uuid,
) -> Result<()> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT owner_user_id FROM addressbooks WHERE id = $1 AND tenant_id = $2",
    )
    .bind(book_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((owner,)) if owner == user_id => Ok(()),
        Some(_) => Err(ContactsError::Forbidden),
        None    => Err(ContactsError::AddressbookNotFound(book_id.to_string())),
    }
}

async fn list_acl(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(book_id): Path<Uuid>,
) -> Result<Json<Vec<AclEntry>>> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, book_id, ctx.user_id).await?;

    let rows: Vec<AclEntry> = sqlx::query_as(
        r#"SELECT a.addressbook_id, a.tenant_id, a.grantee_id, a.privilege, u.email, a.created_at
             FROM addressbook_acl a
             LEFT JOIN users u ON u.id = a.grantee_id
            WHERE a.addressbook_id = $1 AND a.tenant_id = $2
            ORDER BY a.created_at"#,
    )
    .bind(book_id)
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    Ok(Json(rows))
}

async fn share(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(book_id): Path<Uuid>,
    Json(req):    Json<ShareRequest>,
) -> Result<Json<AclEntry>> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, book_id, ctx.user_id).await?;

    if req.grantee_id == ctx.user_id {
        return Err(ContactsError::BadRequest("owner already has full access".into()));
    }
    let priv_up = validate_priv(&req.privilege)?;

    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM users WHERE id = $1 AND tenant_id = $2",
    )
    .bind(req.grantee_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(ContactsError::BadRequest(format!("grantee not in tenant: {}", req.grantee_id)));
    }

    let row: AclEntry = sqlx::query_as(
        r#"WITH ins AS (
             INSERT INTO addressbook_acl (addressbook_id, tenant_id, grantee_id, privilege)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (addressbook_id, grantee_id)
             DO UPDATE SET privilege = EXCLUDED.privilege
             RETURNING addressbook_id, tenant_id, grantee_id, privilege, created_at
           )
           SELECT ins.addressbook_id, ins.tenant_id, ins.grantee_id, ins.privilege, u.email, ins.created_at
             FROM ins LEFT JOIN users u ON u.id = ins.grantee_id"#,
    )
    .bind(book_id)
    .bind(ctx.tenant_id)
    .bind(req.grantee_id)
    .bind(&priv_up)
    .fetch_one(pool)
    .await?;

    Ok(Json(row))
}

async fn revoke(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((book_id, grantee_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, book_id, ctx.user_id).await?;

    let res = sqlx::query(
        "DELETE FROM addressbook_acl WHERE addressbook_id = $1 AND grantee_id = $2 AND tenant_id = $3",
    )
    .bind(book_id)
    .bind(grantee_id)
    .bind(ctx.tenant_id)
    .execute(pool)
    .await?;

    Ok(Json(serde_json::json!({ "revoked": res.rows_affected() })))
}

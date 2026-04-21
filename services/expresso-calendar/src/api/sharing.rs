//! Calendar sharing (ACL) endpoints.
//!
//! Model: only the calendar owner may manage its ACL. Grants store the
//! grantee + privilege (READ|WRITE|ADMIN) in `calendar_acl`. List of shares
//! is visible to the owner; a grantee sees the calendar via the list
//! endpoint only if the repository joins calendar_acl (future work).

use axum::{
    extract::{Path, State},
    routing::{delete, get},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{
    api::context::RequestCtx,
    error::{CalendarError, Result},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/calendars/:cal_id/acl",              get(list_acl).post(share))
        .route("/api/v1/calendars/:cal_id/acl/:grantee_id",  delete(revoke))
}

#[derive(Debug, Deserialize)]
pub struct ShareRequest {
    pub grantee_id: Uuid,
    pub privilege:  String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct AclEntry {
    pub calendar_id: Uuid,
    pub tenant_id:   Uuid,
    pub grantee_id:  Uuid,
    pub privilege:   String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:  OffsetDateTime,
}

fn validate_priv(p: &str) -> Result<String> {
    let up = p.trim().to_ascii_uppercase();
    match up.as_str() {
        "READ" | "WRITE" | "ADMIN" => Ok(up),
        _ => Err(CalendarError::BadRequest(format!("invalid privilege: {p}"))),
    }
}

async fn assert_owner(
    pool: &expresso_core::DbPool,
    tenant_id: Uuid,
    cal_id:    Uuid,
    user_id:   Uuid,
) -> Result<()> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT owner_user_id FROM calendars WHERE id = $1 AND tenant_id = $2",
    )
    .bind(cal_id)
    .bind(tenant_id)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((owner,)) if owner == user_id => Ok(()),
        Some(_) => Err(CalendarError::Forbidden),
        None    => Err(CalendarError::CalendarNotFound(cal_id.to_string())),
    }
}

pub async fn list_acl(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
) -> Result<Json<Vec<AclEntry>>> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let rows: Vec<AclEntry> = sqlx::query_as(
        r#"SELECT calendar_id, tenant_id, grantee_id, privilege, created_at
             FROM calendar_acl
            WHERE calendar_id = $1 AND tenant_id = $2
            ORDER BY created_at"#,
    )
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    Ok(Json(rows))
}

pub async fn share(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Json(req):    Json<ShareRequest>,
) -> Result<Json<AclEntry>> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    if req.grantee_id == ctx.user_id {
        return Err(CalendarError::BadRequest("owner already has full access".into()));
    }

    let priv_up = validate_priv(&req.privilege)?;

    // Confirm grantee exists inside tenant → avoid dangling FK surprise on races.
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM users WHERE id = $1 AND tenant_id = $2",
    )
    .bind(req.grantee_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(CalendarError::BadRequest(format!("grantee not in tenant: {}", req.grantee_id)));
    }

    let row: AclEntry = sqlx::query_as(
        r#"INSERT INTO calendar_acl (calendar_id, tenant_id, grantee_id, privilege)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (calendar_id, grantee_id)
           DO UPDATE SET privilege = EXCLUDED.privilege
           RETURNING calendar_id, tenant_id, grantee_id, privilege, created_at"#,
    )
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .bind(req.grantee_id)
    .bind(&priv_up)
    .fetch_one(pool)
    .await?;

    Ok(Json(row))
}

pub async fn revoke(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, grantee_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let res = sqlx::query(
        "DELETE FROM calendar_acl WHERE calendar_id = $1 AND grantee_id = $2 AND tenant_id = $3",
    )
    .bind(cal_id)
    .bind(grantee_id)
    .bind(ctx.tenant_id)
    .execute(pool)
    .await?;

    Ok(Json(serde_json::json!({ "revoked": res.rows_affected() })))
}

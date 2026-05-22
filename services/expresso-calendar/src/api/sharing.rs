//! Calendar sharing (ACL) endpoints.
//!
//! Model: only the calendar owner may manage its ACL. Grants store the
//! grantee + privilege (READ|WRITE|ADMIN) in `calendar_acl`. List of shares
//! is visible to the owner; a grantee sees the calendar via the list
//! endpoint only if the repository joins calendar_acl (future work).

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
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
    #[sqlx(default)]
    #[serde(default)]
    pub email:       Option<String>,
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
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let max_created: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM calendar_acl WHERE calendar_id = $1 AND tenant_id = $2",
    )
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_created {
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

    let rows: Vec<AclEntry> = sqlx::query_as(
        r#"SELECT a.calendar_id, a.tenant_id, a.grantee_id, a.privilege, u.email, a.created_at
             FROM calendar_acl a
             LEFT JOIN users u ON u.id = a.grantee_id
            WHERE a.calendar_id = $1 AND a.tenant_id = $2
            ORDER BY a.created_at"#,
    )
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_created {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
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
        r#"WITH ins AS (
             INSERT INTO calendar_acl (calendar_id, tenant_id, grantee_id, privilege)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (calendar_id, grantee_id)
             DO UPDATE SET privilege = EXCLUDED.privilege
             RETURNING calendar_id, tenant_id, grantee_id, privilege, created_at
           )
           SELECT ins.calendar_id, ins.tenant_id, ins.grantee_id, ins.privilege, u.email, ins.created_at
             FROM ins LEFT JOIN users u ON u.id = ins.grantee_id"#,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_priv_accepts_read() {
        assert_eq!(validate_priv("read").unwrap(), "READ");
    }

    #[test]
    fn validate_priv_accepts_write() {
        assert_eq!(validate_priv("WRITE").unwrap(), "WRITE");
    }

    #[test]
    fn validate_priv_accepts_admin() {
        assert_eq!(validate_priv(" Admin ").unwrap(), "ADMIN");
    }

    #[test]
    fn validate_priv_rejects_invalid() {
        assert!(validate_priv("OWNER").is_err());
        assert!(validate_priv("").is_err());
    }

    #[test]
    fn validate_priv_normalizes_mixed_case() {
        assert_eq!(validate_priv("wRiTe").unwrap(), "WRITE");
    }

    #[test]
    fn share_request_deser() {
        use uuid::Uuid;
        let id = Uuid::new_v4();
        let json = format!(r#"{{"grantee_id":"{id}","privilege":"read"}}"#);
        let r: ShareRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r.privilege, "read");
        assert_eq!(r.grantee_id, id);
    }

    #[test]
    fn validate_priv_admin_accepted() {
        assert_eq!(validate_priv("admin").unwrap(), "ADMIN");
    }

    #[test]
    fn validate_priv_garbage_rejected() {
        assert!(validate_priv("owner").is_err());
    }

    #[test]
    fn validate_priv_read_accepted() {
        assert_eq!(validate_priv("read").unwrap(), "READ");
    }

    #[test]
    fn validate_priv_write_normalised_uppercase() {
        assert_eq!(validate_priv("write").unwrap(), "WRITE");
    }

    #[test]
    fn validate_priv_invalid_value_returns_error() {
        assert!(validate_priv("owner").is_err());
    }

    #[test]
    fn validate_priv_empty_string_returns_error() {
        assert!(validate_priv("").is_err());
    }

    #[test]
    fn validate_priv_write_accepted() {
        assert!(validate_priv("write").is_ok());
    }

    #[test]
    fn validate_priv_unknown_value_returns_error() {
        assert!(validate_priv("owner").is_err());
    }

    #[test]
    fn acl_entry_privilege_field_preserved() {
        use uuid::Uuid;
        use time::macros::datetime;
        let entry = AclEntry {
            calendar_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            grantee_id: Uuid::nil(),
            privilege: "READ".into(),
            email: None,
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(entry.privilege, "READ");
    }

    #[test]
    fn validate_priv_mixed_case_normalises_to_uppercase() {
        assert_eq!(validate_priv("Write").unwrap(), "WRITE");
        assert_eq!(validate_priv("READ").unwrap(), "READ");
    }
}

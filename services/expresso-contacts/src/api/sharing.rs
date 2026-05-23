//! Address book sharing (ACL) endpoints. Owner-managed.

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
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Path(book_id): Path<Uuid>,
    req_headers:   HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    assert_owner(pool, ctx.tenant_id, book_id, ctx.user_id).await?;

    let max_created: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(created_at) FROM addressbook_acl WHERE addressbook_id = $1 AND tenant_id = $2",
    )
    .bind(book_id)
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

    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_created {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_priv_accepts_read() {
        assert_eq!(validate_priv("read").unwrap(), "READ");
    }

    #[test]
    fn validate_priv_accepts_write() {
        assert_eq!(validate_priv("write").unwrap(), "WRITE");
    }

    #[test]
    fn validate_priv_accepts_admin() {
        assert_eq!(validate_priv(" Admin ").unwrap(), "ADMIN");
    }

    #[test]
    fn validate_priv_rejects_unknown() {
        assert!(validate_priv("OWNER").is_err());
    }

    #[test]
    fn validate_priv_rejects_empty() {
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
        let json = format!(r#"{{"grantee_id":"{id}","privilege":"admin"}}"#);
        let r: ShareRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r.privilege, "admin");
    }

    #[test]
    fn acl_entry_privilege_in_roundtrip() {
        use uuid::Uuid;
        use time::macros::datetime;
        let entry = AclEntry {
            addressbook_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            grantee_id: Uuid::nil(),
            privilege: "READ".into(),
            email: None,
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        let v: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(v["privilege"], "READ");
    }

    #[test]
    fn acl_entry_grantee_id_preserved() {
        use uuid::Uuid;
        use time::macros::datetime;
        let gid = Uuid::new_v4();
        let entry = AclEntry {
            addressbook_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            grantee_id: gid,
            privilege: "WRITE".into(),
            email: None,
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert_eq!(entry.grantee_id, gid);
    }

    #[test]
    fn share_request_privilege_preserved() {
        let json = r#"{"grantee_id":"00000000-0000-0000-0000-000000000000","privilege":"READ"}"#;
        let r: ShareRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.privilege, "READ");
    }

    #[test]
    fn share_request_grantee_id_preserved() {
        let id = uuid::Uuid::nil();
        let json = format!(r#"{{"grantee_id":"{id}","privilege":"WRITE"}}"#);
        let r: ShareRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r.grantee_id, id);
    }

    #[test]
    fn share_request_write_privilege_preserved() {
        let id = uuid::Uuid::nil();
        let json = format!(r#"{{"grantee_id":"{id}","privilege":"WRITE"}}"#);
        let r: ShareRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(r.privilege, "WRITE");
    }

    #[test]
    fn share_request_nil_grantee_deserializes() {
        let json = r#"{"grantee_id":"00000000-0000-0000-0000-000000000000","privilege":"READ"}"#;
        let r: ShareRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.grantee_id, uuid::Uuid::nil());
    }

    #[test]
    fn validate_priv_rejects_owner() {
        assert!(validate_priv("OWNER").is_err());
    }

    #[test]
    fn acl_entry_email_none_allowed() {
        use uuid::Uuid;
        use time::macros::datetime;
        let entry = AclEntry {
            addressbook_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            grantee_id: Uuid::nil(),
            privilege: "READ".into(),
            email: None,
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        assert!(entry.email.is_none());
    }

    #[test]
    fn validate_priv_accepts_admin_lowercase() {
        assert_eq!(validate_priv("admin").unwrap(), "ADMIN");
    }

    #[test]
    fn validate_priv_trims_whitespace() {
        assert_eq!(validate_priv("  read  ").unwrap(), "READ");
    }

    #[test]
    fn validate_priv_uppercase_write_accepted() {
        assert_eq!(validate_priv("WRITE").unwrap(), "WRITE");
    }

    #[test]
    fn validate_priv_read_uppercase_accepted() {
        assert_eq!(validate_priv("READ").unwrap(), "READ");
    }

    #[test]
    fn validate_priv_rejects_guest() {
        assert!(validate_priv("GUEST").is_err());
    }

    #[test]
    fn validate_priv_rejects_viewer() {
        assert!(validate_priv("VIEWER").is_err());
    }

    #[test]
    fn acl_entry_privilege_is_string_type() {
        let json = r#"{"grantee_id":"00000000-0000-0000-0000-000000000000","privilege":"admin"}"#;
        let req: ShareRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.privilege, "admin");
    }
}

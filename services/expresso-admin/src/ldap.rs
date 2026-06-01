//! Per-tenant LDAP / AD federation configuration API — CRUD for ldap_config.
//!
//! Tenant-scoped: super_admin may manage any tenant; a tenant_admin only their
//! own (enforced by `auth::require_tenant_match`). Keycloak User Federation does
//! the actual LDAP work — this service stores the registration that Keycloak
//! provisioning (sprint B) consumes.
//!
//! SECURITY: the bind-account password is never stored in `ldap_config`. The
//! upsert body accepts `bind_credential` only to forward it to Keycloak (sprint
//! B writes it into the KC component); this handler does not persist it and GET
//! never returns it. Keycloak is the source of truth for the secret.
//!
//! GET    /api/v1/ldap/configs?tenant_id=  — list a tenant's LDAP configs
//! POST   /api/v1/ldap/configs            — create/upsert a config
//! GET    /api/v1/ldap/configs/:id        — fetch one by id
//! DELETE /api/v1/ldap/configs/:id        — remove one

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth, AppState};

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct LdapConfig {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub alias: String,
    pub vendor: String,
    pub connection_url: String,
    pub users_dn: String,
    pub bind_dn: String,
    pub username_attr: String,
    pub rdn_attr: String,
    pub uuid_attr: String,
    pub user_object_classes: String,
    pub search_scope: i32,
    pub sync_period_secs: Option<i32>,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct TenantQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct UpsertLdapBody {
    pub tenant_id: Uuid,
    pub alias: String,
    pub vendor: Option<String>,
    pub connection_url: String,
    pub users_dn: String,
    pub bind_dn: String,
    /// LDAP bind password. Write-only: forwarded to Keycloak by the sync step
    /// (sprint B), never persisted in `ldap_config`. Accepted here so the admin
    /// supplies it in one request.
    #[serde(default)]
    pub bind_credential: Option<String>,
    pub username_attr: Option<String>,
    pub rdn_attr: Option<String>,
    pub uuid_attr: Option<String>,
    pub user_object_classes: Option<String>,
    pub search_scope: Option<i32>,
    pub sync_period_secs: Option<i32>,
    pub enabled: Option<bool>,
}

const COLS: &str = "id, tenant_id, alias, vendor, connection_url, users_dn, bind_dn, \
     username_attr, rdn_attr, uuid_attr, user_object_classes, search_scope, \
     sync_period_secs, enabled, created_at, updated_at";

// Err is an axum Response by design (early-return into the handler); boxing it
// would only add an allocation on the unavailable path.
#[allow(clippy::result_large_err)]
fn db_or_503(st: &Arc<AppState>) -> Result<&expresso_core::DbPool, Response> {
    st.db
        .as_ref()
        .ok_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<TenantQuery>,
) -> Result<Response, Response> {
    if let Some(r) = auth::require_tenant_match(&st, &headers, q.tenant_id).await {
        return Err(r);
    }
    let pool = db_or_503(&st)?;
    let rows: Vec<LdapConfig> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM ldap_config WHERE tenant_id = $1 ORDER BY alias"
    ))
    .bind(q.tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    Ok(Json(rows).into_response())
}

pub async fn get_one(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Response, Response> {
    let pool = db_or_503(&st)?;
    let row: Option<LdapConfig> =
        sqlx::query_as(&format!("SELECT {COLS} FROM ldap_config WHERE id = $1"))
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    let r = row.ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
    // Authorize against the row's tenant so a tenant_admin can't read another
    // tenant's config by guessing the id.
    if let Some(resp) = auth::require_tenant_match(&st, &headers, r.tenant_id).await {
        return Err(resp);
    }
    Ok(Json(r).into_response())
}

pub async fn upsert(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<UpsertLdapBody>,
) -> Result<(StatusCode, Json<LdapConfig>), Response> {
    if let Some(r) = auth::require_tenant_match(&st, &headers, body.tenant_id).await {
        return Err(r);
    }
    if body.alias.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "alias must not be empty").into_response());
    }
    if body.connection_url.trim().is_empty()
        || body.users_dn.trim().is_empty()
        || body.bind_dn.trim().is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "connection_url, users_dn and bind_dn are required",
        )
            .into_response());
    }
    let pool = db_or_503(&st)?;
    let row: LdapConfig = sqlx::query_as(&format!(
        "INSERT INTO ldap_config \
             (tenant_id, alias, vendor, connection_url, users_dn, bind_dn, username_attr, \
              rdn_attr, uuid_attr, user_object_classes, search_scope, sync_period_secs, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (tenant_id, alias) DO UPDATE SET \
             vendor = EXCLUDED.vendor, \
             connection_url = EXCLUDED.connection_url, \
             users_dn = EXCLUDED.users_dn, \
             bind_dn = EXCLUDED.bind_dn, \
             username_attr = EXCLUDED.username_attr, \
             rdn_attr = EXCLUDED.rdn_attr, \
             uuid_attr = EXCLUDED.uuid_attr, \
             user_object_classes = EXCLUDED.user_object_classes, \
             search_scope = EXCLUDED.search_scope, \
             sync_period_secs = EXCLUDED.sync_period_secs, \
             enabled = EXCLUDED.enabled, \
             updated_at = now() \
         RETURNING {COLS}"
    ))
    .bind(body.tenant_id)
    .bind(body.alias.trim())
    .bind(body.vendor.as_deref().unwrap_or("other"))
    .bind(&body.connection_url)
    .bind(&body.users_dn)
    .bind(&body.bind_dn)
    .bind(body.username_attr.as_deref().unwrap_or("uid"))
    .bind(body.rdn_attr.as_deref().unwrap_or("uid"))
    .bind(body.uuid_attr.as_deref().unwrap_or("entryUUID"))
    .bind(
        body.user_object_classes
            .as_deref()
            .unwrap_or("inetOrgPerson, organizationalPerson"),
    )
    .bind(body.search_scope.unwrap_or(2))
    .bind(body.sync_period_secs)
    .bind(body.enabled.unwrap_or(true))
    .fetch_one(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    // bind_credential is intentionally not persisted (write-through to Keycloak
    // happens in the sync step); we only note whether one was supplied.
    tracing::info!(target: "audit",
        event = "ldap.config.upsert",
        tenant_id = %body.tenant_id,
        alias = %row.alias,
        credential_supplied = body.bind_credential.is_some());
    Ok((StatusCode::OK, Json(row)))
}

pub async fn delete(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, Response> {
    let pool = db_or_503(&st)?;
    let tenant_id: Option<Uuid> =
        sqlx::query_scalar("SELECT tenant_id FROM ldap_config WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    let tenant_id = tenant_id.ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
    if let Some(resp) = auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Err(resp);
    }
    sqlx::query("DELETE FROM ldap_config WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    tracing::info!(target: "audit", event = "ldap.config.delete", id = %id, tenant_id = %tenant_id);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_body_deser_minimal() {
        let t = Uuid::new_v4();
        let json = format!(
            r#"{{"tenant_id":"{t}","alias":"corp-ad","connection_url":"ldaps://ad:636",
                 "users_dn":"CN=Users,DC=corp","bind_dn":"CN=svc,DC=corp"}}"#
        );
        let b: UpsertLdapBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.alias, "corp-ad");
        assert_eq!(b.tenant_id, t);
        assert!(b.vendor.is_none());
        assert!(b.bind_credential.is_none());
        assert!(b.search_scope.is_none());
    }

    #[test]
    fn upsert_body_deser_full() {
        let t = Uuid::new_v4();
        let json = format!(
            r#"{{"tenant_id":"{t}","alias":"ad","vendor":"ad","connection_url":"ldaps://ad",
                 "users_dn":"dc=x","bind_dn":"cn=svc","bind_credential":"s3cret",
                 "username_attr":"sAMAccountName","search_scope":1,"enabled":false}}"#
        );
        let b: UpsertLdapBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.vendor.as_deref(), Some("ad"));
        assert_eq!(b.bind_credential.as_deref(), Some("s3cret"));
        assert_eq!(b.username_attr.as_deref(), Some("sAMAccountName"));
        assert_eq!(b.search_scope, Some(1));
        assert_eq!(b.enabled, Some(false));
    }

    #[test]
    fn cols_excludes_credential() {
        // The bind credential must never appear in the column list / responses.
        assert!(!COLS.contains("credential"));
        assert!(!COLS.contains("password"));
    }

    #[test]
    fn cols_lists_connection_fields() {
        assert!(COLS.contains("connection_url"));
        assert!(COLS.contains("users_dn"));
        assert!(COLS.contains("bind_dn"));
    }

    #[test]
    fn tenant_query_deser() {
        let t = Uuid::new_v4();
        let q: TenantQuery = serde_json::from_str(&format!(r#"{{"tenant_id":"{t}"}}"#)).unwrap();
        assert_eq!(q.tenant_id, t);
    }
}

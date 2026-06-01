//! Per-tenant SAML IdP configuration API — CRUD for saml_idp_config plus a
//! read-only view of saml_user_map.
//!
//! Tenant-scoped: super_admin may manage any tenant; a tenant_admin only their
//! own tenant (enforced by `auth::require_tenant_match` on the body/query
//! tenant_id). The actual SAML assertion handling is brokered by Keycloak — this
//! service only stores the IdP registration that Keycloak provisioning consumes.
//!
//! GET    /api/v1/saml/idps?tenant_id=     — list a tenant's IdP configs
//! POST   /api/v1/saml/idps               — create/upsert an IdP config
//! GET    /api/v1/saml/idps/:id           — fetch one by id
//! DELETE /api/v1/saml/idps/:id           — remove one
//! GET    /api/v1/saml/mappings?tenant_id= — list JIT user mappings (read-only)

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
pub struct SamlIdpConfig {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub alias: String,
    pub display_name: String,
    pub entity_id: String,
    pub sso_url: String,
    pub slo_url: Option<String>,
    pub signing_cert: String,
    pub name_id_format: String,
    pub attr_email: Option<String>,
    pub attr_display_name: Option<String>,
    pub attr_given: Option<String>,
    pub attr_family: Option<String>,
    pub enabled: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SamlUserMapping {
    pub tenant_id: Uuid,
    pub idp_alias: String,
    pub saml_subject: String,
    pub user_id: Uuid,
    pub name_id_format: Option<String>,
    pub created_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct TenantQuery {
    pub tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct UpsertIdpBody {
    pub tenant_id: Uuid,
    pub alias: String,
    pub display_name: String,
    pub entity_id: String,
    pub sso_url: String,
    pub slo_url: Option<String>,
    pub signing_cert: String,
    pub name_id_format: Option<String>,
    pub attr_email: Option<String>,
    pub attr_display_name: Option<String>,
    pub attr_given: Option<String>,
    pub attr_family: Option<String>,
    pub enabled: Option<bool>,
}

const COLS: &str = "id, tenant_id, alias, display_name, entity_id, sso_url, slo_url, \
     signing_cert, name_id_format, attr_email, attr_display_name, attr_given, \
     attr_family, enabled, created_at, updated_at";

const DEFAULT_NAME_ID_FORMAT: &str = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress";

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
    let rows: Vec<SamlIdpConfig> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM saml_idp_config WHERE tenant_id = $1 ORDER BY alias"
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
    let row: Option<SamlIdpConfig> =
        sqlx::query_as(&format!("SELECT {COLS} FROM saml_idp_config WHERE id = $1"))
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    let r = row.ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
    // Authorize against the row's tenant so a tenant_admin can't read another
    // tenant's IdP by guessing the id.
    if let Some(resp) = auth::require_tenant_match(&st, &headers, r.tenant_id).await {
        return Err(resp);
    }
    Ok(Json(r).into_response())
}

pub async fn upsert(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<UpsertIdpBody>,
) -> Result<(StatusCode, Json<SamlIdpConfig>), Response> {
    if let Some(r) = auth::require_tenant_match(&st, &headers, body.tenant_id).await {
        return Err(r);
    }
    if body.alias.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "alias must not be empty").into_response());
    }
    if body.sso_url.trim().is_empty() || body.entity_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "entity_id and sso_url are required",
        )
            .into_response());
    }
    let pool = db_or_503(&st)?;
    let name_id_format = body
        .name_id_format
        .unwrap_or_else(|| DEFAULT_NAME_ID_FORMAT.to_string());
    let row: SamlIdpConfig = sqlx::query_as(&format!(
        "INSERT INTO saml_idp_config \
             (tenant_id, alias, display_name, entity_id, sso_url, slo_url, signing_cert, \
              name_id_format, attr_email, attr_display_name, attr_given, attr_family, enabled) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
         ON CONFLICT (tenant_id, alias) DO UPDATE SET \
             display_name = EXCLUDED.display_name, \
             entity_id = EXCLUDED.entity_id, \
             sso_url = EXCLUDED.sso_url, \
             slo_url = EXCLUDED.slo_url, \
             signing_cert = EXCLUDED.signing_cert, \
             name_id_format = EXCLUDED.name_id_format, \
             attr_email = EXCLUDED.attr_email, \
             attr_display_name = EXCLUDED.attr_display_name, \
             attr_given = EXCLUDED.attr_given, \
             attr_family = EXCLUDED.attr_family, \
             enabled = EXCLUDED.enabled, \
             updated_at = now() \
         RETURNING {COLS}"
    ))
    .bind(body.tenant_id)
    .bind(body.alias.trim())
    .bind(&body.display_name)
    .bind(&body.entity_id)
    .bind(&body.sso_url)
    .bind(&body.slo_url)
    .bind(&body.signing_cert)
    .bind(&name_id_format)
    .bind(&body.attr_email)
    .bind(&body.attr_display_name)
    .bind(&body.attr_given)
    .bind(&body.attr_family)
    .bind(body.enabled.unwrap_or(true))
    .fetch_one(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    tracing::info!(target: "audit",
        event = "saml.idp.upsert",
        tenant_id = %body.tenant_id,
        alias = %row.alias);
    Ok((StatusCode::OK, Json(row)))
}

pub async fn delete(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, Response> {
    let pool = db_or_503(&st)?;
    // Resolve the row's tenant first so deletion is authorized per-tenant.
    let tenant_id: Option<Uuid> =
        sqlx::query_scalar("SELECT tenant_id FROM saml_idp_config WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    let tenant_id = tenant_id.ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
    if let Some(resp) = auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Err(resp);
    }
    sqlx::query("DELETE FROM saml_idp_config WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    tracing::info!(target: "audit", event = "saml.idp.delete", id = %id, tenant_id = %tenant_id);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_mappings(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(q): Query<TenantQuery>,
) -> Result<Response, Response> {
    if let Some(r) = auth::require_tenant_match(&st, &headers, q.tenant_id).await {
        return Err(r);
    }
    let pool = db_or_503(&st)?;
    let rows: Vec<SamlUserMapping> = sqlx::query_as(
        "SELECT tenant_id, idp_alias, saml_subject, user_id, name_id_format, created_at, last_login_at \
         FROM saml_user_map WHERE tenant_id = $1 ORDER BY last_login_at DESC NULLS LAST",
    )
    .bind(q.tenant_id)
    .fetch_all(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    Ok(Json(rows).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_body_deser_minimal() {
        let t = Uuid::new_v4();
        let json = format!(
            r#"{{"tenant_id":"{t}","alias":"okta","display_name":"Okta",
                 "entity_id":"https://idp/meta","sso_url":"https://idp/sso",
                 "signing_cert":"MIIC..."}}"#
        );
        let b: UpsertIdpBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.alias, "okta");
        assert_eq!(b.tenant_id, t);
        assert!(b.slo_url.is_none());
        assert!(b.enabled.is_none());
        assert!(b.name_id_format.is_none());
    }

    #[test]
    fn upsert_body_deser_full() {
        let t = Uuid::new_v4();
        let json = format!(
            r#"{{"tenant_id":"{t}","alias":"azure","display_name":"Azure AD",
                 "entity_id":"e","sso_url":"s","slo_url":"l","signing_cert":"c",
                 "name_id_format":"urn:fmt","attr_email":"email","enabled":false}}"#
        );
        let b: UpsertIdpBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.slo_url.as_deref(), Some("l"));
        assert_eq!(b.attr_email.as_deref(), Some("email"));
        assert_eq!(b.enabled, Some(false));
        assert_eq!(b.name_id_format.as_deref(), Some("urn:fmt"));
    }

    #[test]
    fn tenant_query_deser() {
        let t = Uuid::new_v4();
        let q: TenantQuery = serde_json::from_str(&format!(r#"{{"tenant_id":"{t}"}}"#)).unwrap();
        assert_eq!(q.tenant_id, t);
    }

    #[test]
    fn default_name_id_format_is_email() {
        assert!(DEFAULT_NAME_ID_FORMAT.contains("emailAddress"));
    }

    #[test]
    fn cols_lists_all_fields() {
        assert!(COLS.contains("signing_cert"));
        assert!(COLS.contains("name_id_format"));
        assert!(COLS.contains("enabled"));
    }
}

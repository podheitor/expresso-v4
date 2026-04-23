//! Super-admin CRUD for tenants.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    audit,
    auth,
    templates::{TenantAdminEditTpl, TenantConfigTpl, TenantRow, TenantsAdminTpl},
    AdminError, AppState,
};

const PLANS:    &[&str] = &["standard", "professional", "enterprise"];
const STATUSES: &[&str] = &["active", "suspended", "cancelled"];

fn valid_slug(s: &str) -> bool {
    let bytes = s.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.iter().all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && bytes.first().map(|b| b.is_ascii_lowercase() || b.is_ascii_digit()).unwrap_or(false)
        && bytes.last().map(|b| b.is_ascii_lowercase() || b.is_ascii_digit()).unwrap_or(false)
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let rows = sqlx::query_as::<_, (uuid::Uuid, String, String, Option<String>, String, String, i64)>(
        r#"SELECT t.id, t.slug, t.name, t.cnpj, t.plan, t.status,
                  COALESCE((SELECT COUNT(*) FROM users u WHERE u.tenant_id = t.id), 0) AS user_count
             FROM tenants t
            ORDER BY t.name"#,
    ).fetch_all(pool).await.map_err(|e| AdminError(e.into()))?;

    let rows = rows.into_iter().map(|(id, slug, name, cnpj, plan, status, uc)| TenantRow {
        id: id.to_string(), slug, name,
        cnpj: cnpj.unwrap_or_default(),
        plan, status, user_count: uc,
    }).collect();
    Ok(TenantsAdminTpl { current: "tenants", rows, flash: None }.into_response())
}

pub async fn new_form(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    Ok(TenantAdminEditTpl {
        current: "tenants", id: None,
        slug: String::new(), name: String::new(), cnpj: String::new(),
        plan: "standard".into(), status: "active".into(), error: None,
    }.into_response())
}

#[derive(Deserialize)]
pub struct TenantForm {
    pub slug:   String,
    pub name:   String,
    #[serde(default)] pub cnpj:   String,
    pub plan:   String,
    pub status: String,
}

fn validate(f: &TenantForm) -> Option<String> {
    if !valid_slug(f.slug.trim()) {
        return Some("slug inválido (use a-z, 0-9, hifens; 1-63 chars)".into());
    }
    if f.name.trim().is_empty() { return Some("nome obrigatório".into()); }
    if !PLANS.contains(&f.plan.as_str()) { return Some("plano inválido".into()); }
    if !STATUSES.contains(&f.status.as_str()) { return Some("status inválido".into()); }
    None
}

pub async fn create_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(f): Form<TenantForm>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    if let Some(err) = validate(&f) {
        return Ok(TenantAdminEditTpl {
            current: "tenants", id: None,
            slug: f.slug, name: f.name, cnpj: f.cnpj,
            plan: f.plan, status: f.status, error: Some(err),
        }.into_response());
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let cnpj = if f.cnpj.trim().is_empty() { None } else { Some(f.cnpj.trim().to_string()) };
    let res = sqlx::query(
        r#"INSERT INTO tenants (slug, name, cnpj, plan, status)
           VALUES ($1, $2, $3, $4, $5)"#,
    ).bind(f.slug.trim()).bind(f.name.trim()).bind(cnpj).bind(&f.plan).bind(&f.status)
     .execute(pool).await;
    if let Err(e) = res {
        return Ok(TenantAdminEditTpl {
            current: "tenants", id: None,
            slug: f.slug, name: f.name, cnpj: f.cnpj,
            plan: f.plan, status: f.status,
            error: Some(format!("DB: {e}")),
        }.into_response());
    }
    audit::record(
        &st, &headers, &axum::http::Method::POST, "/tenants/new",
        "admin.tenant.create", Some("tenant"), None, Some(302),
        serde_json::json!({ "slug": f.slug, "name": f.name, "plan": f.plan, "status": f.status }),
    ).await;
    Ok(Redirect::to("/tenants").into_response())
}

pub async fn edit_form(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let row = sqlx::query_as::<_, (String, String, Option<String>, String, String)>(
        r#"SELECT slug, name, cnpj, plan, status FROM tenants WHERE id = $1"#,
    ).bind(id).fetch_optional(pool).await.map_err(|e| AdminError(e.into()))?;
    let Some((slug, name, cnpj, plan, status)) = row else {
        return Ok(Redirect::to("/tenants").into_response());
    };
    Ok(TenantAdminEditTpl {
        current: "tenants", id: Some(id.to_string()),
        slug, name, cnpj: cnpj.unwrap_or_default(),
        plan, status, error: None,
    }.into_response())
}

pub async fn edit_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    Form(f): Form<TenantForm>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    if let Some(err) = validate(&f) {
        return Ok(TenantAdminEditTpl {
            current: "tenants", id: Some(id.to_string()),
            slug: f.slug, name: f.name, cnpj: f.cnpj,
            plan: f.plan, status: f.status, error: Some(err),
        }.into_response());
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let cnpj = if f.cnpj.trim().is_empty() { None } else { Some(f.cnpj.trim().to_string()) };
    let res = sqlx::query(
        r#"UPDATE tenants
              SET slug = $2, name = $3, cnpj = $4, plan = $5, status = $6
            WHERE id = $1"#,
    ).bind(id).bind(f.slug.trim()).bind(f.name.trim()).bind(cnpj).bind(&f.plan).bind(&f.status)
     .execute(pool).await;
    if let Err(e) = res {
        return Ok(TenantAdminEditTpl {
            current: "tenants", id: Some(id.to_string()),
            slug: f.slug, name: f.name, cnpj: f.cnpj,
            plan: f.plan, status: f.status,
            error: Some(format!("DB: {e}")),
        }.into_response());
    }
    audit::record(
        &st, &headers, &axum::http::Method::POST, &format!("/tenants/{id}/edit"),
        "admin.tenant.update", Some("tenant"), Some(id.to_string()), Some(302),
        serde_json::json!({ "slug": f.slug, "name": f.name, "plan": f.plan, "status": f.status }),
    ).await;
    Ok(Redirect::to("/tenants").into_response())
}

pub async fn delete_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    sqlx::query("DELETE FROM tenants WHERE id = $1")
        .bind(id).execute(pool).await.map_err(|e| AdminError(e.into()))?;
    audit::record(
        &st, &headers, &axum::http::Method::POST, &format!("/tenants/{id}/delete"),
        "admin.tenant.delete", Some("tenant"), Some(id.to_string()), Some(302),
        serde_json::json!({}),
    ).await;
    Ok(Redirect::to("/tenants").into_response())
}


// ─── Config JSONB editor ─────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ConfigForm { pub config_json: String }

pub async fn config_form(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let row = sqlx::query_as::<_, (String, String, serde_json::Value)>(
        r#"SELECT slug, name, config FROM tenants WHERE id = $1"#,
    ).bind(id).fetch_optional(pool).await.map_err(|e| AdminError(e.into()))?;
    let Some((slug, name, cfg)) = row else {
        return Ok(Redirect::to("/tenants").into_response());
    };
    let pretty = serde_json::to_string_pretty(&cfg).unwrap_or_else(|_| "{}".into());
    Ok(TenantConfigTpl {
        current: "tenants", id: id.to_string(), slug, name,
        config_json: pretty, error: None, flash: None,
    }.into_response())
}

pub async fn config_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
    Form(f): Form<ConfigForm>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Ok(r); }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;

    // Parse + validate: must be a JSON object (not array/scalar).
    let parsed: serde_json::Value = match serde_json::from_str(&f.config_json) {
        Ok(v) => v,
        Err(e) => return render_config_err(id, f.config_json, format!("JSON inválido: {e}"), pool).await,
    };
    if !parsed.is_object() {
        return render_config_err(id, f.config_json, "config precisa ser um JSON object".into(), pool).await;
    }

    // Whitelist: only these top-level keys are accepted. Add new keys here
    // alongside the feature that consumes them (fail-closed policy).
    const ALLOWED_KEYS: &[&str] = &[
        "branding",        // logo_url, colors
        "features",        // feature flags (calendar, drive, chat, mail)
        "smtp",             // per-tenant SMTP override
        "quota",           // storage quotas (user_bytes, file_max_bytes)
        "retention",       // days for audit/tombstones/etc
        "locale",          // default locale + timezone
        "caldav",          // CalDAV-specific overrides
        "carddav",         // CardDAV-specific overrides
        "webmail",         // webmail UI options
        "security",        // password policy, MFA requirements
    ];
    // Snapshot keys for audit metadata (avoid dumping full potentially-sensitive config).
    let keys: Vec<String> = parsed.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    let unknown: Vec<&str> = keys.iter()
        .map(|k| k.as_str())
        .filter(|k| !ALLOWED_KEYS.contains(k))
        .collect();
    if !unknown.is_empty() {
        let msg = format!(
            "chaves desconhecidas: {} (permitidas: {})",
            unknown.join(", "),
            ALLOWED_KEYS.join(", "),
        );
        return render_config_err(id, f.config_json, msg, pool).await;
    }

    if let Err(e) = sqlx::query("UPDATE tenants SET config = $2, updated_at = NOW() WHERE id = $1")
        .bind(id).bind(&parsed).execute(pool).await
    {
        return render_config_err(id, f.config_json, format!("DB: {e}"), pool).await;
    }

    audit::record(
        &st, &headers, &axum::http::Method::POST, &format!("/tenants/{id}/config"),
        "admin.tenant.config_update", Some("tenant"), Some(id.to_string()), Some(302),
        serde_json::json!({ "keys": keys, "size_bytes": f.config_json.len() }),
    ).await;
    Ok(Redirect::to(&format!("/tenants/{id}/config")).into_response())
}

async fn render_config_err(
    id: uuid::Uuid,
    submitted: String,
    msg: String,
    pool: &sqlx::PgPool,
) -> Result<Response, AdminError> {
    // Fetch slug/name for header context; fall back gracefully.
    let (slug, name) = sqlx::query_as::<_, (String, String)>(
        r#"SELECT slug, name FROM tenants WHERE id = $1"#,
    ).bind(id).fetch_one(pool).await
        .map(|(s,n)| (s,n)).unwrap_or_default();
    Ok(TenantConfigTpl {
        current: "tenants", id: id.to_string(), slug, name,
        config_json: submitted, error: Some(msg), flash: None,
    }.into_response())
}

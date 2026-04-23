//! Drive quotas admin UI — per-tenant storage limits.

use std::sync::Arc;

use askama::Template;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Form,
};
use serde::Deserialize;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{audit, auth, AppState};

#[derive(Debug)]
pub struct QuotaRow {
    pub tenant_id: String,
    pub slug:      String,
    pub name:      String,
    pub max_mb:    String,
    pub used_mb:   String,
    pub used_pct:  String,
    pub updated:   String,
}

#[derive(Template)]
#[template(path = "drive_quotas_admin.html")]
pub struct DriveQuotasTpl {
    pub current: &'static str,
    pub rows:    Vec<QuotaRow>,
    pub flash:   Option<String>,
}

fn mb(bytes: i64) -> String {
    if bytes <= 0 { "0".into() } else { format!("{:.1}", bytes as f64 / 1_048_576.0) }
}

fn pct(used: i64, max: i64) -> String {
    if max <= 0 { "∞".into() } else { format!("{:.1}%", (used as f64 / max as f64) * 100.0) }
}

pub async fn page(
    State(st): State<Arc<AppState>>,
    headers:   HeaderMap,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let Some(pool) = st.db.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "db unavailable").into_response();
    };

    // Left join so tenants without explicit quota still appear.
    // used = SUM(drive_files.size) per tenant (best-effort; may skip deleted).
    let sql = r#"
        SELECT t.id AS tenant_id, t.slug, t.name,
               COALESCE(q.max_bytes, 0) AS max_bytes,
               COALESCE(u.used_bytes, 0) AS used_bytes,
               q.updated_at
          FROM tenants t
     LEFT JOIN drive_quotas q ON q.tenant_id = t.id
     LEFT JOIN (
            SELECT tenant_id, SUM(COALESCE(size_bytes, 0))::BIGINT AS used_bytes
              FROM drive_files
          GROUP BY tenant_id
     ) u ON u.tenant_id = t.id
         ORDER BY t.slug
    "#;
    let rows = match sqlx::query(sql).fetch_all(pool).await {
        Ok(rs) => rs.into_iter().map(|r| {
            let max = r.try_get::<i64, _>("max_bytes").unwrap_or(0);
            let used = r.try_get::<i64, _>("used_bytes").unwrap_or(0);
            QuotaRow {
                tenant_id: r.try_get::<Uuid, _>("tenant_id").map(|u| u.to_string()).unwrap_or_default(),
                slug:      r.try_get::<String, _>("slug").unwrap_or_default(),
                name:      r.try_get::<String, _>("name").unwrap_or_default(),
                max_mb:    mb(max),
                used_mb:   mb(used),
                used_pct:  pct(used, max),
                updated:   r.try_get::<Option<OffsetDateTime>, _>("updated_at").ok().flatten()
                    .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
                    .unwrap_or_else(|| "—".into()),
            }
        }).collect(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("query: {e}")).into_response(),
    };

    match (DriveQuotasTpl { current: "drivequotas", rows, flash: None }).render() {
        Ok(html) => (StatusCode::OK, [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response(),
        Err(e)   => (StatusCode::INTERNAL_SERVER_ERROR, format!("template: {e}")).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateForm {
    pub max_mb: i64,
}

pub async fn update(
    State(st):        State<Arc<AppState>>,
    headers:          HeaderMap,
    Path(tenant_id):  Path<Uuid>,
    Form(f):          Form<UpdateForm>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return r; }
    let Some(pool) = st.db.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "db unavailable").into_response();
    };
    // Clamp: 0..=10TB. 0 means "no limit" (set row to 0; UI shows ∞ when max=0).
    let max_bytes: i64 = f.max_mb.clamp(0, 10 * 1024 * 1024) * 1_048_576;

    let res = sqlx::query(
        r#"INSERT INTO drive_quotas (tenant_id, max_bytes) VALUES ($1, $2)
           ON CONFLICT (tenant_id) DO UPDATE
             SET max_bytes = EXCLUDED.max_bytes, updated_at = NOW()"#,
    )
    .bind(tenant_id)
    .bind(max_bytes)
    .execute(pool).await;
    if let Err(e) = res {
        tracing::warn!(%e, %tenant_id, "drive quota update failed");
        return Redirect::to("/drive-quotas.html?error=update-failed").into_response();
    }

    audit::record(
        &st, &headers, &Method::POST, "/drive-quotas/:tenant_id",
        "admin.drive.quota_update",
        Some("drive_quota"), Some(tenant_id.to_string()),
        Some(303),
        serde_json::json!({ "max_bytes": max_bytes, "max_mb": f.max_mb }),
    ).await;

    Redirect::to("/drive-quotas.html?updated=1").into_response()
}

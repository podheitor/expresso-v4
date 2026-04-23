//! Cross-service audit log writer (append-only).
//!
//! Writes to production `audit_log` table schema:
//!   id, tenant_id UUID NOT NULL, user_id UUID, action, resource,
//!   metadata JSONB, ip_addr INET, user_agent, status (success|failure|partial), created_at
//!
//! Richer fields (actor_email, actor_roles, http_method, http_path,
//! target_type, status_code) are folded into `metadata` JSONB so they remain
//! queryable without schema churn.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub tenant_id:   Option<Uuid>,
    /// Parsed as UUID → `user_id`. Non-UUID strings fold into metadata.
    pub actor_sub:   Option<String>,
    pub actor_email: Option<String>,
    pub actor_roles: Vec<String>,
    pub action:      String,
    pub target_type: Option<String>,
    pub target_id:   Option<String>,
    pub http_method: Option<String>,
    pub http_path:   Option<String>,
    /// HTTP status code → folded into metadata + mapped to `status` enum.
    pub status_code: Option<i16>,
    pub metadata:    JsonValue,
}

impl AuditEntry {
    pub fn new(action: impl Into<String>) -> Self {
        Self {
            tenant_id: None, actor_sub: None, actor_email: None,
            actor_roles: Vec::new(),
            action: action.into(),
            target_type: None, target_id: None,
            http_method: None, http_path: None, status_code: None,
            metadata: JsonValue::Object(Default::default()),
        }
    }
}

/// Map HTTP status → audit_log.status enum (success|failure|partial).
fn status_enum(code: Option<i16>) -> &'static str {
    match code {
        Some(c) if (200..400).contains(&c) => "success",
        Some(_) => "failure",
        None => "success",
    }
}

/// Fold rich fields not present in the table into metadata so they remain queryable.
fn enrich_metadata(e: &AuditEntry) -> JsonValue {
    let mut base = match &e.metadata {
        JsonValue::Object(_) => e.metadata.clone(),
        other => json!({ "data": other }),
    };
    let obj = base.as_object_mut().expect("metadata object");
    if let Some(v) = &e.actor_email   { obj.insert("actor_email".into(), json!(v)); }
    if !e.actor_roles.is_empty()      { obj.insert("actor_roles".into(), json!(e.actor_roles)); }
    if let Some(v) = &e.target_type   { obj.insert("target_type".into(), json!(v)); }
    if let Some(v) = &e.http_method   { obj.insert("http_method".into(), json!(v)); }
    if let Some(v) = &e.http_path     { obj.insert("http_path".into(), json!(v)); }
    if let Some(v) = e.status_code    { obj.insert("status_code".into(), json!(v)); }
    // When actor_sub is not UUID-parseable, still preserve the raw string.
    if let Some(v) = &e.actor_sub {
        if Uuid::parse_str(v).is_err() {
            obj.insert("actor_sub_raw".into(), json!(v));
        }
    }
    base
}

pub async fn record(pool: &PgPool, e: AuditEntry) -> Result<(), sqlx::Error> {
    // tenant_id now nullable (migration 20260424130000): pre-tenant events
    // (failed logins, refresh failures, system tasks) can audit without it.
    let tenant = e.tenant_id;
    let user_id = e.actor_sub.as_deref().and_then(|s| Uuid::parse_str(s).ok());
    // resource encodes target_type:target_id (both optional) for quick filtering.
    let resource = match (&e.target_type, &e.target_id) {
        (Some(t), Some(i)) => Some(format!("{t}:{i}")),
        (Some(t), None)    => Some(t.clone()),
        (None,    Some(i)) => Some(i.clone()),
        _ => None,
    };
    let metadata = enrich_metadata(&e);
    let status = status_enum(e.status_code);

    sqlx::query(
        r#"INSERT INTO audit_log
           (tenant_id, user_id, action, resource, metadata, status)
           VALUES ($1, $2, $3, $4, $5, $6)"#,
    )
    .bind(tenant)
    .bind(user_id)
    .bind(&e.action)
    .bind(resource)
    .bind(&metadata)
    .bind(status)
    .execute(pool)
    .await?;
    Ok(())
}

pub fn record_async(pool: PgPool, e: AuditEntry) {
    tokio::spawn(async move {
        if let Err(err) = record(&pool, e.clone()).await {
            tracing::warn!(error = %err, action = %e.action, "audit write failed");
        }
    });
}

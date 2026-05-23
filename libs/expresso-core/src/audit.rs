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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn status_enum_2xx_is_success() {
        assert_eq!(status_enum(Some(200)), "success");
        assert_eq!(status_enum(Some(201)), "success");
        assert_eq!(status_enum(Some(204)), "success");
        assert_eq!(status_enum(Some(302)), "success");
    }

    #[test]
    fn status_enum_4xx_5xx_is_failure() {
        assert_eq!(status_enum(Some(400)), "failure");
        assert_eq!(status_enum(Some(404)), "failure");
        assert_eq!(status_enum(Some(500)), "failure");
    }

    #[test]
    fn status_enum_none_is_success() {
        assert_eq!(status_enum(None), "success");
    }

    #[test]
    fn enrich_metadata_folds_actor_email() {
        let mut e = AuditEntry::new("test.action");
        e.actor_email = Some("user@example.com".into());
        let m = enrich_metadata(&e);
        assert_eq!(m["actor_email"], json!("user@example.com"));
    }

    #[test]
    fn enrich_metadata_folds_actor_roles() {
        let mut e = AuditEntry::new("test.action");
        e.actor_roles = vec!["admin".into(), "user".into()];
        let m = enrich_metadata(&e);
        assert_eq!(m["actor_roles"], json!(["admin", "user"]));
    }

    #[test]
    fn enrich_metadata_folds_status_code() {
        let mut e = AuditEntry::new("test.action");
        e.status_code = Some(403);
        let m = enrich_metadata(&e);
        assert_eq!(m["status_code"], json!(403));
    }

    #[test]
    fn enrich_metadata_non_uuid_actor_sub_stored_as_raw() {
        let mut e = AuditEntry::new("test.action");
        e.actor_sub = Some("not-a-uuid".into());
        let m = enrich_metadata(&e);
        assert_eq!(m["actor_sub_raw"], json!("not-a-uuid"));
    }

    #[test]
    fn enrich_metadata_uuid_actor_sub_not_stored_as_raw() {
        let mut e = AuditEntry::new("test.action");
        e.actor_sub = Some("00000000-0000-0000-0000-000000000001".into());
        let m = enrich_metadata(&e);
        assert!(m.get("actor_sub_raw").is_none());
    }

    #[test]
    fn audit_entry_action_preserved() {
        let e = AuditEntry::new("calendar.event.created");
        assert_eq!(e.action, "calendar.event.created");
    }

    #[test]
    fn audit_entry_tenant_id_none_by_default() {
        let e = AuditEntry::new("drive.file.deleted");
        assert!(e.tenant_id.is_none());
    }

    #[test]
    fn audit_entry_action_contact_deleted_preserved() {
        let e = AuditEntry::new("contact.deleted");
        assert_eq!(e.action, "contact.deleted");
    }

    #[test]
    fn audit_entry_metadata_is_empty_by_default() {
        let e = AuditEntry::new("mail.sent");
        assert!(e.metadata.is_null() || e.metadata.as_object().map(|m| m.is_empty()).unwrap_or(true));
    }

    #[test]
    fn audit_entry_actor_email_none_by_default() {
        let e = AuditEntry::new("calendar.event_created");
        assert!(e.actor_email.is_none());
    }

    #[test]
    fn audit_entry_action_is_preserved() {
        let e = AuditEntry::new("drive.file_deleted");
        assert_eq!(e.action, "drive.file_deleted");
    }

    #[test]
    fn audit_entry_actor_roles_empty_by_default() {
        let e = AuditEntry::new("mail.delivered");
        assert!(e.actor_roles.is_empty());
    }

    #[test]
    fn audit_entry_target_type_none_by_default() {
        let e = AuditEntry::new("auth.login");
        assert!(e.target_type.is_none());
    }

    #[test]
    fn audit_entry_folder_created_action_has_no_tenant() {
        let e = AuditEntry::new("drive.folder_created");
        assert!(e.tenant_id.is_none());
    }

    #[test]
    fn enrich_metadata_folds_http_method() {
        let mut e = AuditEntry::new("test.action");
        e.http_method = Some("POST".into());
        let m = enrich_metadata(&e);
        assert_eq!(m["http_method"], json!("POST"));
    }

    #[test]
    fn audit_entry_status_none_by_default() {
        let e = AuditEntry::new("drive.upload");
        assert!(e.status_code.is_none());
    }

    #[test]
    fn enrich_metadata_folds_http_path() {
        let mut e = AuditEntry::new("test.action");
        e.http_path = Some("/api/v1/events".into());
        let m = enrich_metadata(&e);
        assert_eq!(m["http_path"], json!("/api/v1/events"));
    }

    #[test]
    fn enrich_metadata_folds_target_type() {
        let mut e = AuditEntry::new("test.action");
        e.target_type = Some("drive_file".into());
        let m = enrich_metadata(&e);
        assert_eq!(m["target_type"], json!("drive_file"));
    }

    #[test]
    fn audit_entry_action_is_string_not_empty() {
        let e = AuditEntry::new("calendar.created");
        assert!(!e.action.is_empty());
    }

    #[test]
    fn enrich_metadata_omits_actor_email_when_none() {
        let e = AuditEntry::new("test.action");
        let m = enrich_metadata(&e);
        assert!(m.get("actor_email").is_none());
    }
}

//! Impersonation endpoints (SuperAdmin-gated).
//!
//! MVP: audit-only tracking. Full session swap via Keycloak admin UI
//! (`/admin/<realm>/console/#/<realm>/users/<id>/impersonate`). This module
//! records the operator's intent so the audit trail is complete; the actual
//! token issuance for the target user is delegated to Keycloak's admin REST.
//!
//! Endpoints:
//!   POST /auth/impersonate/:target_user_id  — start; audit `auth.impersonate.start`.
//!   POST /auth/impersonate/end              — stop;  audit `auth.impersonate.end`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Serialize;
use uuid::Uuid;

use expresso_auth_client::Authenticated;
use expresso_core::audit::{record_async, AuditEntry};

use std::sync::Arc;
use crate::state::AppState;

const SUPER_ROLE: &str = "superadmin";

#[derive(Debug, Serialize)]
pub struct ImpersonateResp {
    pub impersonator_sub: String,
    pub target_user_id:   Uuid,
    pub keycloak_url:     Option<String>,
    pub message:          String,
}

fn is_super(ctx: &expresso_auth_client::AuthContext) -> bool {
    ctx.roles.iter().any(|r| r.eq_ignore_ascii_case(SUPER_ROLE) || r.eq_ignore_ascii_case("super_admin") || r.eq_ignore_ascii_case("SuperAdmin"))
}

pub async fn start(
    State(st):             State<Arc<AppState>>,
    Authenticated(ctx):    Authenticated,
    Path(target_user_id):  Path<Uuid>,
) -> Result<Json<ImpersonateResp>, StatusCode> {
    if !is_super(&ctx) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Audit first — if pool absent, still return the Keycloak URL so operator
    // can act; alerting pipelines will catch missing DB separately.
    if let Some(pool) = st.pool.as_ref() {
        let entry = AuditEntry {
            tenant_id:   Some(ctx.tenant_id),
            actor_sub:   Some(ctx.user_id.to_string()),
            actor_email: Some(ctx.email.clone()),
            actor_roles: ctx.roles.clone(),
            action:      "auth.impersonate.start".into(),
            target_type: Some("user".into()),
            target_id:   Some(target_user_id.to_string()),
            http_method: Some("POST".into()),
            http_path:   Some("/auth/impersonate/:target_user_id".into()),
            status_code: Some(200),
            metadata:    serde_json::json!({
                "impersonator_email": ctx.email,
                "target_user_id":     target_user_id.to_string(),
            }),
        };
        record_async(pool.clone(), entry);
    }

    let keycloak_url = Some(st.cfg.issuer.as_str())
        .and_then(|iss| iss.trim_end_matches('/').rsplit_once("/realms/").map(|(base, realm)| {
            format!("{base}/admin/{realm}/console/#/{realm}/users/{target_user_id}/impersonate")
        }));

    Ok(Json(ImpersonateResp {
        impersonator_sub: ctx.user_id.to_string(),
        target_user_id,
        keycloak_url,
        message: "impersonation recorded; follow keycloak_url in admin console to acquire session for target user (MVP — full token-exchange pending)".into(),
    }))
}

pub async fn end(
    State(st):          State<Arc<AppState>>,
    Authenticated(ctx): Authenticated,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !is_super(&ctx) {
        return Err(StatusCode::FORBIDDEN);
    }

    if let Some(pool) = st.pool.as_ref() {
        let entry = AuditEntry {
            tenant_id:   Some(ctx.tenant_id),
            actor_sub:   Some(ctx.user_id.to_string()),
            actor_email: Some(ctx.email.clone()),
            actor_roles: ctx.roles.clone(),
            action:      "auth.impersonate.end".into(),
            target_type: None,
            target_id:   None,
            http_method: Some("POST".into()),
            http_path:   Some("/auth/impersonate/end".into()),
            status_code: Some(200),
            metadata:    serde_json::json!({ "operator_email": ctx.email }),
        };
        record_async(pool.clone(), entry);
    }

    Ok(Json(serde_json::json!({ "status": "recorded" })))
}

//! Impersonation endpoints (SuperAdmin-gated).
//!
//! Full token-exchange (RFC 8693) when a confidential exchange client is
//! configured (`KC_TOKEN_EXCHANGE_CLIENT_ID/SECRET`); otherwise falls back
//! to returning the Keycloak admin-console URL so the operator can finish
//! the swap manually. Both paths emit an audit event.
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
use crate::kc_admin::{KcAdmin, KcAdminConfig, ImpersonationTokens};
use crate::state::AppState;

const SUPER_ROLE: &str = "superadmin";

#[derive(Debug, Serialize)]
pub struct ImpersonateResp {
    pub impersonator_sub: String,
    pub target_user_id:   Uuid,
    /// Admin-console fallback URL — always populated when issuer is parseable.
    pub keycloak_url:     Option<String>,
    /// Token set for the target user when token-exchange succeeded.
    /// Caller's frontend swaps its session cookie/Bearer to these tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens:           Option<ImpersonationTokens>,
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

    let keycloak_url = Some(st.cfg.issuer.as_str())
        .and_then(|iss| iss.trim_end_matches('/').rsplit_once("/realms/").map(|(base, realm)| {
            format!("{base}/admin/{realm}/console/#/{realm}/users/{target_user_id}/impersonate")
        }));

    // Try RFC 8693 token-exchange when an exchange client is configured.
    // Falls back to admin-console URL only when KC isn't configured or the
    // call fails — keeps the operator unblocked even on partial setup.
    let (tokens, exchange_status, exchange_error) = match KcAdminConfig::from_env() {
        Some(cfg) if cfg.has_exchange_client() => {
            let kc = KcAdmin::new(cfg);
            match kc.impersonate_token(&target_user_id.to_string()).await {
                Ok(t)  => (Some(t), "exchanged", None),
                Err(e) => {
                    tracing::warn!(error=%e, target=%target_user_id, "token-exchange failed");
                    (None, "exchange_failed", Some(format!("{e}")))
                }
            }
        }
        Some(_) => (None, "exchange_client_not_configured", None),
        None    => (None, "kc_admin_not_configured", None),
    };

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
                "exchange_status":    exchange_status,
                "exchange_error":     exchange_error,
            }),
        };
        record_async(pool.clone(), entry);
    }

    let message = match exchange_status {
        "exchanged" =>
            "token-exchange succeeded; tokens issued for target user".into(),
        "exchange_failed" =>
            "token-exchange failed; follow keycloak_url in admin console (see audit log for cause)".into(),
        "exchange_client_not_configured" =>
            "KC_TOKEN_EXCHANGE_CLIENT_ID/SECRET not set; follow keycloak_url in admin console".into(),
        _ /* kc_admin_not_configured */ =>
            "KC_ADMIN_* not configured; impersonation recorded only (no Keycloak interaction)".into(),
    };

    Ok(Json(ImpersonateResp {
        impersonator_sub: ctx.user_id.to_string(),
        target_user_id,
        keycloak_url,
        tokens,
        message,
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

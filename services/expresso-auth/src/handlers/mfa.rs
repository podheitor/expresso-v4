//! MFA administration endpoints (SuperAdmin-gated).
//!
//! Thin admin surface over Keycloak: the enrollment crypto (TOTP secrets,
//! WebAuthn attestation) lives entirely in Keycloak's own flows. These
//! endpoints let an operator inspect, require, and reset a user's MFA factors
//! without opening the Keycloak admin console.
//!
//! Endpoints:
//!   GET    /auth/admin/users/:id/mfa              — list MFA factors.
//!   POST   /auth/admin/users/:id/mfa/require      — email a CONFIGURE_TOTP /
//!          webauthn-register required-action so the user enrolls on next login.
//!   DELETE /auth/admin/users/:id/mfa/:credential_id — remove one factor (reset).
//!
//! All three require the `superadmin` role and emit an audit event.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use expresso_auth_client::{AuthContext, Authenticated};
use expresso_core::audit::{record_async, AuditEntry};

use crate::kc_admin::{KcAdmin, KcAdminConfig, KcCredential};
use crate::state::AppState;

const SUPER_ROLE: &str = "superadmin";

/// Keycloak credential `type` values that are MFA factors (everything else,
/// e.g. `password`, is filtered out of the MFA view).
const MFA_TYPES: &[&str] = &["otp", "webauthn", "webauthn-passwordless"];

/// Default lifespan for the required-action email link (12h), matching the
/// password-reset flow's generous window so the user has time to act.
const REQUIRE_LIFESPAN_SECS: u32 = 12 * 3600;

fn is_super(ctx: &AuthContext) -> bool {
    ctx.roles.iter().any(|r| {
        r.eq_ignore_ascii_case(SUPER_ROLE)
            || r.eq_ignore_ascii_case("super_admin")
            || r.eq_ignore_ascii_case("SuperAdmin")
    })
}

/// Map a missing/failed Keycloak admin config or call to a status code. KC not
/// configured → 503 (the feature needs it); any call failure → 502 (upstream).
fn kc_admin() -> Result<KcAdmin, StatusCode> {
    KcAdminConfig::from_env()
        .map(KcAdmin::new)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

#[derive(Debug, Serialize)]
pub struct MfaListResp {
    pub user_id: Uuid,
    pub factors: Vec<KcCredential>,
}

pub async fn list(
    st: State<Arc<AppState>>,
    auth: Authenticated,
    target: Path<Uuid>,
) -> Result<Json<MfaListResp>, StatusCode> {
    let started = Instant::now();
    let r = list_inner(st, auth, target).await;
    crate::metrics::record_result(crate::metrics::OP_MFA_LIST, started, &r);
    r
}

async fn list_inner(
    State(st): State<Arc<AppState>>,
    Authenticated(ctx): Authenticated,
    Path(user_id): Path<Uuid>,
) -> Result<Json<MfaListResp>, StatusCode> {
    if !is_super(&ctx) {
        return Err(StatusCode::FORBIDDEN);
    }
    let kc = kc_admin()?;
    let all = kc
        .list_credentials(&user_id.to_string())
        .await
        .map_err(|e| {
            tracing::warn!(error=%e, target=%user_id, "kc list-credentials failed");
            StatusCode::BAD_GATEWAY
        })?;
    let factors: Vec<KcCredential> = all
        .into_iter()
        .filter(|c| MFA_TYPES.contains(&c.credential_type.as_str()))
        .collect();
    audit(
        &st,
        &ctx,
        "auth.mfa.list",
        user_id,
        serde_json::json!({ "factor_count": factors.len() }),
    );
    Ok(Json(MfaListResp { user_id, factors }))
}

#[derive(Debug, Deserialize)]
pub struct RequireBody {
    /// Which factor to require. `"totp"` → `CONFIGURE_TOTP`;
    /// `"webauthn"` → `webauthn-register`. Defaults to `"totp"`.
    #[serde(default)]
    pub factor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RequireResp {
    pub user_id: Uuid,
    pub action: String,
    pub message: String,
}

pub async fn require(
    st: State<Arc<AppState>>,
    auth: Authenticated,
    target: Path<Uuid>,
    body: Option<Json<RequireBody>>,
) -> Result<Json<RequireResp>, StatusCode> {
    let started = Instant::now();
    let r = require_inner(st, auth, target, body).await;
    crate::metrics::record_result(crate::metrics::OP_MFA_REQUIRE, started, &r);
    r
}

async fn require_inner(
    State(st): State<Arc<AppState>>,
    Authenticated(ctx): Authenticated,
    Path(user_id): Path<Uuid>,
    body: Option<Json<RequireBody>>,
) -> Result<Json<RequireResp>, StatusCode> {
    if !is_super(&ctx) {
        return Err(StatusCode::FORBIDDEN);
    }
    let factor = body
        .and_then(|b| b.0.factor)
        .unwrap_or_else(|| "totp".to_string());
    let action = match factor.to_ascii_lowercase().as_str() {
        "totp" | "otp" => "CONFIGURE_TOTP",
        "webauthn" => "webauthn-register",
        other => {
            tracing::warn!(factor = %other, "mfa require: unknown factor");
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    let kc = kc_admin()?;
    kc.execute_actions_email(&user_id.to_string(), &[action], REQUIRE_LIFESPAN_SECS)
        .await
        .map_err(|e| {
            tracing::warn!(error=%e, target=%user_id, action, "kc require-action email failed");
            StatusCode::BAD_GATEWAY
        })?;

    audit(
        &st,
        &ctx,
        "auth.mfa.require",
        user_id,
        serde_json::json!({ "action": action }),
    );

    Ok(Json(RequireResp {
        user_id,
        action: action.to_string(),
        message: format!("{action} required-action email sent"),
    }))
}

pub async fn delete(
    st: State<Arc<AppState>>,
    auth: Authenticated,
    path: Path<(Uuid, String)>,
) -> Result<StatusCode, StatusCode> {
    let started = Instant::now();
    let r = delete_inner(st, auth, path).await;
    crate::metrics::record_result(crate::metrics::OP_MFA_DELETE, started, &r);
    r
}

async fn delete_inner(
    State(st): State<Arc<AppState>>,
    Authenticated(ctx): Authenticated,
    Path((user_id, credential_id)): Path<(Uuid, String)>,
) -> Result<StatusCode, StatusCode> {
    if !is_super(&ctx) {
        return Err(StatusCode::FORBIDDEN);
    }
    let kc = kc_admin()?;
    kc.delete_credential(&user_id.to_string(), &credential_id)
        .await
        .map_err(|e| {
            tracing::warn!(error=%e, target=%user_id, credential=%credential_id,
                "kc delete-credential failed");
            StatusCode::BAD_GATEWAY
        })?;

    audit(
        &st,
        &ctx,
        "auth.mfa.delete",
        user_id,
        serde_json::json!({ "credential_id": credential_id }),
    );

    Ok(StatusCode::NO_CONTENT)
}

/// Record an MFA admin action to the audit log (best-effort; no-op without a
/// DB pool, mirroring the impersonate handler).
fn audit(
    st: &AppState,
    ctx: &AuthContext,
    action: &str,
    target: Uuid,
    metadata: serde_json::Value,
) {
    let Some(pool) = st.pool.as_ref() else {
        return;
    };
    let entry = AuditEntry {
        tenant_id: Some(ctx.tenant_id),
        actor_sub: Some(ctx.user_id.to_string()),
        actor_email: Some(ctx.email.clone()),
        actor_roles: ctx.roles.clone(),
        action: action.into(),
        target_type: Some("user".into()),
        target_id: Some(target.to_string()),
        http_method: None,
        http_path: None,
        status_code: Some(200),
        metadata,
    };
    record_async(pool.clone(), entry);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_roles(roles: &[&str]) -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            email: "x@ex.com".into(),
            display_name: "X".into(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
            expires_at: 9999999999,
            acr: None,
            amr: vec![],
            govbr_cpf_hash: None,
            govbr_confiabilidades: vec![],
            identity_provider: None,
            ldap_id: None,
        }
    }

    #[test]
    fn is_super_true_for_superadmin() {
        assert!(is_super(&ctx_with_roles(&["superadmin"])));
    }

    #[test]
    fn is_super_false_for_regular_admin() {
        assert!(!is_super(&ctx_with_roles(&["admin", "tenantAdmin"])));
    }

    #[test]
    fn is_super_false_for_empty_roles() {
        assert!(!is_super(&ctx_with_roles(&[])));
    }

    #[test]
    fn mfa_types_cover_otp_and_webauthn() {
        assert!(MFA_TYPES.contains(&"otp"));
        assert!(MFA_TYPES.contains(&"webauthn"));
        assert!(MFA_TYPES.contains(&"webauthn-passwordless"));
    }

    #[test]
    fn mfa_types_exclude_password() {
        assert!(!MFA_TYPES.contains(&"password"));
    }

    #[test]
    fn require_lifespan_is_twelve_hours() {
        assert_eq!(REQUIRE_LIFESPAN_SECS, 43200);
    }

    #[test]
    fn require_body_factor_optional() {
        let b: RequireBody = serde_json::from_str("{}").unwrap();
        assert!(b.factor.is_none());
    }

    #[test]
    fn require_body_factor_parsed() {
        let b: RequireBody = serde_json::from_str(r#"{"factor":"webauthn"}"#).unwrap();
        assert_eq!(b.factor.as_deref(), Some("webauthn"));
    }

    #[test]
    fn super_role_constant_is_superadmin() {
        assert_eq!(SUPER_ROLE, "superadmin");
    }
}

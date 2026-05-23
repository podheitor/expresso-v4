//! Password-reset self-service.
//!
//! `POST /auth/forgot {"email": "..."}` — always returns 204 (no user-existence
//! leak). If the email matches a realm user, KC is instructed to send an
//! `UPDATE_PASSWORD` action email via its configured SMTP.
//!
//! No local token state: Keycloak owns the reset token + landing page.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use tracing::{info, warn};

use expresso_core::audit::{record_async, AuditEntry};

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ForgotReq {
    pub email: String,
}

const ACTION_LIFESPAN_SECS: u32 = 3600; // 1h

pub async fn forgot(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ForgotReq>,
) -> StatusCode {
    let email = req.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        // Still return 204 to avoid probing.
        return StatusCode::NO_CONTENT;
    }

    let Some(kc_cfg) = crate::kc_admin::KcAdminConfig::from_env() else {
        warn!("password-reset requested but KC_ADMIN_* env not set; returning 204 (no-op)");
        return StatusCode::NO_CONTENT;
    };
    let kc = crate::kc_admin::KcAdmin::new(kc_cfg);

    match kc.user_id_by_email(&email).await {
        Ok(Some(uid)) => {
            match kc.execute_actions_email(&uid, &["UPDATE_PASSWORD"], ACTION_LIFESPAN_SECS).await {
                Ok(()) => {
                    info!(user_id = %uid, "password reset email dispatched");
                    if let Some(pool) = state.pool.as_ref() {
                        let mut e = AuditEntry::new("auth.password_reset.requested");
                        e.actor_email = Some(email.clone());
                        e.target_type = Some("kc_user".into());
                        e.target_id   = Some(uid);
                        record_async(pool.clone(), e);
                    }
                }
                Err(e) => {
                    warn!(error = %e, "execute_actions_email failed");
                }
            }
        }
        Ok(None) => {
            info!(email = %email, "password reset: no user found (silent 204)");
        }
        Err(e) => {
            warn!(error = %e, "user_id_by_email failed");
        }
    }

    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgot_req_deser() {
        let json = r#"{"email":"heitor@ex.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "heitor@ex.com");
    }

    #[test]
    fn action_lifespan_is_one_hour() {
        assert_eq!(ACTION_LIFESPAN_SECS, 3600);
    }

    #[test]
    fn forgot_req_email_trimmed_at_handler_level() {
        // The struct just stores raw input; trimming happens in the handler.
        // Verify round-trip with whitespace in JSON.
        let json = r#"{"email":"  spaces@ex.com  "}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "  spaces@ex.com  ");
    }

    #[test]
    fn forgot_req_missing_email_fails_deser() {
        let json = r#"{}"#;
        let r = serde_json::from_str::<ForgotReq>(json);
        assert!(r.is_err());
    }

    #[test]
    fn action_lifespan_secs_is_u32() {
        let _: u32 = ACTION_LIFESPAN_SECS;
        assert!(ACTION_LIFESPAN_SECS > 0);
    }

    #[test]
    fn forgot_req_email_roundtrip() {
        let json = r#"{"email":"user@example.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "user@example.com");
    }

    #[test]
    fn action_lifespan_is_3600() {
        assert_eq!(ACTION_LIFESPAN_SECS, 3600);
    }

    #[test]
    fn forgot_req_unicode_email() {
        let json = r#"{"email":"usuário@exemplo.com.br"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert!(r.email.contains("usuário"));
    }

    #[test]
    fn forgot_req_long_email_stored() {
        let email = format!("{}@example.com", "a".repeat(60));
        let json = format!(r#"{{"email":"{email}"}}"#);
        let r: ForgotReq = serde_json::from_str(&json).unwrap();
        assert_eq!(r.email, email);
    }

    #[test]
    fn forgot_req_email_with_plus_tag() {
        let json = r#"{"email":"user+tag@example.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "user+tag@example.com");
    }

    #[test]
    fn forgot_req_unicode_email_preserved() {
        let json = r#"{"email":"üser@example.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "üser@example.com");
    }

    #[test]
    fn forgot_req_subdomain_email_preserved() {
        let json = r#"{"email":"user@sub.domain.example.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "user@sub.domain.example.com");
    }

    #[test]
    fn forgot_req_email_not_empty_after_roundtrip() {
        let json = r#"{"email":"nonempty@host.org"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert!(!r.email.is_empty());
    }

    #[test]
    fn forgot_req_email_with_plus_tag_preserved() {
        let json = r#"{"email":"user+tag@example.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "user+tag@example.com");
    }

    #[test]
    fn forgot_req_email_unicode_domain() {
        let json = r#"{"email":"user@büro.example"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert!(r.email.contains("büro"));
    }

    #[test]
    fn action_lifespan_equals_one_hour_in_seconds() {
        assert_eq!(ACTION_LIFESPAN_SECS, 60 * 60);
    }

    #[test]
    fn forgot_req_email_field_name_matches_json_key() {
        let json = r#"{"email":"check@domain.io"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert_eq!(r.email, "check@domain.io");
    }

    #[test]
    fn action_lifespan_is_positive() {
        assert!(ACTION_LIFESPAN_SECS > 0);
    }

    #[test]
    fn forgot_req_email_does_not_contain_newline() {
        let json = r#"{"email":"user@example.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert!(!r.email.contains('\n'));
    }

    #[test]
    fn forgot_req_email_does_not_contain_carriage_return() {
        let json = r#"{"email":"user@example.com"}"#;
        let r: ForgotReq = serde_json::from_str(json).unwrap();
        assert!(!r.email.contains('\r'));
    }
}

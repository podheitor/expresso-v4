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

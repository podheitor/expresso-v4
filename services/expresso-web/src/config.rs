//! Upstream backend URLs — mirror vite.config dev proxy.

use std::env;

fn envs(k: &str) -> Option<String> {
    env::var(k).ok().filter(|v| !v.trim().is_empty())
}

#[derive(Debug, Clone)]
pub struct Backends {
    pub auth:     String,
    pub mail:     String,
    pub calendar: String,
    pub contacts: String,
    pub drive:    String,
}

impl Backends {
    pub fn from_env() -> Self {
        Self {
            auth:     envs("BACKEND__AUTH").unwrap_or_else(|| "http://localhost:8012".into()),
            mail:     envs("BACKEND__MAIL").unwrap_or_else(|| "http://localhost:8001".into()),
            calendar: envs("BACKEND__CALENDAR").unwrap_or_else(|| "http://localhost:8002".into()),
            contacts: envs("BACKEND__CONTACTS").unwrap_or_else(|| "http://localhost:8003".into()),
            drive:    envs("BACKEND__DRIVE").unwrap_or_else(|| "http://localhost:8004".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Public {
    /// URL do console de autoatendimento Keycloak (link WebAuthn).
    pub kc_account: String,
    /// Caminho público do endpoint login upstream (AUTH rp).
    pub auth_login_path: String,
    pub auth_logout_path: String,
}

impl Public {
    pub fn from_env() -> Self {
        Self {
            kc_account: envs("PUBLIC__KC_ACCOUNT")
                .unwrap_or_else(|| "/auth/realms/expresso/account/#/security/signingin".into()),
            auth_login_path:  envs("PUBLIC__AUTH_LOGIN").unwrap_or_else(|| "/auth/login".into()),
            auth_logout_path: envs("PUBLIC__AUTH_LOGOUT").unwrap_or_else(|| "/auth/logout".into()),
        }
    }
}

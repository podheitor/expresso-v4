//! Relying Party configuration resolved from env at startup.

use std::time::Duration;

/// OIDC RP config. Public client (PKCE); no client_secret needed.
#[derive(Debug, Clone)]
pub struct RpConfig {
    /// Full issuer URL, e.g. `http://192.168.15.125:8080/realms/expresso`.
    pub issuer:        String,
    /// Keycloak client_id — must be public + direct-access enabled.
    pub client_id:     String,
    /// Callback registered at Keycloak (`/auth/callback`).
    pub redirect_uri:  String,
    /// Optional post-logout landing page.
    pub post_logout_redirect_uri: Option<String>,
    /// Pending-login state TTL.
    pub state_ttl:     Duration,
    /// HTTP timeout for token / discovery calls.
    pub http_timeout:  Duration,
}

impl RpConfig {
    /// Build from env. Required: `AUTH_RP__ISSUER`, `AUTH_RP__CLIENT_ID`,
    /// `AUTH_RP__REDIRECT_URI`. Optional: `AUTH_RP__POST_LOGOUT_REDIRECT_URI`.
    pub fn from_env() -> anyhow::Result<Self> {
        let issuer       = req("AUTH_RP__ISSUER")?;
        let client_id    = req("AUTH_RP__CLIENT_ID")?;
        let redirect_uri = req("AUTH_RP__REDIRECT_URI")?;
        let post_logout  = std::env::var("AUTH_RP__POST_LOGOUT_REDIRECT_URI").ok();
        Ok(Self {
            issuer,
            client_id,
            redirect_uri,
            post_logout_redirect_uri: post_logout,
            state_ttl:    Duration::from_secs(600),
            http_timeout: Duration::from_secs(5),
        })
    }
}

fn req(key: &str) -> anyhow::Result<String> {
    std::env::var(key).map_err(|_| anyhow::anyhow!("missing env var: {}", key))
}

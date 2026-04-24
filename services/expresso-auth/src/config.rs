//! Relying Party configuration resolved from env at startup.

use std::time::Duration;

/// OIDC RP config. Public client (PKCE); no client_secret needed.
#[derive(Debug, Clone)]
pub struct RpConfig {
    /// Full issuer URL for **single-realm** (fallback) mode.
    pub issuer:        String,
    /// Keycloak client_id (same id must exist in every realm when multi-realm).
    pub client_id:     String,
    /// Callback URI for single-realm fallback.
    pub redirect_uri:  String,
    /// Optional post-logout landing page (single-realm).
    pub post_logout_redirect_uri: Option<String>,
    /// Pending-login state TTL.
    pub state_ttl:     Duration,
    /// HTTP timeout for token / discovery calls.
    pub http_timeout:  Duration,

    // ─── Multi-realm (opt-in, fase 46) ───
    /// When set, enables per-Host realm routing. E.g.:
    /// `http://auth.example.com:8080/realms/{realm}`.
    pub issuer_template:       Option<String>,
    /// Template for redirect_uri with `{host}` placeholder. E.g.:
    /// `https://{host}/auth/callback`. When unset, `redirect_uri` is reused.
    pub redirect_uri_template: Option<String>,
    /// Template for post_logout with `{host}` placeholder. Optional.
    pub post_logout_template:  Option<String>,
}

impl RpConfig {
    /// Build from env. Required: `AUTH_RP__ISSUER`, `AUTH_RP__CLIENT_ID`,
    /// `AUTH_RP__REDIRECT_URI`. Optional multi-realm:
    /// `AUTH_RP__ISSUER_TEMPLATE`, `AUTH_RP__REDIRECT_URI_TEMPLATE`,
    /// `AUTH_RP__POST_LOGOUT_TEMPLATE`.
    pub fn from_env() -> anyhow::Result<Self> {
        let issuer       = req("AUTH_RP__ISSUER")?;
        let client_id    = req("AUTH_RP__CLIENT_ID")?;
        let redirect_uri = req("AUTH_RP__REDIRECT_URI")?;
        let post_logout  = std::env::var("AUTH_RP__POST_LOGOUT_REDIRECT_URI").ok();
        let issuer_template       = std::env::var("AUTH_RP__ISSUER_TEMPLATE").ok().filter(|v| !v.is_empty());
        let redirect_uri_template = std::env::var("AUTH_RP__REDIRECT_URI_TEMPLATE").ok().filter(|v| !v.is_empty());
        let post_logout_template  = std::env::var("AUTH_RP__POST_LOGOUT_TEMPLATE").ok().filter(|v| !v.is_empty());
        Ok(Self {
            issuer,
            client_id,
            redirect_uri,
            post_logout_redirect_uri: post_logout,
            state_ttl:    Duration::from_secs(600),
            http_timeout: Duration::from_secs(5),
            issuer_template,
            redirect_uri_template,
            post_logout_template,
        })
    }
}

fn req(key: &str) -> anyhow::Result<String> {
    std::env::var(key).map_err(|_| anyhow::anyhow!("missing env var: {}", key))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(
        issuer: &str,
        client_id: &str,
        redirect_uri: &str,
        issuer_template: Option<&str>,
        redirect_uri_template: Option<&str>,
        post_logout_template: Option<&str>,
    ) -> RpConfig {
        RpConfig {
            issuer:                   issuer.into(),
            client_id:                client_id.into(),
            redirect_uri:             redirect_uri.into(),
            post_logout_redirect_uri: None,
            state_ttl:                Duration::from_secs(600),
            http_timeout:             Duration::from_secs(5),
            issuer_template:          issuer_template.map(|s| s.into()),
            redirect_uri_template:    redirect_uri_template.map(|s| s.into()),
            post_logout_template:     post_logout_template.map(|s| s.into()),
        }
    }

    #[test]
    fn redirect_uri_template_is_stored() {
        let c = cfg("https://kc/realms/r", "cli", "https://app/cb",
                    None, Some("https://{host}/auth/callback"), None);
        assert_eq!(c.redirect_uri_template.as_deref(), Some("https://{host}/auth/callback"));
    }

    #[test]
    fn issuer_template_is_stored() {
        let c = cfg("https://kc/realms/r", "cli", "https://app/cb",
                    Some("https://kc/realms/{realm}"), None, None);
        assert_eq!(c.issuer_template.as_deref(), Some("https://kc/realms/{realm}"));
    }

    #[test]
    fn defaults_when_no_templates() {
        let c = cfg("https://kc/realms/r", "cli", "https://app/cb", None, None, None);
        assert!(c.issuer_template.is_none());
        assert!(c.redirect_uri_template.is_none());
        assert!(c.post_logout_template.is_none());
    }

    #[test]
    fn state_ttl_default_is_ten_minutes() {
        let c = cfg("i", "c", "r", None, None, None);
        assert_eq!(c.state_ttl.as_secs(), 600);
    }

    #[test]
    fn http_timeout_default_is_five_seconds() {
        let c = cfg("i", "c", "r", None, None, None);
        assert_eq!(c.http_timeout.as_secs(), 5);
    }

    #[test]
    fn issuer_field_preserved() {
        let c = cfg("https://kc/realms/demo", "cli", "https://app/cb", None, None, None);
        assert_eq!(c.issuer, "https://kc/realms/demo");
        assert_eq!(c.client_id, "cli");
    }

    #[test]
    fn redirect_uri_template_stored_when_provided() {
        let c = cfg("i", "cl", "r", Some("https://{host}/cb".into()), None, None);
        assert_eq!(c.redirect_uri_template.as_deref(), Some("https://{host}/cb"));
    }

    #[test]
    fn redirect_uri_field_preserved() {
        let c = cfg("i", "cli", "https://app/callback", None, None, None);
        assert_eq!(c.redirect_uri, "https://app/callback");
    }

    #[test]
    fn redirect_uri_template_none_when_omitted() {
        let c = cfg("i", "cli", "https://app/callback", None, None, None);
        assert!(c.redirect_uri_template.is_none());
    }

    #[test]
    fn redirect_uri_preserved() {
        let c = cfg("i", "cli", "https://app/callback", None, None, None);
        assert_eq!(c.redirect_uri, "https://app/callback");
    }

    #[test]
    fn client_id_preserved() {
        let c = cfg("my-issuer", "my-client", "https://app/cb", None, None, None);
        assert_eq!(c.client_id, "my-client");
    }

    #[test]
    fn redirect_uri_full_url_preserved() {
        let c = cfg("issuer", "client", "https://app.example.com/callback", None, None, None);
        assert_eq!(c.redirect_uri, "https://app.example.com/callback");
    }

    #[test]
    fn issuer_template_none_when_not_provided() {
        let c = cfg("iss", "cid", "https://cb", None, None, None);
        assert!(c.issuer_template.is_none());
    }

    #[test]
    fn issuer_template_some_when_provided() {
        let c = cfg("iss", "cid", "https://cb", Some("http://kc/{realm}"), None, None);
        assert!(c.issuer_template.is_some());
    }

    #[test]
    fn client_id_field_preserved() {
        let c = cfg("https://iss", "my-client", "https://cb", None, None, None);
        assert_eq!(c.client_id, "my-client");
    }

    #[test]
    fn post_logout_template_stored_when_provided() {
        let c = cfg("i", "c", "r", None, None, Some("https://{host}/bye"));
        assert_eq!(c.post_logout_template.as_deref(), Some("https://{host}/bye"));
    }

    #[test]
    fn post_logout_redirect_uri_none_when_absent() {
        let c = cfg("https://kc/realms/r", "my-client", "https://app/cb", None, None, None);
        assert!(c.post_logout_redirect_uri.is_none());
    }

    #[test]
    fn state_ttl_is_greater_than_zero() {
        let c = cfg("i", "c", "r", None, None, None);
        assert!(c.state_ttl.as_secs() > 0);
    }

    #[test]
    fn http_timeout_is_greater_than_zero() {
        let c = cfg("i", "c", "r", None, None, None);
        assert!(c.http_timeout.as_secs() > 0);
    }

    #[test]
    fn post_logout_template_none_when_not_provided() {
        let c = cfg("https://kc/realms/r", "client-id", "https://app/cb", None, None, None);
        assert!(c.post_logout_template.is_none());
    }
}

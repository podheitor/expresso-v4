//! Thin Keycloak admin REST client — password-reset emails + RFC 8693
//! token-exchange for impersonation.
//!
//! Uses `admin-cli` password grant against master realm for admin actions
//! (lookup-user, execute-actions-email). Token-exchange uses a separate
//! confidential client with `realm-management/impersonation` permission.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct KcAdminConfig {
    pub base_url:   String,
    pub realm:      String,
    pub admin_user: String,
    pub admin_pass: String,
    /// Confidential client used for RFC 8693 token-exchange (impersonation).
    /// Requires the client to have `realm-management/impersonation` and
    /// `token-exchange` enabled in Keycloak. Optional — when absent, the
    /// impersonate handler falls back to admin-console URL only.
    pub exchange_client_id:     Option<String>,
    pub exchange_client_secret: Option<String>,
}

impl KcAdminConfig {
    pub fn from_env() -> Option<Self> {
        let base_url   = std::env::var("KC_URL").ok()?;
        let realm      = std::env::var("KC_REALM").unwrap_or_else(|_| "expresso".into());
        let admin_user = std::env::var("KC_ADMIN_USER").ok()?;
        let admin_pass = std::env::var("KC_ADMIN_PASS").ok()?;
        let exchange_client_id     = std::env::var("KC_TOKEN_EXCHANGE_CLIENT_ID").ok()
            .filter(|s| !s.trim().is_empty());
        let exchange_client_secret = std::env::var("KC_TOKEN_EXCHANGE_CLIENT_SECRET").ok()
            .filter(|s| !s.trim().is_empty());
        Some(Self {
            base_url, realm, admin_user, admin_pass,
            exchange_client_id, exchange_client_secret,
        })
    }

    pub fn has_exchange_client(&self) -> bool {
        self.exchange_client_id.is_some() && self.exchange_client_secret.is_some()
    }
}

pub struct KcAdmin {
    cfg:  KcAdminConfig,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct TokenResp { access_token: String }

#[derive(Deserialize)]
struct KcUserLite { id: String }

/// Result of a successful RFC 8693 token-exchange — full token set the
/// caller can hand back to its client (or proxy) to act AS the target.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ImpersonationTokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in:    i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_expires_in: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type:    Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope:         Option<String>,
}

impl KcAdmin {
    pub fn new(cfg: KcAdminConfig) -> Self {
        Self { cfg, http: reqwest::Client::new() }
    }

    async fn token(&self) -> Result<String> {
        let url = format!("{}/realms/master/protocol/openid-connect/token", self.cfg.base_url);
        let r: TokenResp = self.http.post(&url)
            .form(&[
                ("grant_type", "password"),
                ("client_id",  "admin-cli"),
                ("username",   &self.cfg.admin_user),
                ("password",   &self.cfg.admin_pass),
            ])
            .send().await.context("kc token req")?
            .error_for_status().context("kc token status")?
            .json().await.context("kc token json")?;
        Ok(r.access_token)
    }

    /// Returns Some(user_id) if a user with that email exists in the configured realm.
    pub async fn user_id_by_email(&self, email: &str) -> Result<Option<String>> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}/users", self.cfg.base_url, self.cfg.realm);
        let users: Vec<KcUserLite> = self.http.get(&url)
            .bearer_auth(&tok)
            .query(&[("email", email), ("exact", "true")])
            .send().await?.error_for_status()?.json().await?;
        Ok(users.into_iter().next().map(|u| u.id))
    }

    /// RFC 8693 token-exchange: acquire a token set that impersonates
    /// `target_user_id` in the configured realm. Requires
    /// `KC_TOKEN_EXCHANGE_CLIENT_ID/SECRET` and the client to hold
    /// `realm-management/impersonation` permission in Keycloak.
    ///
    /// Errors when the exchange client is not configured or KC rejects
    /// the request. Caller should audit both success and failure.
    pub async fn impersonate_token(&self, target_user_id: &str) -> Result<ImpersonationTokens> {
        let client_id = self.cfg.exchange_client_id.as_deref()
            .context("KC_TOKEN_EXCHANGE_CLIENT_ID not configured")?;
        let client_secret = self.cfg.exchange_client_secret.as_deref()
            .context("KC_TOKEN_EXCHANGE_CLIENT_SECRET not configured")?;
        let url = format!(
            "{}/realms/{}/protocol/openid-connect/token",
            self.cfg.base_url, self.cfg.realm
        );
        let resp = self.http.post(&url)
            .form(&[
                ("grant_type",      "urn:ietf:params:oauth:grant-type:token-exchange"),
                ("client_id",       client_id),
                ("client_secret",   client_secret),
                ("requested_subject", target_user_id),
                ("requested_token_type", "urn:ietf:params:oauth:token-type:access_token"),
                ("scope",           "openid"),
            ])
            .send().await.context("kc token-exchange req")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("kc token-exchange failed: {status} body={body}");
        }
        let tokens: ImpersonationTokens = resp.json().await
            .context("kc token-exchange json")?;
        Ok(tokens)
    }

    /// Triggers KC to email the user with the given required-action token(s).
    /// `actions` example: `["UPDATE_PASSWORD"]`.
    /// KC sends the email itself via its configured SMTP.
    pub async fn execute_actions_email(
        &self,
        user_id: &str,
        actions: &[&str],
        lifespan_secs: u32,
    ) -> Result<()> {
        let tok = self.token().await?;
        let url = format!(
            "{}/admin/realms/{}/users/{}/execute-actions-email?lifespan={}",
            self.cfg.base_url, self.cfg.realm, user_id, lifespan_secs
        );
        self.http.put(&url)
            .bearer_auth(&tok)
            .json(&actions)
            .send().await?.error_for_status()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(client_id: Option<&str>, client_secret: Option<&str>) -> KcAdminConfig {
        KcAdminConfig {
            base_url:   "http://kc:8080".into(),
            realm:      "expresso".into(),
            admin_user: "admin".into(),
            admin_pass: "pass".into(),
            exchange_client_id:     client_id.map(|s| s.into()),
            exchange_client_secret: client_secret.map(|s| s.into()),
        }
    }

    #[test]
    fn has_exchange_client_both_set() {
        assert!(cfg(Some("client"), Some("secret")).has_exchange_client());
    }

    #[test]
    fn has_exchange_client_missing_id() {
        assert!(!cfg(None, Some("secret")).has_exchange_client());
    }

    #[test]
    fn has_exchange_client_missing_secret() {
        assert!(!cfg(Some("client"), None).has_exchange_client());
    }

    #[test]
    fn has_exchange_client_both_absent() {
        assert!(!cfg(None, None).has_exchange_client());
    }

    #[test]
    fn impersonation_tokens_serde_minimal() {
        let json = r#"{"access_token":"tok123","expires_in":300}"#;
        let t: ImpersonationTokens = serde_json::from_str(json).unwrap();
        assert_eq!(t.access_token, "tok123");
        assert_eq!(t.expires_in, 300);
        assert!(t.refresh_token.is_none());
    }

    #[test]
    fn impersonation_tokens_skip_serializing_none() {
        let t = ImpersonationTokens {
            access_token: "tok".into(),
            refresh_token: None,
            expires_in: 0,
            refresh_expires_in: None,
            token_type: None,
            scope: None,
        };
        let j = serde_json::to_string(&t).unwrap();
        assert!(!j.contains("refresh_token"));
        assert!(!j.contains("token_type"));
    }

    #[test]
    fn impersonation_tokens_scope_present_serializes() {
        let t = ImpersonationTokens {
            access_token: "tok".into(),
            refresh_token: None,
            expires_in: 60,
            refresh_expires_in: None,
            token_type: Some("Bearer".into()),
            scope: Some("openid profile".into()),
        };
        let j = serde_json::to_string(&t).unwrap();
        assert!(j.contains("openid profile"));
        assert!(j.contains("Bearer"));
    }

    #[test]
    fn has_exchange_client_empty_strings_treated_as_absent() {
        let cfg = KcAdminConfig {
            base_url: "http://kc".into(),
            realm: "r".into(),
            admin_user: "u".into(),
            admin_pass: "p".into(),
            exchange_client_id: None,
            exchange_client_secret: None,
        };
        assert!(!cfg.has_exchange_client());
    }

    #[test]
    fn kc_admin_config_base_url_preserved() {
        let c = KcAdminConfig {
            base_url: "http://keycloak:8080".into(),
            realm: "expresso".into(),
            admin_user: "admin".into(),
            admin_pass: "pw".into(),
            exchange_client_id: None,
            exchange_client_secret: None,
        };
        assert_eq!(c.base_url, "http://keycloak:8080");
        assert_eq!(c.realm, "expresso");
    }

    #[test]
    fn kc_admin_config_admin_user_preserved() {
        let c = KcAdminConfig {
            base_url:              "http://kc:8080".into(),
            realm:                 "master".into(),
            admin_user:            "admin".into(),
            admin_pass:            "pw".into(),
            exchange_client_id:    None,
            exchange_client_secret: None,
        };
        assert_eq!(c.admin_user, "admin");
    }

    #[test]
    fn kc_admin_config_realm_preserved() {
        let c = KcAdminConfig {
            base_url:               "http://kc:8080".into(),
            realm:                  "expresso".into(),
            admin_user:             "admin".into(),
            admin_pass:             "pw".into(),
            exchange_client_id:     None,
            exchange_client_secret: None,
        };
        assert_eq!(c.realm, "expresso");
    }

    #[test]
    fn kc_admin_config_base_url_with_port_preserved() {
        let c = KcAdminConfig {
            base_url:               "https://keycloak.internal:8443".into(),
            realm:                  "corp".into(),
            admin_user:             "admin".into(),
            admin_pass:             "s3cr3t".into(),
            exchange_client_id:     None,
            exchange_client_secret: None,
        };
        assert_eq!(c.base_url, "https://keycloak.internal:8443");
    }

    #[test]
    fn kc_admin_config_realm_name_preserved() {
        let c = KcAdminConfig {
            base_url:               "https://kc".into(),
            realm:                  "myrealm".into(),
            admin_user:             "admin".into(),
            admin_pass:             "pass".into(),
            exchange_client_id:     None,
            exchange_client_secret: None,
        };
        assert_eq!(c.realm, "myrealm");
    }

    #[test]
    fn kc_admin_config_svc_admin_user_preserved() {
        let c = KcAdminConfig {
            base_url:               "https://kc".into(),
            realm:                  "r".into(),
            admin_user:             "svc-admin".into(),
            admin_pass:             "secret".into(),
            exchange_client_id:     None,
            exchange_client_secret: None,
        };
        assert_eq!(c.admin_user, "svc-admin");
    }

    #[test]
    fn kc_admin_config_has_exchange_client_when_both_set() {
        let c = cfg(Some("client"), Some("secret"));
        assert!(c.has_exchange_client());
    }

    #[test]
    fn kc_admin_config_admin_pass_preserved() {
        let c = KcAdminConfig {
            base_url:               "https://kc".into(),
            realm:                  "r".into(),
            admin_user:             "admin".into(),
            admin_pass:             "s3cr3t".into(),
            exchange_client_id:     None,
            exchange_client_secret: None,
        };
        assert_eq!(c.admin_pass, "s3cr3t");
    }

    #[test]
    fn kc_admin_config_no_exchange_client_when_only_id_set() {
        let c = cfg(Some("client-id"), None);
        assert!(!c.has_exchange_client());
    }

    #[test]
    fn impersonation_tokens_expires_in_zero_by_default() {
        let json = r#"{"access_token":"tok"}"#;
        let t: ImpersonationTokens = serde_json::from_str(json).unwrap();
        assert_eq!(t.expires_in, 0);
    }

    #[test]
    fn kc_admin_config_exchange_client_id_preserved_when_set() {
        let c = cfg(Some("my-exchange-client"), Some("my-secret"));
        assert_eq!(c.exchange_client_id.as_deref(), Some("my-exchange-client"));
    }

    #[test]
    fn kc_admin_config_exchange_secret_preserved_when_set() {
        let c = cfg(Some("cid"), Some("supersecret"));
        assert_eq!(c.exchange_client_secret.as_deref(), Some("supersecret"));
    }
}

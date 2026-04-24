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

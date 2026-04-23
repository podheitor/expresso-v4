//! Thin Keycloak admin REST client — used for password-reset action emails.
//!
//! Uses `admin-cli` password grant against master realm (same pattern as
//! `expresso-admin`). Keep scope minimal: token + lookup-user-by-email +
//! execute-actions-email.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct KcAdminConfig {
    pub base_url:   String,
    pub realm:      String,
    pub admin_user: String,
    pub admin_pass: String,
}

impl KcAdminConfig {
    pub fn from_env() -> Option<Self> {
        let base_url   = std::env::var("KC_URL").ok()?;
        let realm      = std::env::var("KC_REALM").unwrap_or_else(|_| "expresso".into());
        let admin_user = std::env::var("KC_ADMIN_USER").ok()?;
        let admin_pass = std::env::var("KC_ADMIN_PASS").ok()?;
        Some(Self { base_url, realm, admin_user, admin_pass })
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

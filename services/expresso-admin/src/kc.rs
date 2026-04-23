//! Minimal Keycloak admin REST client (password grant w/ admin-cli).

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

use crate::templates::{KcRealm, KcUser};

#[derive(Debug, Clone)]
pub struct KcConfig {
    pub base_url:   String,
    pub realm:      String,
    pub admin_user: String,
    pub admin_pass: String,
}

impl KcConfig {
    pub fn from_env() -> Self {
        Self {
            base_url:   std::env::var("KC_URL").unwrap_or_else(|_| "http://expresso-keycloak:8080".into()),
            realm:      std::env::var("KC_REALM").unwrap_or_else(|_| "expresso".into()),
            admin_user: std::env::var("KC_ADMIN_USER").unwrap_or_else(|_| "admin".into()),
            admin_pass: std::env::var("KC_ADMIN_PASS").unwrap_or_default(),
        }
    }
}

#[derive(Deserialize)]
struct TokenResp { access_token: String }

pub struct KcClient {
    cfg: KcConfig,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Default)]
pub struct NewUser {
    pub username:   String,
    pub email:      String,
    pub first_name: String,
    pub last_name:  String,
    pub enabled:    bool,
    pub password:   String,
    pub temporary:  bool,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateUser {
    pub email:      Option<String>,
    pub first_name: Option<String>,
    pub last_name:  Option<String>,
    pub enabled:    Option<bool>,
}

impl KcClient {
    pub fn new(cfg: KcConfig) -> Self {
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

    pub async fn users(&self) -> Result<Vec<KcUser>> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}/users?max=500", self.cfg.base_url, self.cfg.realm);
        Ok(self.http.get(&url).bearer_auth(&tok).send().await?.error_for_status()?.json().await?)
    }

    pub async fn user(&self, id: &str) -> Result<KcUser> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}/users/{}", self.cfg.base_url, self.cfg.realm, id);
        Ok(self.http.get(&url).bearer_auth(&tok).send().await?.error_for_status()?.json().await?)
    }

    pub async fn realm(&self) -> Result<KcRealm> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}", self.cfg.base_url, self.cfg.realm);
        Ok(self.http.get(&url).bearer_auth(&tok).send().await?.error_for_status()?.json().await?)
    }

    /// Create user. Returns created user id (from Location header) when password set.
    pub async fn create_user(&self, u: &NewUser) -> Result<String> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}/users", self.cfg.base_url, self.cfg.realm);
        let body = json!({
            "username":  u.username,
            "email":     u.email,
            "firstName": u.first_name,
            "lastName":  u.last_name,
            "enabled":   u.enabled,
            "emailVerified": true,
        });
        let resp = self.http.post(&url).bearer_auth(&tok).json(&body)
            .send().await.context("kc create_user req")?
            .error_for_status().context("kc create_user status")?;
        let id = resp.headers().get("location")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next().map(String::from))
            .context("kc create_user: missing Location header")?;
        if !u.password.is_empty() {
            self.set_password(&id, &u.password, u.temporary).await?;
        }
        Ok(id)
    }

    pub async fn update_user(&self, id: &str, patch: &UpdateUser) -> Result<()> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}/users/{}", self.cfg.base_url, self.cfg.realm, id);
        let mut body = serde_json::Map::new();
        if let Some(v) = &patch.email      { body.insert("email".into(),     json!(v)); }
        if let Some(v) = &patch.first_name { body.insert("firstName".into(), json!(v)); }
        if let Some(v) = &patch.last_name  { body.insert("lastName".into(),  json!(v)); }
        if let Some(v) =  patch.enabled    { body.insert("enabled".into(),   json!(v)); }
        self.http.put(&url).bearer_auth(&tok).json(&body)
            .send().await.context("kc update_user req")?
            .error_for_status().context("kc update_user status")?;
        Ok(())
    }

    pub async fn set_password(&self, id: &str, password: &str, temporary: bool) -> Result<()> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}/users/{}/reset-password", self.cfg.base_url, self.cfg.realm, id);
        let body = json!({ "type": "password", "value": password, "temporary": temporary });
        self.http.put(&url).bearer_auth(&tok).json(&body)
            .send().await.context("kc set_password req")?
            .error_for_status().context("kc set_password status")?;
        Ok(())
    }

    /// Send execute-actions-email with CONFIGURE_TOTP (lifespan 1h).
    /// KC emails user an enrollment link; user scans QR in KC account page.
    pub async fn enroll_totp(&self, id: &str) -> Result<()> {
        let tok = self.token().await?;
        let url = format!(
            "{}/admin/realms/{}/users/{}/execute-actions-email?lifespan=3600",
            self.cfg.base_url, self.cfg.realm, id
        );
        self.http.put(&url).bearer_auth(&tok)
            .json(&["CONFIGURE_TOTP"])
            .send().await.context("kc enroll_totp req")?
            .error_for_status().context("kc enroll_totp status")?;
        Ok(())
    }

    /// Returns true iff user has at least one credential of type `otp`.
    /// One HTTP call per user; callers should rate-limit bulk scans.
    pub async fn user_has_totp(&self, id: &str) -> Result<bool> {
        let tok = self.token().await?;
        let url = format!(
            "{}/admin/realms/{}/users/{}/credentials",
            self.cfg.base_url, self.cfg.realm, id
        );
        let creds: Vec<serde_json::Value> = self.http.get(&url).bearer_auth(&tok)
            .send().await.context("kc has_totp list req")?
            .error_for_status().context("kc has_totp list status")?
            .json().await.context("kc has_totp list json")?;
        Ok(creds.iter().any(|c| c.get("type").and_then(|v| v.as_str()) == Some("otp")))
    }

    /// Deletes all OTP credentials for user → forces re-enrollment on next login.
    pub async fn reset_totp(&self, id: &str) -> Result<u32> {
        let tok = self.token().await?;
        // List credentials.
        let url = format!(
            "{}/admin/realms/{}/users/{}/credentials",
            self.cfg.base_url, self.cfg.realm, id
        );
        let creds: Vec<serde_json::Value> = self.http.get(&url).bearer_auth(&tok)
            .send().await.context("kc list creds req")?
            .error_for_status().context("kc list creds status")?
            .json().await.context("kc list creds json")?;
        let mut removed = 0u32;
        for c in creds {
            let t = c.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if t == "otp" {
                if let Some(cid) = c.get("id").and_then(|v| v.as_str()) {
                    let del = format!(
                        "{}/admin/realms/{}/users/{}/credentials/{}",
                        self.cfg.base_url, self.cfg.realm, id, cid
                    );
                    self.http.delete(&del).bearer_auth(&tok)
                        .send().await.context("kc del cred req")?
                        .error_for_status().context("kc del cred status")?;
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    pub async fn delete_user(&self, id: &str) -> Result<()> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}/users/{}", self.cfg.base_url, self.cfg.realm, id);
        self.http.delete(&url).bearer_auth(&tok)
            .send().await.context("kc delete_user req")?
            .error_for_status().context("kc delete_user status")?;
        Ok(())
    }
}

impl From<anyhow::Error> for crate::AdminError {
    fn from(e: anyhow::Error) -> Self { Self(e) }
}

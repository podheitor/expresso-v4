//! Minimal Keycloak admin REST client (password grant w/ admin-cli).

use anyhow::{Context, Result};
use serde::Deserialize;

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

    pub async fn realm(&self) -> Result<KcRealm> {
        let tok = self.token().await?;
        let url = format!("{}/admin/realms/{}", self.cfg.base_url, self.cfg.realm);
        Ok(self.http.get(&url).bearer_auth(&tok).send().await?.error_for_status()?.json().await?)
    }
}

impl From<anyhow::Error> for crate::AdminError {
    fn from(e: anyhow::Error) -> Self { Self(e) }
}

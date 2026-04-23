//! HTTP Basic → Keycloak password-grant validator.
//!
//! Designed for CalDAV / CardDAV / IMAP-like flows where clients send raw
//! username+password over HTTP Basic. We exchange these credentials with
//! Keycloak's `token` endpoint (grant_type=password) and, on success, trust
//! that the submitted username is authenticated.
//!
//! Short-TTL in-memory cache avoids one Keycloak round-trip per DAV request
//! (clients frequently send PROPFIND in tight loops).

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Configuration for `KcBasicAuthenticator`.
#[derive(Debug, Clone)]
pub struct KcBasicConfig {
    /// Keycloak base URL, e.g. `http://expresso-keycloak:8080`.
    pub url:           String,
    pub realm:         String,
    pub client_id:     String,
    /// Required only for confidential clients.
    pub client_secret: Option<String>,
    pub cache_ttl:     Duration,
    pub http_timeout:  Duration,
}

impl KcBasicConfig {
    pub fn from_env_prefix(prefix: &str) -> Option<Self> {
        let url   = std::env::var(format!("{prefix}_URL")).ok()?;
        let realm = std::env::var(format!("{prefix}_REALM")).ok()?;
        let client_id =
            std::env::var(format!("{prefix}_CLIENT_ID")).ok()?;
        let client_secret = std::env::var(format!("{prefix}_CLIENT_SECRET")).ok();
        Some(Self {
            url, realm, client_id, client_secret,
            cache_ttl:    Duration::from_secs(60),
            http_timeout: Duration::from_secs(5),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KcBasicError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("keycloak unreachable: {0}")]
    Unreachable(String),
    #[error("keycloak error: {0}")]
    Upstream(String),
}

#[derive(Deserialize)]
struct TokenResp { access_token: String }

struct CacheEntry {
    username: String,
    expires:  Instant,
}

pub struct KcBasicAuthenticator {
    cfg:   KcBasicConfig,
    http:  reqwest::Client,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl KcBasicAuthenticator {
    pub fn new(cfg: KcBasicConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(cfg.http_timeout)
            .build()
            .unwrap_or_default();
        Self { cfg, http, cache: Mutex::new(HashMap::new()) }
    }

    /// Validate `user:pass` against Keycloak. On success returns the
    /// authenticated username (echoed back — identity binding is proven by
    /// successful password grant).
    pub async fn authenticate(&self, user: &str, pass: &str) -> Result<String, KcBasicError> {
        let key = cache_key(user, pass);

        if let Some(hit) = self.cache_lookup(&key) {
            return Ok(hit);
        }

        let url = format!(
            "{}/realms/{}/protocol/openid-connect/token",
            self.cfg.url.trim_end_matches('/'),
            self.cfg.realm,
        );
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "password"),
            ("client_id",  &self.cfg.client_id),
            ("username",   user),
            ("password",   pass),
            ("scope",      "openid"),
        ];
        if let Some(s) = self.cfg.client_secret.as_deref() {
            form.push(("client_secret", s));
        }

        let resp = self.http.post(&url).form(&form).send().await
            .map_err(|e| KcBasicError::Unreachable(e.to_string()))?;

        match resp.status() {
            StatusCode::OK => {
                let _body: TokenResp = resp.json().await
                    .map_err(|e| KcBasicError::Upstream(e.to_string()))?;
                self.cache_insert(key, user);
                Ok(user.to_owned())
            }
            StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST => {
                Err(KcBasicError::InvalidCredentials)
            }
            other => Err(KcBasicError::Upstream(format!("http {}", other.as_u16()))),
        }
    }

    fn cache_lookup(&self, key: &str) -> Option<String> {
        let mut guard = self.cache.lock().ok()?;
        // Opportunistic eviction of expired keys.
        let now = Instant::now();
        guard.retain(|_, v| v.expires > now);
        guard.get(key).map(|e| e.username.clone())
    }

    fn cache_insert(&self, key: String, username: &str) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.insert(key, CacheEntry {
                username: username.to_owned(),
                expires:  Instant::now() + self.cfg.cache_ttl,
            });
        }
    }
}

fn cache_key(user: &str, pass: &str) -> String {
    let mut h = Sha256::new();
    h.update(user.as_bytes());
    h.update(b":");
    h.update(pass.as_bytes());
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_varies() {
        assert_ne!(cache_key("a", "p"), cache_key("a", "q"));
        assert_ne!(cache_key("a", "p"), cache_key("b", "p"));
        assert_eq!(cache_key("x", "y"), cache_key("x", "y"));
    }

    #[test]
    fn config_from_env_missing() {
        // Unset vars → None
        assert!(KcBasicConfig::from_env_prefix("__NOPE_XYZ").is_none());
    }
}

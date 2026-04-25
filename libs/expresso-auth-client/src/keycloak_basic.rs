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
    /// Falhas consecutivas (na janela `failure_window`) antes do
    /// lockout disparar. Default 10 — alto o suficiente pra usuários
    /// reais não serem prejudicados por typo, baixo pra throttle KC.
    pub max_failures:     u32,
    /// Janela de contagem das falhas. Default 60s.
    pub failure_window:   Duration,
    /// Duração do lockout depois de atingir `max_failures`. Default 5min.
    pub lockout_duration: Duration,
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
            cache_ttl:        Duration::from_secs(60),
            http_timeout:     Duration::from_secs(5),
            max_failures:     10,
            failure_window:   Duration::from_secs(60),
            lockout_duration: Duration::from_secs(5 * 60),
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

/// Tracker per-username de falhas recentes. Usar lowercased username
/// como chave (não incluir senha) — caso contrário atacante rotaciona
/// senhas pra burlar o counter.
#[derive(Debug)]
struct FailureTracker {
    window_start: Instant,
    failures:     u32,
    locked_until: Option<Instant>,
}

pub struct KcBasicAuthenticator {
    cfg:      KcBasicConfig,
    http:     reqwest::Client,
    cache:    Mutex<HashMap<String, CacheEntry>>,
    failures: Mutex<HashMap<String, FailureTracker>>,
}

impl KcBasicAuthenticator {
    pub fn new(cfg: KcBasicConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(cfg.http_timeout)
            .build()
            .unwrap_or_default();
        Self {
            cfg,
            http,
            cache:    Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// Validate `user:pass` against Keycloak. On success returns the
    /// authenticated username (echoed back — identity binding is proven by
    /// successful password grant).
    pub async fn authenticate(&self, user: &str, pass: &str) -> Result<String, KcBasicError> {
        // Lockout: depois de N falhas na janela, recusa sem bater no KC.
        // Protege contra brute-force E evita inundar o KC com chamadas.
        // Erros de rede/upstream não contam como falha (não confundir
        // "credencial errada" com "KC fora do ar").
        let user_key = user.to_ascii_lowercase();
        if self.is_locked_out(&user_key) {
            return Err(KcBasicError::InvalidCredentials);
        }

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
                self.clear_failures(&user_key);
                self.cache_insert(key, user);
                Ok(user.to_owned())
            }
            StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST => {
                self.record_failure(&user_key);
                Err(KcBasicError::InvalidCredentials)
            }
            other => Err(KcBasicError::Upstream(format!("http {}", other.as_u16()))),
        }
    }

    fn is_locked_out(&self, user_key: &str) -> bool {
        let Ok(guard) = self.failures.lock() else { return false; };
        let now = Instant::now();
        guard.get(user_key)
            .and_then(|t| t.locked_until)
            .is_some_and(|until| until > now)
    }

    fn record_failure(&self, user_key: &str) {
        let Ok(mut guard) = self.failures.lock() else { return; };
        let now = Instant::now();
        let entry = guard.entry(user_key.to_string()).or_insert(FailureTracker {
            window_start: now,
            failures:     0,
            locked_until: None,
        });
        // Janela expirou → reseta o counter.
        if now.duration_since(entry.window_start) > self.cfg.failure_window {
            entry.window_start = now;
            entry.failures     = 0;
            entry.locked_until = None;
        }
        entry.failures += 1;
        if entry.failures >= self.cfg.max_failures {
            entry.locked_until = Some(now + self.cfg.lockout_duration);
        }
    }

    fn clear_failures(&self, user_key: &str) {
        if let Ok(mut guard) = self.failures.lock() {
            guard.remove(user_key);
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

    fn fixture_auth(max: u32, window: Duration, lock: Duration) -> KcBasicAuthenticator {
        let cfg = KcBasicConfig {
            url:              "http://x".into(),
            realm:            "r".into(),
            client_id:        "c".into(),
            client_secret:    None,
            cache_ttl:        Duration::from_secs(60),
            http_timeout:     Duration::from_secs(5),
            max_failures:     max,
            failure_window:   window,
            lockout_duration: lock,
        };
        KcBasicAuthenticator::new(cfg)
    }

    #[test]
    fn lockout_triggers_after_max_failures() {
        let a = fixture_auth(3, Duration::from_secs(60), Duration::from_secs(60));
        assert!(!a.is_locked_out("alice"));
        a.record_failure("alice");
        a.record_failure("alice");
        assert!(!a.is_locked_out("alice"));
        a.record_failure("alice"); // reaches max
        assert!(a.is_locked_out("alice"));
        // Bob unaffected — lockout é per-username.
        assert!(!a.is_locked_out("bob"));
    }

    #[test]
    fn lockout_expires_after_duration() {
        let a = fixture_auth(2, Duration::from_secs(60), Duration::from_millis(50));
        a.record_failure("alice");
        a.record_failure("alice");
        assert!(a.is_locked_out("alice"));
        std::thread::sleep(Duration::from_millis(80));
        assert!(!a.is_locked_out("alice"));
    }

    #[test]
    fn success_clears_failures() {
        let a = fixture_auth(3, Duration::from_secs(60), Duration::from_secs(60));
        a.record_failure("alice");
        a.record_failure("alice");
        a.clear_failures("alice");
        a.record_failure("alice"); // counter reseta — só conta como 1
        assert!(!a.is_locked_out("alice"));
    }

    #[test]
    fn window_expiry_resets_counter() {
        let a = fixture_auth(3, Duration::from_millis(40), Duration::from_secs(60));
        a.record_failure("alice");
        a.record_failure("alice");
        std::thread::sleep(Duration::from_millis(60));
        // Janela expirou — próxima falha começa um counter novo.
        a.record_failure("alice");
        a.record_failure("alice");
        assert!(!a.is_locked_out("alice"));
    }
}

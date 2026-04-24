//! OIDC token validator with JWKS caching.
//!
//! Strategy:
//! 1. Discovery: fetch `{issuer}/.well-known/openid-configuration` on boot →
//!    resolves `jwks_uri`.
//! 2. JWKS fetch: pull RSA/EC signing keys from `jwks_uri`; cache in an
//!    `ArcSwap<HashMap<kid, DecodingKey>>`.
//! 3. Validate: jwt header → lookup `kid` → decode+verify (RS256/ES256) →
//!    check `iss` + `aud` + `exp` → build `AuthContext`.
//!
//! Refresh: on cache-miss `kid`, re-fetch JWKS once (keys rotated). If still
//! missing → `KidNotFound`. TTL refresh is opportunistic (no background task).

use std::{collections::HashMap, sync::Arc, time::{Duration, Instant}};

use arc_swap::ArcSwap;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::claims::{AuthContext, RawClaims};
use crate::error::{AuthError, Result};

#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// Expected `iss` claim — full realm URL, e.g.
    /// `http://keycloak/realms/expresso`.
    pub issuer:    String,
    /// Expected `aud` entry — Keycloak client_id (e.g. `expresso-web`).
    pub audience:  String,
    /// Minimum interval between JWKS refresh attempts. Prevents thundering
    /// herd if every unknown `kid` triggered a fetch.
    pub jwks_min_refresh: Duration,
    /// HTTP timeout for discovery + JWKS fetch.
    pub http_timeout: Duration,
}

impl OidcConfig {
    pub fn new(issuer: impl Into<String>, audience: impl Into<String>) -> Self {
        Self {
            issuer:   issuer.into(),
            audience: audience.into(),
            jwks_min_refresh: Duration::from_secs(30),
            http_timeout:     Duration::from_secs(5),
        }
    }

    /// Multi-aud support: comma-separated audience values accepted during JWT
    /// validation. First entry also used as `primary_audience` for role/roles
    /// extraction from `resource_access[audience]`.
    pub fn audiences(&self) -> Vec<&str> {
        self.audience.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect()
    }

    pub fn primary_audience(&self) -> &str {
        self.audiences().into_iter().next().unwrap_or(self.audience.as_str())
    }
}

#[derive(Debug, Deserialize)]
struct Discovery {
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    kty: String,
    alg: Option<String>,
    // RSA
    n:   Option<String>,
    e:   Option<String>,
    // EC
    crv: Option<String>,
    x:   Option<String>,
    y:   Option<String>,
}

struct KeyEntry {
    key: DecodingKey,
    alg: Algorithm,
}

pub struct OidcValidator {
    cfg:      OidcConfig,
    http:     reqwest::Client,
    jwks_uri: ArcSwap<String>,
    keys:     ArcSwap<HashMap<String, Arc<KeyEntry>>>,
    refresh:  Mutex<Instant>,  // last refresh attempt
}

impl OidcValidator {
    /// Build a validator — fetches discovery + initial JWKS eagerly so that
    /// the first token doesn't pay the latency hit.
    pub async fn new(cfg: OidcConfig) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(cfg.http_timeout)
            .build()
            .map_err(|e| AuthError::Config(format!("http client: {e}")))?;

        let jwks_uri = Self::discover(&http, &cfg.issuer).await?;
        let keys = Self::fetch_jwks(&http, &jwks_uri).await?;
        Ok(Self {
            cfg,
            http,
            jwks_uri: ArcSwap::from_pointee(jwks_uri),
            keys:     ArcSwap::from_pointee(keys),
            refresh:  Mutex::new(Instant::now()),
        })
    }

    pub fn config(&self) -> &OidcConfig { &self.cfg }

    /// Validate a bearer JWT and return a normalized `AuthContext`.
    pub async fn validate(&self, token: &str) -> Result<AuthContext> {
        let header = decode_header(token)
            .map_err(|e| AuthError::InvalidToken(format!("header decode: {e}")))?;
        let kid = header.kid.clone();

        let key_entry = self.lookup_or_refresh(kid.as_deref()).await?;

        let mut val = Validation::new(key_entry.alg);
        val.set_issuer(&[self.cfg.issuer.as_str()]);
        let auds = self.cfg.audiences();
        val.set_audience(&auds);
        val.validate_exp = true;

        let data = decode::<RawClaims>(token, &key_entry.key, &val)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => AuthError::Expired,
                _ => AuthError::InvalidToken(e.to_string()),
            })?;

        AuthContext::from_raw(data.claims, self.cfg.primary_audience())
    }

    /// Lookup a `kid` in the cache; on miss, refresh JWKS once (rate-limited).
    async fn lookup_or_refresh(&self, kid: Option<&str>) -> Result<Arc<KeyEntry>> {
        if let Some(entry) = self.lookup(kid) { return Ok(entry); }

        // Cache miss — rate-limit refresh attempts to avoid hammering Keycloak
        // on malformed tokens.
        let mut last = self.refresh.lock().await;
        if last.elapsed() < self.cfg.jwks_min_refresh {
            return Err(AuthError::KidNotFound(kid.map(String::from)));
        }
        *last = Instant::now();
        drop(last);

        debug!(?kid, "jwks cache miss — refreshing");
        let uri = self.jwks_uri.load_full();
        match Self::fetch_jwks(&self.http, &uri).await {
            Ok(new_keys) => self.keys.store(Arc::new(new_keys)),
            Err(e)       => warn!(error = %e, "jwks refresh failed"),
        }
        self.lookup(kid).ok_or(AuthError::KidNotFound(kid.map(String::from)))
    }

    fn lookup(&self, kid: Option<&str>) -> Option<Arc<KeyEntry>> {
        let keys = self.keys.load();
        match kid {
            Some(k) => keys.get(k).cloned(),
            // When the token has no `kid` and exactly one key exists, use it
            // (common in single-key dev realms).
            None if keys.len() == 1 => keys.values().next().cloned(),
            None => None,
        }
    }

    async fn discover(http: &reqwest::Client, issuer: &str) -> Result<String> {
        let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
        let resp = http.get(&url).send().await
            .map_err(|e| AuthError::Config(format!("discovery fetch: {e}")))?;
        if !resp.status().is_success() {
            return Err(AuthError::Config(format!("discovery HTTP {}", resp.status())));
        }
        let disc: Discovery = resp.json().await
            .map_err(|e| AuthError::Config(format!("discovery decode: {e}")))?;
        Ok(disc.jwks_uri)
    }

    async fn fetch_jwks(
        http: &reqwest::Client,
        uri: &str,
    ) -> Result<HashMap<String, Arc<KeyEntry>>> {
        let resp = http.get(uri).send().await
            .map_err(|e| AuthError::JwksFetch(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(AuthError::JwksFetch(format!("HTTP {}", resp.status())));
        }
        let jwks: Jwks = resp.json().await
            .map_err(|e| AuthError::JwksFetch(format!("decode: {e}")))?;
        let mut out = HashMap::with_capacity(jwks.keys.len());
        for jwk in jwks.keys {
            if let Some(entry) = jwk_to_key(&jwk) {
                out.insert(jwk.kid, Arc::new(entry));
            }
        }
        if out.is_empty() {
            return Err(AuthError::JwksFetch("no usable keys".into()));
        }
        Ok(out)
    }
}

fn jwk_to_key(jwk: &Jwk) -> Option<KeyEntry> {
    match jwk.kty.as_str() {
        "RSA" => {
            let (n, e) = (jwk.n.as_ref()?, jwk.e.as_ref()?);
            let key = DecodingKey::from_rsa_components(n, e).ok()?;
            let alg = match jwk.alg.as_deref() {
                Some("RS384") => Algorithm::RS384,
                Some("RS512") => Algorithm::RS512,
                _             => Algorithm::RS256,
            };
            Some(KeyEntry { key, alg })
        }
        "EC" => {
            let (x, y) = (jwk.x.as_ref()?, jwk.y.as_ref()?);
            let key = DecodingKey::from_ec_components(x, y).ok()?;
            let alg = match jwk.crv.as_deref() {
                Some("P-384") => Algorithm::ES384,
                _             => Algorithm::ES256,
            };
            Some(KeyEntry { key, alg })
        }
        _ => None,
    }
}

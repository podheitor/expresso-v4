//! Personal access token (PAT) resolution for the `Authenticated` extractor.
//!
//! PATs (`ept_…`) are opaque, not JWTs — they can't be validated locally. This
//! resolver exchanges one for an [`AuthContext`] by calling expresso-auth's
//! `POST /internal/tokens/introspect` (the phase-2 endpoint). It is injected
//! into request extensions like `OidcValidator`; when absent, the extractor's
//! PAT branch stays off and only JWTs are accepted.

use serde::Deserialize;
use uuid::Uuid;

use crate::claims::AuthContext;
use crate::error::AuthError;

/// Token prefix that marks an opaque PAT (vs. a JWT). Mirrors the issuer in
/// expresso-auth's token handler.
pub const PAT_PREFIX: &str = "ept_";

/// A short synthetic lifetime applied to a PAT-derived context's `expires_at`.
/// The PAT's real expiry/revocation is enforced server-side at introspection;
/// this only bounds how long downstream code treats the resolved context as
/// fresh within a single request.
const PAT_CONTEXT_TTL_SECS: i64 = 300;

/// Resolves PATs against the auth service's introspection endpoint.
#[derive(Debug, Clone)]
pub struct PatResolver {
    /// expresso-auth base URL (no trailing slash), e.g. `http://expresso-auth:8080`.
    auth_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct IntrospectResp {
    active: bool,
    user_id: Option<Uuid>,
    tenant_id: Option<Uuid>,
    email: Option<String>,
    display_name: Option<String>,
    role: Option<String>,
}

impl PatResolver {
    #[must_use]
    pub fn new(auth_url: impl Into<String>, http: reqwest::Client) -> Self {
        Self {
            auth_url: auth_url.into().trim_end_matches('/').to_string(),
            http,
        }
    }

    /// Whether a token looks like a PAT (so the extractor can route it here
    /// instead of the JWT validator).
    #[must_use]
    pub fn is_pat(token: &str) -> bool {
        token.starts_with(PAT_PREFIX)
    }

    /// Exchange a PAT for an `AuthContext`. Errors as `InvalidToken` when the
    /// token is unknown/revoked/expired, or `Misconfigured`-adjacent transport
    /// failures surface as `InvalidToken` too (fail-closed: a resolver outage
    /// must not authenticate).
    pub async fn resolve(&self, token: &str, now_unix: i64) -> Result<AuthContext, AuthError> {
        let url = format!("{}/internal/tokens/introspect", self.auth_url);
        let resp = self
            .http
            .post(&url)
            .json(&serde_json::json!({ "token": token }))
            .send()
            .await
            .map_err(|e| AuthError::InvalidToken(format!("pat introspect failed: {e}")))?;
        if !resp.status().is_success() {
            return Err(AuthError::InvalidToken(format!(
                "pat introspect HTTP {}",
                resp.status()
            )));
        }
        let body: IntrospectResp = resp
            .json()
            .await
            .map_err(|e| AuthError::InvalidToken(format!("pat introspect decode: {e}")))?;

        if !body.active {
            return Err(AuthError::InvalidToken("pat inactive".into()));
        }
        let (Some(user_id), Some(tenant_id)) = (body.user_id, body.tenant_id) else {
            return Err(AuthError::InvalidToken("pat missing identity".into()));
        };
        Ok(AuthContext {
            user_id,
            tenant_id,
            email: body.email.unwrap_or_default(),
            display_name: body.display_name.unwrap_or_default(),
            roles: body.role.into_iter().collect(),
            expires_at: now_unix + PAT_CONTEXT_TTL_SECS,
            acr: None,
            amr: vec!["pat".into()],
            govbr_cpf_hash: None,
            govbr_confiabilidades: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_pat_detects_prefix() {
        assert!(PatResolver::is_pat("ept_abc123"));
        assert!(!PatResolver::is_pat("eyJhbGci...")); // a JWT
        assert!(!PatResolver::is_pat(""));
    }

    #[test]
    fn new_trims_trailing_slash() {
        let r = PatResolver::new("http://auth:8080/", reqwest::Client::new());
        assert_eq!(r.auth_url, "http://auth:8080");
    }
}

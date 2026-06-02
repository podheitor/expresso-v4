//! Shared-secret gate for the `/internal/*` routes (defense-in-depth on top of
//! the LAN-trust boundary).
//!
//! These endpoints (token introspect, SAML/LDAP realm reflection) are called by
//! the admin service over the internal network and mutate Keycloak with shared
//! admin credentials, so a single compromised LAN service could otherwise pivot
//! to any tenant's IdP config. Requiring a shared secret (`AUTH__INTERNAL_TOKEN`)
//! on every `/internal/*` request contains that blast radius.
//!
//! Fail-open when the secret is unset: deployments that have not configured one
//! keep the prior behaviour (a one-time warning is logged at startup via
//! [`warn_if_unset`]). Set the env on both the auth service and the admin caller
//! to turn the gate on.

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Header carrying the shared secret on `/internal/*` calls.
pub const INTERNAL_TOKEN_HEADER: &str = "x-internal-token";
const INTERNAL_TOKEN_ENV: &str = "AUTH__INTERNAL_TOKEN";

/// Read the configured secret (trimmed, non-empty) or `None` when unset.
fn configured_secret() -> Option<String> {
    std::env::var(INTERNAL_TOKEN_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Log once at startup if the gate is disabled, so an operator notices.
pub fn warn_if_unset() {
    if configured_secret().is_none() {
        tracing::warn!(
            "{INTERNAL_TOKEN_ENV} unset: /internal/* routes rely on network trust only \
             (set it on auth + admin to enable the shared-secret gate)"
        );
    }
}

/// Constant-time equality so a wrong secret can't be recovered by timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Middleware: when a secret is configured, require a matching
/// `X-Internal-Token` header; otherwise pass through (fail-open).
pub async fn internal_auth_mw(headers: HeaderMap, req: Request, next: Next) -> Response {
    if let Some(secret) = configured_secret() {
        let presented = headers
            .get(INTERNAL_TOKEN_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !ct_eq(presented.as_bytes(), secret.as_bytes()) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::ct_eq;

    #[test]
    fn ct_eq_matches_equal() {
        assert!(ct_eq(b"s3cret", b"s3cret"));
    }

    #[test]
    fn ct_eq_rejects_different() {
        assert!(!ct_eq(b"s3cret", b"s3crxt"));
        assert!(!ct_eq(b"s3cret", b"s3cre"));
        assert!(!ct_eq(b"", b"x"));
    }

    #[test]
    fn ct_eq_empty_equal() {
        assert!(ct_eq(b"", b""));
    }
}

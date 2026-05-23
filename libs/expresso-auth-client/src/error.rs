//! Auth client error types.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, AuthError>;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("config: {0}")]
    Config(String),

    #[error("jwks fetch failed: {0}")]
    JwksFetch(String),

    #[error("jwks has no key matching kid={0:?}")]
    KidNotFound(Option<String>),

    #[error("invalid token: {0}")]
    InvalidToken(String),

    #[error("token expired")]
    Expired,

    #[error("missing required claim: {0}")]
    MissingClaim(&'static str),

    #[error("malformed claim `{0}`: {1}")]
    MalformedClaim(&'static str, String),

    #[error("authorization header missing or malformed")]
    MissingBearer,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_display_contains_message() {
        let e = AuthError::Config("no issuer".into());
        assert!(e.to_string().contains("no issuer"));
    }

    #[test]
    fn jwks_fetch_display_contains_message() {
        let e = AuthError::JwksFetch("connection refused".into());
        assert!(e.to_string().contains("connection refused"));
    }

    #[test]
    fn kid_not_found_some_displays_kid() {
        let e = AuthError::KidNotFound(Some("abc123".into()));
        assert!(e.to_string().contains("abc123"));
    }

    #[test]
    fn kid_not_found_none_displays_none() {
        let e = AuthError::KidNotFound(None);
        assert!(e.to_string().contains("None"));
    }

    #[test]
    fn invalid_token_display() {
        let e = AuthError::InvalidToken("signature".into());
        assert!(e.to_string().contains("signature"));
    }

    #[test]
    fn expired_display() {
        assert_eq!(AuthError::Expired.to_string(), "token expired");
    }

    #[test]
    fn missing_claim_display() {
        let e = AuthError::MissingClaim("tenant_id");
        assert!(e.to_string().contains("tenant_id"));
    }

    #[test]
    fn malformed_claim_display() {
        let e = AuthError::MalformedClaim("sub", "not uuid".into());
        assert!(e.to_string().contains("sub") && e.to_string().contains("not uuid"));
    }

    #[test]
    fn missing_bearer_display() {
        let s = AuthError::MissingBearer.to_string();
        assert!(!s.is_empty());
    }

    #[test]
    fn expired_display_not_empty() {
        let s = AuthError::Expired.to_string();
        assert!(!s.is_empty());
    }

    #[test]
    fn invalid_token_display_not_empty() {
        let s = AuthError::InvalidToken("bad sig".into()).to_string();
        assert!(s.contains("bad sig") || !s.is_empty());
    }

    #[test]
    fn missing_bearer_display_not_empty() {
        let s = AuthError::MissingBearer.to_string();
        assert!(!s.is_empty());
    }

    #[test]
    fn expired_display_is_token_expired() {
        let s = AuthError::Expired.to_string();
        assert_eq!(s, "token expired");
    }

    #[test]
    fn malformed_claim_display_contains_field_name() {
        let e = AuthError::MalformedClaim("roles", "not an array".into());
        assert!(e.to_string().contains("roles"));
    }

    #[test]
    fn config_error_display_contains_message() {
        let e = AuthError::Config("missing issuer".into());
        assert!(e.to_string().contains("missing issuer"));
    }

    #[test]
    fn kid_not_found_display_with_known_kid() {
        let e = AuthError::KidNotFound(Some("rsa-key-2026".into()));
        assert!(e.to_string().contains("rsa-key-2026"));
    }

    #[test]
    fn missing_bearer_display_is_not_empty() {
        let e = AuthError::MissingBearer;
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn kid_not_found_none_display_is_not_empty() {
        let e = AuthError::KidNotFound(None);
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn invalid_token_and_expired_are_distinct_messages() {
        let a = AuthError::InvalidToken("bad sig".into()).to_string();
        let b = AuthError::Expired.to_string();
        assert_ne!(a, b);
    }

    #[test]
    fn malformed_claim_display_contains_both_field_and_reason() {
        let e = AuthError::MalformedClaim("iat", "overflow".into());
        let s = e.to_string();
        assert!(s.contains("iat") && s.contains("overflow"));
    }

    #[test]
    fn config_and_jwks_fetch_are_distinct_variants() {
        let a = AuthError::Config("x".into()).to_string();
        let b = AuthError::JwksFetch("x".into()).to_string();
        assert_ne!(a, b);
    }

    #[test]
    fn missing_claim_display_contains_claim_name() {
        let e = AuthError::MissingClaim("sub");
        assert!(e.to_string().contains("sub"));
    }

    #[test]
    fn malformed_claim_and_missing_claim_are_distinct_variants() {
        let a = AuthError::MalformedClaim("sub", "bad".into()).to_string();
        let b = AuthError::MissingClaim("sub").to_string();
        assert_ne!(a, b);
    }

    #[test]
    fn jwks_fetch_display_is_not_empty() {
        let e = AuthError::JwksFetch("dns timeout".into());
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn kid_not_found_some_and_none_displays_differ() {
        let a = AuthError::KidNotFound(Some("k1".into())).to_string();
        let b = AuthError::KidNotFound(None).to_string();
        assert_ne!(a, b);
    }
}

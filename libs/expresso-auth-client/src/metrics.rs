//! Metrics for multi-realm auth validation.
//!
//! Counter: `auth_validation_total{realm, result}` — results:
//!   ok | expired | invalid | unknown_key | forbidden | misconfigured
//! Gauge:   `auth_realm_cache_size` — validators cached in MultiRealmValidator.

use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, register_int_gauge, IntCounterVec, IntGauge};

pub static VALIDATION_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "auth_validation_total",
        "JWT validation attempts by realm and result.",
        &["realm", "result"]
    )
    .expect("register auth_validation_total")
});

pub static REALM_CACHE_SIZE: Lazy<IntGauge> = Lazy::new(|| {
    register_int_gauge!(
        "auth_realm_cache_size",
        "Number of per-realm validators cached in MultiRealmValidator."
    )
    .expect("register auth_realm_cache_size")
});

/// Classify AuthError → metric result label.
pub fn result_label(err: &crate::error::AuthError) -> &'static str {
    use crate::error::AuthError::*;
    match err {
        Expired => "expired",
        MissingBearer => "missing_bearer",
        InvalidToken(_) => "invalid",
        KidNotFound(_) => "unknown_key",
        MalformedClaim(..) => "malformed",
        MissingClaim(_) => "forbidden",
        Config(_) => "misconfigured",
        JwksFetch(_) => "jwks_fetch",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AuthError;

    #[test]
    fn result_label_expired() {
        assert_eq!(result_label(&AuthError::Expired), "expired");
    }

    #[test]
    fn result_label_missing_bearer() {
        assert_eq!(result_label(&AuthError::MissingBearer), "missing_bearer");
    }

    #[test]
    fn result_label_invalid_token() {
        assert_eq!(
            result_label(&AuthError::InvalidToken("bad sig".into())),
            "invalid"
        );
    }

    #[test]
    fn result_label_kid_not_found() {
        assert_eq!(
            result_label(&AuthError::KidNotFound(Some("k1".into()))),
            "unknown_key"
        );
    }

    #[test]
    fn result_label_malformed_claim() {
        assert_eq!(
            result_label(&AuthError::MalformedClaim("exp", "nan".into())),
            "malformed"
        );
    }

    #[test]
    fn result_label_missing_claim() {
        assert_eq!(result_label(&AuthError::MissingClaim("sub")), "forbidden");
    }

    #[test]
    fn result_label_config() {
        assert_eq!(
            result_label(&AuthError::Config("bad url".into())),
            "misconfigured"
        );
    }

    #[test]
    fn result_label_jwks_fetch() {
        assert_eq!(
            result_label(&AuthError::JwksFetch("timeout".into())),
            "jwks_fetch"
        );
    }

    #[test]
    fn result_label_forbidden_maps_missing_claim() {
        assert_eq!(result_label(&AuthError::MissingClaim("aud")), "forbidden");
    }

    #[test]
    fn result_label_missing_bearer_maps_missing_bearer() {
        assert_eq!(result_label(&AuthError::MissingBearer), "missing_bearer");
    }

    #[test]
    fn result_label_all_labels_are_nonempty() {
        use crate::error::AuthError;
        for label in [
            result_label(&AuthError::Expired),
            result_label(&AuthError::MissingBearer),
            result_label(&AuthError::InvalidToken("x".into())),
        ] {
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn result_label_malformed_claim_maps_malformed() {
        assert_eq!(
            result_label(&AuthError::MalformedClaim("iat", "not-a-number".into())),
            "malformed"
        );
    }

    #[test]
    fn result_label_config_maps_misconfigured() {
        assert_eq!(
            result_label(&AuthError::Config("bad jwks url".into())),
            "misconfigured"
        );
    }

    #[test]
    fn result_label_jwks_fetch_maps_jwks_fetch() {
        assert_eq!(
            result_label(&AuthError::JwksFetch("timeout".into())),
            "jwks_fetch"
        );
    }

    #[test]
    fn result_label_expired_maps_expired() {
        assert_eq!(result_label(&AuthError::Expired), "expired");
    }

    #[test]
    fn result_label_missing_claim_maps_forbidden() {
        assert_eq!(
            result_label(&AuthError::MissingClaim("tenant_id")),
            "forbidden"
        );
    }

    #[test]
    fn result_label_missing_bearer_variant_returns_missing_bearer_str() {
        assert_eq!(result_label(&AuthError::MissingBearer), "missing_bearer");
    }

    #[test]
    fn result_label_kid_not_found_none_is_unknown_key() {
        assert_eq!(result_label(&AuthError::KidNotFound(None)), "unknown_key");
    }

    #[test]
    fn result_label_kid_not_found_some_is_unknown_key() {
        assert_eq!(
            result_label(&AuthError::KidNotFound(Some("k1".into()))),
            "unknown_key"
        );
    }

    #[test]
    fn result_label_invalid_token_is_invalid() {
        assert_eq!(
            result_label(&AuthError::InvalidToken("x".into())),
            "invalid"
        );
    }

    #[test]
    fn result_label_all_variants_are_nonempty() {
        let labels = [
            result_label(&AuthError::Expired),
            result_label(&AuthError::MissingBearer),
            result_label(&AuthError::InvalidToken("t".into())),
            result_label(&AuthError::KidNotFound(None)),
            result_label(&AuthError::MalformedClaim("f", "r".into())),
            result_label(&AuthError::MissingClaim("c")),
            result_label(&AuthError::Config("c".into())),
            result_label(&AuthError::JwksFetch("e".into())),
        ];
        for l in labels {
            assert!(!l.is_empty());
        }
    }

    #[test]
    fn result_label_expired_is_static_str() {
        let label: &'static str = result_label(&AuthError::Expired);
        assert_eq!(label, "expired");
    }

    #[test]
    fn result_label_config_and_jwks_fetch_differ() {
        let a = result_label(&AuthError::Config("x".into()));
        let b = result_label(&AuthError::JwksFetch("x".into()));
        assert_ne!(a, b);
    }

    #[test]
    fn result_label_expired_and_invalid_token_differ() {
        let a = result_label(&AuthError::Expired);
        let b = result_label(&AuthError::InvalidToken("x".into()));
        assert_ne!(a, b);
    }

    #[test]
    fn result_label_missing_claim_and_malformed_claim_differ() {
        let a = result_label(&AuthError::MissingClaim("sub"));
        let b = result_label(&AuthError::MalformedClaim("sub", "bad".into()));
        assert_ne!(a, b);
    }

    #[test]
    fn result_label_invalid_token_and_malformed_claim_differ() {
        let a = result_label(&AuthError::InvalidToken("bad".into()));
        let b = result_label(&AuthError::MalformedClaim("sub", "bad".into()));
        assert_ne!(a, b);
    }
}

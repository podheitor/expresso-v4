//! Metrics for multi-realm auth validation.
//!
//! Counter: `auth_validation_total{realm, result}` — results:
//!   ok | expired | invalid | unknown_key | forbidden | misconfigured
//! Gauge:   `auth_realm_cache_size` — validators cached in MultiRealmValidator.

use once_cell::sync::Lazy;
use prometheus::{
    register_int_counter_vec, register_int_gauge, IntCounterVec, IntGauge,
};

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
        Expired            => "expired",
        MissingBearer      => "missing_bearer",
        InvalidToken(_)    => "invalid",
        KidNotFound(_)     => "unknown_key",
        MalformedClaim(..) => "malformed",
        MissingClaim(_)    => "forbidden",
        Config(_)          => "misconfigured",
        JwksFetch(_)       => "jwks_fetch",
    }
}

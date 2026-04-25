//! Prometheus metrics for the WOPI handler surface.
//!
//! Single counter `drive_wopi_ops_total{op, outcome}` covers the lifecycle:
//! - `op`      → check_file_info, get_file, put_file, lock, unlock,
//!               refresh_lock, get_lock, unlock_and_relock, other
//! - `outcome` → ok, conflict, unauthorized, bad_request, quota_exceeded,
//!               not_found, forbidden, error
//!
//! Cardinality is capped: handlers always pass one of the canonical labels
//! above. Unknown `X-WOPI-Override` values collapse to `op="other"`.

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

use crate::error::DriveError;

pub static WOPI_OPS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "drive_wopi_ops_total",
            "WOPI handler outcomes per operation",
        ),
        &["op", "outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

const OPS: &[&str] = &[
    "check_file_info",
    "get_file",
    "put_file",
    "lock",
    "unlock",
    "refresh_lock",
    "get_lock",
    "unlock_and_relock",
    "other",
];

const OUTCOMES: &[&str] = &[
    "ok",
    "conflict",
    "unauthorized",
    "bad_request",
    "quota_exceeded",
    "not_found",
    "forbidden",
    "error",
];

/// Pre-populate label series so Prometheus `rate()` / `increase()` work
/// from the first scrape, even before any client connects. Idempotent.
pub fn init() {
    Lazy::force(&WOPI_OPS_TOTAL);
    for op in OPS {
        for outcome in OUTCOMES {
            WOPI_OPS_TOTAL.with_label_values(&[op, outcome]).inc_by(0);
        }
    }
}

#[inline]
pub fn record(op: &'static str, outcome: &'static str) {
    WOPI_OPS_TOTAL.with_label_values(&[op, outcome]).inc();
}

/// Map a `DriveError` to the canonical outcome label.
pub fn outcome_for_err(e: &DriveError) -> &'static str {
    match e {
        DriveError::Unauthorized   => "unauthorized",
        DriveError::BadRequest(_)  => "bad_request",
        DriveError::Conflict(_)    => "conflict",
        DriveError::QuotaExceeded  => "quota_exceeded",
        DriveError::NotFound(_)    => "not_found",
        DriveError::Forbidden      => "forbidden",
        _                          => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn err_outcome_mapping() {
        assert_eq!(outcome_for_err(&DriveError::Unauthorized),         "unauthorized");
        assert_eq!(outcome_for_err(&DriveError::BadRequest("x".into())), "bad_request");
        assert_eq!(outcome_for_err(&DriveError::Conflict("x".into())),   "conflict");
        assert_eq!(outcome_for_err(&DriveError::QuotaExceeded),         "quota_exceeded");
        assert_eq!(outcome_for_err(&DriveError::NotFound(Uuid::nil())), "not_found");
        assert_eq!(outcome_for_err(&DriveError::Forbidden),             "forbidden");
        assert_eq!(outcome_for_err(&DriveError::DatabaseUnavailable),   "error");
    }
}

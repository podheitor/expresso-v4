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
        DriveError::Unauthorized => "unauthorized",
        DriveError::BadRequest(_) => "bad_request",
        DriveError::Conflict(_) => "conflict",
        DriveError::QuotaExceeded => "quota_exceeded",
        DriveError::NotFound(_) => "not_found",
        DriveError::Forbidden => "forbidden",
        _ => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn err_outcome_mapping() {
        assert_eq!(outcome_for_err(&DriveError::Unauthorized), "unauthorized");
        assert_eq!(
            outcome_for_err(&DriveError::BadRequest("x".into())),
            "bad_request"
        );
        assert_eq!(
            outcome_for_err(&DriveError::Conflict("x".into())),
            "conflict"
        );
        assert_eq!(
            outcome_for_err(&DriveError::QuotaExceeded),
            "quota_exceeded"
        );
        assert_eq!(
            outcome_for_err(&DriveError::NotFound(Uuid::nil())),
            "not_found"
        );
        assert_eq!(outcome_for_err(&DriveError::Forbidden), "forbidden");
        assert_eq!(outcome_for_err(&DriveError::DatabaseUnavailable), "error");
    }

    #[test]
    fn ops_list_contains_canonical() {
        assert!(OPS.contains(&"check_file_info"));
        assert!(OPS.contains(&"get_file"));
        assert!(OPS.contains(&"put_file"));
        assert!(OPS.contains(&"lock"));
        assert!(OPS.contains(&"unlock"));
        assert!(OPS.contains(&"other"));
    }

    #[test]
    fn outcomes_list_contains_canonical() {
        assert!(OUTCOMES.contains(&"ok"));
        assert!(OUTCOMES.contains(&"conflict"));
        assert!(OUTCOMES.contains(&"quota_exceeded"));
        assert!(OUTCOMES.contains(&"not_found"));
        assert!(OUTCOMES.contains(&"error"));
    }

    #[test]
    fn io_error_maps_to_error() {
        let e = DriveError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file missing",
        ));
        assert_eq!(outcome_for_err(&e), "error");
    }

    #[test]
    fn ops_list_contains_get_file() {
        assert!(OPS.contains(&"get_file"));
        assert!(OPS.contains(&"put_file"));
        assert!(OPS.contains(&"check_file_info"));
    }

    #[test]
    fn outcomes_list_contains_ok_and_error() {
        assert!(OUTCOMES.contains(&"ok"));
        assert!(OUTCOMES.contains(&"error"));
    }

    #[test]
    fn ops_list_contains_refresh_and_relock() {
        assert!(OPS.contains(&"refresh_lock"));
        assert!(OPS.contains(&"unlock_and_relock"));
        assert!(OPS.contains(&"get_lock"));
    }

    #[test]
    fn outcomes_list_contains_unauthorized_and_forbidden() {
        assert!(OUTCOMES.contains(&"unauthorized"));
        assert!(OUTCOMES.contains(&"forbidden"));
        assert!(OUTCOMES.contains(&"bad_request"));
    }

    #[test]
    fn outcomes_list_contains_ok() {
        assert!(OUTCOMES.contains(&"ok"));
    }

    #[test]
    fn outcomes_list_contains_quota_exceeded() {
        assert!(OUTCOMES.contains(&"quota_exceeded"));
    }

    #[test]
    fn outcomes_list_contains_not_found() {
        assert!(OUTCOMES.contains(&"not_found"));
    }

    #[test]
    fn outcomes_list_contains_forbidden() {
        assert!(OUTCOMES.contains(&"forbidden"));
    }

    #[test]
    fn outcomes_list_has_eight_entries() {
        assert_eq!(OUTCOMES.len(), 8);
    }

    #[test]
    fn outcomes_contains_not_found() {
        assert!(OUTCOMES.contains(&"not_found"));
    }

    #[test]
    fn ops_list_contains_unlock_and_relock() {
        assert!(OPS.contains(&"unlock_and_relock"));
    }

    #[test]
    fn ops_list_contains_other() {
        assert!(OPS.contains(&"other"));
    }

    #[test]
    fn ops_list_length_is_at_least_six() {
        assert!(OPS.len() >= 6);
    }

    #[test]
    fn outcomes_list_length_is_eight() {
        assert_eq!(OUTCOMES.len(), 8);
    }

    #[test]
    fn ops_list_contains_check_file_info() {
        assert!(OPS.contains(&"check_file_info"));
    }

    #[test]
    fn ops_list_has_nine_entries() {
        assert_eq!(OPS.len(), 9);
    }

    #[test]
    fn outcomes_list_does_not_contain_empty_string() {
        assert!(!OUTCOMES.contains(&""));
    }

    #[test]
    fn ops_list_contains_put_file() {
        assert!(OPS.contains(&"put_file"));
    }

    #[test]
    fn ops_list_does_not_contain_empty_string() {
        assert!(!OPS.contains(&""));
    }

    #[test]
    fn outcomes_list_has_at_least_two_entries() {
        assert!(OUTCOMES.len() >= 2);
    }
}

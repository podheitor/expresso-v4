//! Drive service Prometheus metrics.
//!
//! Single counter `expresso_drive_ops_total{op, outcome}` covers handler
//! outcomes. `op` is one of: file_upload, file_download, file_delete,
//! file_list, share_create, share_revoke, mkdir. `outcome` is `ok` on success
//! or a mapped error label (see `outcome_for_err`), including quota_exceeded.

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

use crate::error::DriveError;

/// Metric namespace for all drive service metrics.
pub const NAMESPACE: &str = "expresso_drive";

/// Label for file-upload operations.
pub const OP_FILE_UPLOAD: &str = "file_upload";
/// Label for file-download operations.
pub const OP_FILE_DOWNLOAD: &str = "file_download";
/// Label for file-delete operations.
pub const OP_FILE_DELETE: &str = "file_delete";
/// Label for file-list operations.
pub const OP_FILE_LIST: &str = "file_list";
/// Label for share-create operations.
pub const OP_SHARE_CREATE: &str = "share_create";
/// Label for share-revoke operations.
pub const OP_SHARE_REVOKE: &str = "share_revoke";
/// Label for folder-create (mkdir) operations.
pub const OP_MKDIR: &str = "mkdir";
/// Outcome label for a quota-exceeded rejection.
pub const EVENT_QUOTA_EXCEEDED: &str = "quota_exceeded";

const OPS: &[&str] = &[
    OP_FILE_UPLOAD,
    OP_FILE_DOWNLOAD,
    OP_FILE_DELETE,
    OP_FILE_LIST,
    OP_SHARE_CREATE,
    OP_SHARE_REVOKE,
    OP_MKDIR,
];

const OUTCOMES: &[&str] = &[
    "ok",
    "not_found",
    "gone",
    "conflict",
    "bad_request",
    "forbidden",
    "unauthorized",
    EVENT_QUOTA_EXCEEDED,
    "unavailable",
    "error",
];

pub static DRIVE_OPS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            metric_name("ops_total"),
            "Drive handler outcomes per operation",
        ),
        &["op", "outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

/// Returns the full Prometheus metric name for a given base name.
pub fn metric_name(base: &str) -> String {
    format!("{NAMESPACE}_{base}")
}

/// Pre-populate label series so `rate()`/`increase()` work from the first
/// scrape. Idempotent.
pub fn init() {
    Lazy::force(&DRIVE_OPS_TOTAL);
    for op in OPS {
        for outcome in OUTCOMES {
            DRIVE_OPS_TOTAL.with_label_values(&[op, outcome]).inc_by(0);
        }
    }
}

/// Record one handler outcome.
#[inline]
pub fn record(op: &'static str, outcome: &'static str) {
    DRIVE_OPS_TOTAL.with_label_values(&[op, outcome]).inc();
}

/// Map a `DriveError` to the canonical `outcome` label.
pub fn outcome_for_err(e: &DriveError) -> &'static str {
    match e {
        DriveError::NotFound(_) => "not_found",
        DriveError::Gone(_) => "gone",
        DriveError::Conflict(_) => "conflict",
        DriveError::BadRequest(_) => "bad_request",
        DriveError::Forbidden => "forbidden",
        DriveError::Unauthorized => "unauthorized",
        DriveError::QuotaExceeded => EVENT_QUOTA_EXCEEDED,
        DriveError::DatabaseUnavailable => "unavailable",
        _ => "error",
    }
}

/// Record `ok` on success or the mapped error label on failure.
pub fn record_result<T>(op: &'static str, result: &Result<T, DriveError>) {
    match result {
        Ok(_) => record(op, "ok"),
        Err(e) => record(op, outcome_for_err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn namespace_value() {
        assert_eq!(NAMESPACE, "expresso_drive");
    }

    #[test]
    fn op_values() {
        assert_eq!(OP_FILE_UPLOAD, "file_upload");
        assert_eq!(OP_FILE_DOWNLOAD, "file_download");
        assert_eq!(OP_FILE_DELETE, "file_delete");
        assert_eq!(OP_FILE_LIST, "file_list");
        assert_eq!(OP_SHARE_CREATE, "share_create");
        assert_eq!(OP_SHARE_REVOKE, "share_revoke");
        assert_eq!(OP_MKDIR, "mkdir");
    }

    #[test]
    fn metric_name_prefixes_namespace() {
        assert_eq!(
            metric_name("requests_total"),
            "expresso_drive_requests_total"
        );
        assert!(metric_name("x").starts_with(NAMESPACE));
        assert_eq!(metric_name("latency"), format!("{NAMESPACE}_latency"));
    }

    #[test]
    fn op_labels_distinct_lowercase() {
        for (i, a) in OPS.iter().enumerate() {
            assert_eq!(*a, a.to_lowercase());
            assert!(!a.contains(' '));
            for (j, b) in OPS.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn outcomes_distinct_nonempty() {
        for (i, a) in OUTCOMES.iter().enumerate() {
            assert!(!a.is_empty());
            for (j, b) in OUTCOMES.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn outcome_for_err_maps_variants() {
        assert_eq!(
            outcome_for_err(&DriveError::NotFound(Uuid::nil())),
            "not_found"
        );
        assert_eq!(outcome_for_err(&DriveError::Gone(Uuid::nil())), "gone");
        assert_eq!(
            outcome_for_err(&DriveError::Conflict("x".into())),
            "conflict"
        );
        assert_eq!(
            outcome_for_err(&DriveError::BadRequest("x".into())),
            "bad_request"
        );
        assert_eq!(outcome_for_err(&DriveError::Forbidden), "forbidden");
        assert_eq!(outcome_for_err(&DriveError::Unauthorized), "unauthorized");
        assert_eq!(
            outcome_for_err(&DriveError::QuotaExceeded),
            "quota_exceeded"
        );
        assert_eq!(
            outcome_for_err(&DriveError::DatabaseUnavailable),
            "unavailable"
        );
    }

    #[test]
    fn record_paths_do_not_panic() {
        init();
        init();
        record(OP_FILE_UPLOAD, "ok");
        let ok: Result<(), DriveError> = Ok(());
        record_result(OP_FILE_UPLOAD, &ok);
        let err: Result<(), DriveError> = Err(DriveError::QuotaExceeded);
        record_result(OP_FILE_UPLOAD, &err);
    }

    #[test]
    fn ops_and_outcomes_counts() {
        assert_eq!(OPS.len(), 7);
        assert_eq!(OUTCOMES.len(), 10);
    }
}

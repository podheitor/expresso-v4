//! Calendar service Prometheus metrics.
//!
//! Single counter `expresso_calendar_ops_total{op, outcome}` covers handler
//! outcomes. `op` is one of: event_create, event_update, event_delete,
//! calendar_list, share, export_ical. `outcome` is `ok` on success or a
//! mapped error label (see `outcome_for_err`).

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

use crate::error::CalendarError;

/// Metric namespace for all calendar service metrics.
pub const NAMESPACE: &str = "expresso_calendar";

/// Label value for event-create operations.
pub const OP_EVENT_CREATE: &str = "event_create";
/// Label value for event-update operations.
pub const OP_EVENT_UPDATE: &str = "event_update";
/// Label value for event-delete operations.
pub const OP_EVENT_DELETE: &str = "event_delete";
/// Label value for calendar-list operations.
pub const OP_CALENDAR_LIST: &str = "calendar_list";
/// Label value for sharing operations.
pub const OP_SHARE: &str = "share";
/// Label value for iCal export operations.
pub const OP_EXPORT_ICAL: &str = "export_ical";

const OPS: &[&str] = &[
    OP_EVENT_CREATE,
    OP_EVENT_UPDATE,
    OP_EVENT_DELETE,
    OP_CALENDAR_LIST,
    OP_SHARE,
    OP_EXPORT_ICAL,
];

const OUTCOMES: &[&str] = &[
    "ok",
    "not_found",
    "bad_request",
    "conflict",
    "forbidden",
    "not_supported",
    "unavailable",
    "error",
];

pub static CALENDAR_OPS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            metric_name("ops_total"),
            "Calendar handler outcomes per operation",
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
    Lazy::force(&CALENDAR_OPS_TOTAL);
    for op in OPS {
        for outcome in OUTCOMES {
            CALENDAR_OPS_TOTAL
                .with_label_values(&[op, outcome])
                .inc_by(0);
        }
    }
}

/// Record one handler outcome.
#[inline]
pub fn record(op: &'static str, outcome: &'static str) {
    CALENDAR_OPS_TOTAL.with_label_values(&[op, outcome]).inc();
}

/// Map a `CalendarError` to the canonical `outcome` label.
pub fn outcome_for_err(e: &CalendarError) -> &'static str {
    match e {
        CalendarError::EventNotFound(_)
        | CalendarError::CalendarNotFound(_)
        | CalendarError::AlarmNotFound(_) => "not_found",
        CalendarError::InvalidICal(_) | CalendarError::BadRequest(_) => "bad_request",
        CalendarError::Conflict(_) => "conflict",
        CalendarError::Forbidden => "forbidden",
        CalendarError::NotSupported(_) => "not_supported",
        CalendarError::DatabaseUnavailable => "unavailable",
        _ => "error",
    }
}

/// Record `ok` on success or the mapped error label on failure.
pub fn record_result<T>(op: &'static str, result: &Result<T, CalendarError>) {
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
        assert_eq!(NAMESPACE, "expresso_calendar");
    }

    #[test]
    fn op_values() {
        assert_eq!(OP_EVENT_CREATE, "event_create");
        assert_eq!(OP_EVENT_UPDATE, "event_update");
        assert_eq!(OP_EVENT_DELETE, "event_delete");
        assert_eq!(OP_CALENDAR_LIST, "calendar_list");
        assert_eq!(OP_SHARE, "share");
        assert_eq!(OP_EXPORT_ICAL, "export_ical");
    }

    #[test]
    fn metric_name_prefixes_namespace() {
        assert_eq!(
            metric_name("requests_total"),
            "expresso_calendar_requests_total"
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
            outcome_for_err(&CalendarError::EventNotFound(Uuid::nil())),
            "not_found"
        );
        assert_eq!(
            outcome_for_err(&CalendarError::CalendarNotFound("c".into())),
            "not_found"
        );
        assert_eq!(
            outcome_for_err(&CalendarError::InvalidICal("x".into())),
            "bad_request"
        );
        assert_eq!(
            outcome_for_err(&CalendarError::BadRequest("x".into())),
            "bad_request"
        );
        assert_eq!(
            outcome_for_err(&CalendarError::Conflict("x".into())),
            "conflict"
        );
        assert_eq!(outcome_for_err(&CalendarError::Forbidden), "forbidden");
        assert_eq!(
            outcome_for_err(&CalendarError::NotSupported("x")),
            "not_supported"
        );
        assert_eq!(
            outcome_for_err(&CalendarError::DatabaseUnavailable),
            "unavailable"
        );
    }

    #[test]
    fn record_paths_do_not_panic() {
        init();
        init();
        record(OP_EVENT_CREATE, "ok");
        let ok: Result<(), CalendarError> = Ok(());
        record_result(OP_EVENT_CREATE, &ok);
        let err: Result<(), CalendarError> = Err(CalendarError::Forbidden);
        record_result(OP_EVENT_DELETE, &err);
    }

    #[test]
    fn ops_and_outcomes_counts() {
        assert_eq!(OPS.len(), 6);
        assert_eq!(OUTCOMES.len(), 8);
    }
}

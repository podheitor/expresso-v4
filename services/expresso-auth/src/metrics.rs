//! Prometheus metrics for expresso-auth.
//!
//! - `expresso_auth_requests_total{operation, status}` — per-operation request
//!   counter (operation ∈ login/callback/refresh/logout/forgot/
//!   impersonate_start/impersonate_end; status ∈ ok/error).
//! - `expresso_auth_errors_total{operation}` — error-only counter, a
//!   convenience for error-rate alerts without a status filter.
//! - `expresso_auth_latency_ms{operation}` — request latency histogram.
//!
//! `record_result(op, started, &result)` is the single entry point used by the
//! handlers: it derives the status label, bumps the counters, and observes the
//! elapsed latency. `record(op, started, ok)` is the boolean variant for
//! handlers that don't return a `Result` (e.g. the always-204 `forgot` flow).

use std::time::Instant;

use once_cell::sync::Lazy;
use prometheus::{HistogramVec, IntCounterVec};

pub const METRIC_AUTH_REQUESTS: &str = "expresso_auth_requests_total";
pub const METRIC_AUTH_ERRORS: &str = "expresso_auth_errors_total";
pub const METRIC_AUTH_LATENCY_MS: &str = "expresso_auth_latency_ms";

pub const LABEL_OPERATION: &str = "operation";
pub const LABEL_STATUS: &str = "status";

pub const OP_LOGIN: &str = "login";
pub const OP_CALLBACK: &str = "callback";
pub const OP_REFRESH: &str = "refresh";
pub const OP_LOGOUT: &str = "logout";
pub const OP_FORGOT: &str = "forgot";
pub const OP_IMPERSONATE_START: &str = "impersonate_start";
pub const OP_IMPERSONATE_END: &str = "impersonate_end";

const OPS: &[&str] = &[
    OP_LOGIN,
    OP_CALLBACK,
    OP_REFRESH,
    OP_LOGOUT,
    OP_FORGOT,
    OP_IMPERSONATE_START,
    OP_IMPERSONATE_END,
];
const STATUSES: &[&str] = &["ok", "error"];

static REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            METRIC_AUTH_REQUESTS,
            "Auth requests per operation and status",
        ),
        &[LABEL_OPERATION, LABEL_STATUS],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

static ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(METRIC_AUTH_ERRORS, "Auth errors per operation"),
        &[LABEL_OPERATION],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

static LATENCY_MS: Lazy<HistogramVec> = Lazy::new(|| {
    let h = HistogramVec::new(
        prometheus::HistogramOpts::new(METRIC_AUTH_LATENCY_MS, "Auth request latency (ms)")
            .buckets(vec![
                1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 5000.0,
            ]),
        &[LABEL_OPERATION],
    )
    .expect("metric build");
    expresso_observability::register(h)
});

/// Pre-populate label series so `rate()`/`increase()` work from the first
/// scrape. Idempotent.
pub fn init() {
    Lazy::force(&REQUESTS_TOTAL);
    Lazy::force(&ERRORS_TOTAL);
    Lazy::force(&LATENCY_MS);
    for op in OPS {
        for status in STATUSES {
            REQUESTS_TOTAL.with_label_values(&[op, status]).inc_by(0);
        }
        ERRORS_TOTAL.with_label_values(&[op]).inc_by(0);
    }
}

/// Record a completed operation: bump the request counter (and error counter on
/// failure) and observe latency since `started`. `op` is one of the `OP_*`
/// labels; `ok` is the success flag.
pub fn record(op: &'static str, started: Instant, ok: bool) {
    let status = if ok { "ok" } else { "error" };
    REQUESTS_TOTAL.with_label_values(&[op, status]).inc();
    if !ok {
        ERRORS_TOTAL.with_label_values(&[op]).inc();
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    LATENCY_MS.with_label_values(&[op]).observe(elapsed_ms);
}

/// `record` for handlers that return a `Result`: success is `result.is_ok()`.
pub fn record_result<T, E>(op: &'static str, started: Instant, result: &Result<T, E>) {
    record(op, started, result.is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_names_have_expresso_prefix() {
        for m in [
            METRIC_AUTH_REQUESTS,
            METRIC_AUTH_ERRORS,
            METRIC_AUTH_LATENCY_MS,
        ] {
            assert!(m.starts_with("expresso_auth_"), "{m}");
        }
    }

    #[test]
    fn counter_metrics_end_with_total() {
        assert!(METRIC_AUTH_REQUESTS.ends_with("_total"));
        assert!(METRIC_AUTH_ERRORS.ends_with("_total"));
    }

    #[test]
    fn histogram_not_total() {
        assert!(!METRIC_AUTH_LATENCY_MS.ends_with("_total"));
    }

    #[test]
    fn label_values() {
        assert_eq!(LABEL_OPERATION, "operation");
        assert_eq!(LABEL_STATUS, "status");
        assert_ne!(LABEL_OPERATION, LABEL_STATUS);
    }

    #[test]
    fn ops_distinct_lowercase() {
        let mut sorted = OPS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), OPS.len());
        for op in OPS {
            assert_eq!(*op, op.to_ascii_lowercase());
            assert!(!op.contains('-'));
        }
    }

    #[test]
    fn metric_names_all_distinct() {
        let names = [
            METRIC_AUTH_REQUESTS,
            METRIC_AUTH_ERRORS,
            METRIC_AUTH_LATENCY_MS,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn record_paths_do_not_panic() {
        init();
        init();
        record(OP_LOGIN, Instant::now(), true);
        record(OP_REFRESH, Instant::now(), false);
        let ok: Result<(), ()> = Ok(());
        record_result(OP_CALLBACK, Instant::now(), &ok);
        let err: Result<(), ()> = Err(());
        record_result(OP_IMPERSONATE_START, Instant::now(), &err);
    }

    #[test]
    fn statuses_are_ok_and_error() {
        assert_eq!(STATUSES, &["ok", "error"]);
    }
}

//! Prometheus metrics for expresso-search.
//!
//! - `expresso_search_requests_total{operation, status}` — per-operation
//!   request counter (operation ∈ search/index/delete/bulk_index;
//!   status ∈ ok/error).
//! - `expresso_search_errors_total{operation}` — error-only counter, a
//!   convenience for error-rate alerts without a status filter.
//! - `expresso_search_latency_ms{operation}` — request latency histogram.
//!
//! `record_result(op, started_at, &result)` is the single entry point used by
//! the handlers: it derives the status label, bumps the counters, and observes
//! the elapsed latency. A per-index `doc_count` gauge is reserved for a future
//! index-introspection probe and intentionally not emitted yet.

use std::time::Instant;

use once_cell::sync::Lazy;
use prometheus::{HistogramVec, IntCounterVec};

pub const METRIC_SEARCH_REQUESTS: &str = "expresso_search_requests_total";
pub const METRIC_SEARCH_ERRORS: &str = "expresso_search_errors_total";
pub const METRIC_SEARCH_LATENCY_MS: &str = "expresso_search_latency_ms";

pub const LABEL_OPERATION: &str = "operation";
pub const LABEL_STATUS: &str = "status";

pub const OP_SEARCH: &str = "search";
pub const OP_INDEX: &str = "index";
pub const OP_DELETE: &str = "delete";
pub const OP_BULK_INDEX: &str = "bulk_index";

const OPS: &[&str] = &[OP_SEARCH, OP_INDEX, OP_DELETE, OP_BULK_INDEX];
const STATUSES: &[&str] = &["ok", "error"];

static REQUESTS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            METRIC_SEARCH_REQUESTS,
            "Search requests per operation and status",
        ),
        &[LABEL_OPERATION, LABEL_STATUS],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

static ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(METRIC_SEARCH_ERRORS, "Search errors per operation"),
        &[LABEL_OPERATION],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

static LATENCY_MS: Lazy<HistogramVec> = Lazy::new(|| {
    let h = HistogramVec::new(
        prometheus::HistogramOpts::new(METRIC_SEARCH_LATENCY_MS, "Search request latency (ms)")
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
/// labels; success is taken from `result.is_ok()`.
pub fn record_result<T, E>(op: &'static str, started: Instant, result: &Result<T, E>) {
    let status = if result.is_ok() { "ok" } else { "error" };
    REQUESTS_TOTAL.with_label_values(&[op, status]).inc();
    if result.is_err() {
        ERRORS_TOTAL.with_label_values(&[op]).inc();
    }
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    LATENCY_MS.with_label_values(&[op]).observe(elapsed_ms);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_names_have_expresso_prefix() {
        for m in [
            METRIC_SEARCH_REQUESTS,
            METRIC_SEARCH_ERRORS,
            METRIC_SEARCH_LATENCY_MS,
        ] {
            assert!(m.starts_with("expresso_"), "{m}");
        }
    }

    #[test]
    fn counter_metrics_end_with_total() {
        assert!(METRIC_SEARCH_REQUESTS.ends_with("_total"));
        assert!(METRIC_SEARCH_ERRORS.ends_with("_total"));
    }

    #[test]
    fn histogram_not_total() {
        assert!(!METRIC_SEARCH_LATENCY_MS.ends_with("_total"));
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
            METRIC_SEARCH_REQUESTS,
            METRIC_SEARCH_ERRORS,
            METRIC_SEARCH_LATENCY_MS,
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
        let ok: Result<(), ()> = Ok(());
        record_result(OP_SEARCH, Instant::now(), &ok);
        let err: Result<(), ()> = Err(());
        record_result(OP_BULK_INDEX, Instant::now(), &err);
    }

    #[test]
    fn statuses_are_ok_and_error() {
        assert_eq!(STATUSES, &["ok", "error"]);
    }
}

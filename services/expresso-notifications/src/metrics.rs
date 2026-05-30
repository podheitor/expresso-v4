//! Prometheus metrics for expresso-notifications.
//!
//! - `notifications_dispatched_total{kind}` — notifications fanned out to the
//!   local SSE bus + Redis relay, by event kind (new_mail/flags_changed/…).
//! - `notifications_failed_total{kind}` — dispatches where the Redis relay
//!   publish failed, by kind. A nonzero rate means other pods may miss events.
//! - `notifications_dispatch_duration_ms` — wall-clock of a single dispatch
//!   (local broadcast + Redis publish) as a histogram.
//!
//! `record_dispatch(kind)` bumps the dispatched counter; `record_failure(kind)`
//! bumps the failed counter; `observe_dispatch(started)` records latency. All
//! three register into the shared `expresso_observability` (default) registry,
//! so they appear on the service's `/metrics` endpoint.

use std::time::Instant;

use once_cell::sync::Lazy;
use prometheus::{HistogramVec, IntCounterVec};

pub const METRIC_NOTIFICATIONS_DISPATCHED: &str = "notifications_dispatched_total";
pub const METRIC_NOTIFICATIONS_FAILED: &str = "notifications_failed_total";
pub const METRIC_DISPATCH_DURATION_MS: &str = "notifications_dispatch_duration_ms";

pub const LABEL_KIND: &str = "kind";

static DISPATCHED_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            METRIC_NOTIFICATIONS_DISPATCHED,
            "Total notifications dispatched, by kind",
        ),
        &[LABEL_KIND],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

static FAILED_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            METRIC_NOTIFICATIONS_FAILED,
            "Notification dispatches whose Redis relay publish failed, by kind",
        ),
        &[LABEL_KIND],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

static DISPATCH_DURATION_MS: Lazy<HistogramVec> = Lazy::new(|| {
    let h = HistogramVec::new(
        prometheus::HistogramOpts::new(
            METRIC_DISPATCH_DURATION_MS,
            "Notification dispatch latency (ms)",
        )
        .buckets(vec![
            0.5, 1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0,
        ]),
        &[],
    )
    .expect("metric build");
    expresso_observability::register(h)
});

/// Force lazy init so the metric families appear on the first scrape even
/// before any dispatch. Idempotent.
pub fn init() {
    Lazy::force(&DISPATCHED_TOTAL);
    Lazy::force(&FAILED_TOTAL);
    Lazy::force(&DISPATCH_DURATION_MS);
}

/// Count one dispatched notification of `kind`.
pub fn record_dispatch(kind: &str) {
    DISPATCHED_TOTAL.with_label_values(&[kind]).inc();
}

/// Count one dispatch whose Redis relay publish failed.
pub fn record_failure(kind: &str) {
    FAILED_TOTAL.with_label_values(&[kind]).inc();
}

/// Observe the elapsed dispatch latency since `started`.
pub fn observe_dispatch(started: Instant) {
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    DISPATCH_DURATION_MS
        .with_label_values(&[])
        .observe(elapsed_ms);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_names_have_notifications_prefix() {
        for m in [
            METRIC_NOTIFICATIONS_DISPATCHED,
            METRIC_NOTIFICATIONS_FAILED,
            METRIC_DISPATCH_DURATION_MS,
        ] {
            assert!(m.starts_with("notifications_"), "{m}");
        }
    }

    #[test]
    fn counter_metrics_end_with_total() {
        assert!(METRIC_NOTIFICATIONS_DISPATCHED.ends_with("_total"));
        assert!(METRIC_NOTIFICATIONS_FAILED.ends_with("_total"));
    }

    #[test]
    fn histogram_not_total() {
        assert!(!METRIC_DISPATCH_DURATION_MS.ends_with("_total"));
    }

    #[test]
    fn metric_names_all_distinct() {
        let names = [
            METRIC_NOTIFICATIONS_DISPATCHED,
            METRIC_NOTIFICATIONS_FAILED,
            METRIC_DISPATCH_DURATION_MS,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn label_kind_is_kind() {
        assert_eq!(LABEL_KIND, "kind");
    }

    #[test]
    fn record_paths_do_not_panic() {
        init();
        init();
        record_dispatch("new_mail");
        record_failure("new_mail");
        observe_dispatch(Instant::now());
    }
}

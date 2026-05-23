//! Prometheus metric name constants for expresso-notifications.

pub const METRIC_NOTIFICATIONS_DISPATCHED: &str = "expresso_notifications_dispatched_total";
pub const METRIC_NOTIFICATIONS_FAILED:     &str = "expresso_notifications_failed_total";
pub const METRIC_SUBSCRIPTIONS_ACTIVE:     &str = "expresso_notifications_subscriptions_active";
pub const METRIC_DISPATCH_DURATION_MS:     &str = "expresso_notifications_dispatch_duration_ms";

/// Labels used on the dispatched / failed counters.
pub const LABEL_CHANNEL: &str = "channel";
pub const LABEL_STATUS:  &str = "status";

/// Known channel values for label cardinality control.
pub const CHANNEL_PUSH:      &str = "push";
pub const CHANNEL_EMAIL:     &str = "email";
pub const CHANNEL_WEBSOCKET: &str = "websocket";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatched_metric_has_expresso_prefix() {
        assert!(METRIC_NOTIFICATIONS_DISPATCHED.starts_with("expresso_"));
    }

    #[test]
    fn failed_metric_has_expresso_prefix() {
        assert!(METRIC_NOTIFICATIONS_FAILED.starts_with("expresso_"));
    }

    #[test]
    fn subscriptions_active_metric_has_expresso_prefix() {
        assert!(METRIC_SUBSCRIPTIONS_ACTIVE.starts_with("expresso_"));
    }

    #[test]
    fn dispatch_duration_metric_has_expresso_prefix() {
        assert!(METRIC_DISPATCH_DURATION_MS.starts_with("expresso_"));
    }

    #[test]
    fn dispatched_metric_ends_with_total() {
        assert!(METRIC_NOTIFICATIONS_DISPATCHED.ends_with("_total"));
    }

    #[test]
    fn failed_metric_ends_with_total() {
        assert!(METRIC_NOTIFICATIONS_FAILED.ends_with("_total"));
    }

    #[test]
    fn label_channel_is_channel() {
        assert_eq!(LABEL_CHANNEL, "channel");
    }

    #[test]
    fn label_status_is_status() {
        assert_eq!(LABEL_STATUS, "status");
    }

    #[test]
    fn channel_push_value() {
        assert_eq!(CHANNEL_PUSH, "push");
    }

    #[test]
    fn channel_email_value() {
        assert_eq!(CHANNEL_EMAIL, "email");
    }

    #[test]
    fn channel_websocket_value() {
        assert_eq!(CHANNEL_WEBSOCKET, "websocket");
    }

    #[test]
    fn all_channel_values_are_distinct() {
        let channels = [CHANNEL_PUSH, CHANNEL_EMAIL, CHANNEL_WEBSOCKET];
        let mut sorted = channels.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn metric_names_are_nonempty() {
        assert!(!METRIC_NOTIFICATIONS_DISPATCHED.is_empty());
        assert!(!METRIC_NOTIFICATIONS_FAILED.is_empty());
        assert!(!METRIC_SUBSCRIPTIONS_ACTIVE.is_empty());
        assert!(!METRIC_DISPATCH_DURATION_MS.is_empty());
    }

    #[test]
    fn dispatched_metric_contains_notifications() {
        assert!(METRIC_NOTIFICATIONS_DISPATCHED.contains("notifications"));
    }

    #[test]
    fn failed_metric_contains_notifications() {
        assert!(METRIC_NOTIFICATIONS_FAILED.contains("notifications"));
    }

    #[test]
    fn subscriptions_metric_contains_subscriptions() {
        assert!(METRIC_SUBSCRIPTIONS_ACTIVE.contains("subscriptions"));
    }

    #[test]
    fn duration_metric_contains_duration() {
        assert!(METRIC_DISPATCH_DURATION_MS.contains("duration"));
    }

    #[test]
    fn label_channel_nonempty() {
        assert!(!LABEL_CHANNEL.is_empty());
    }

    #[test]
    fn label_status_nonempty() {
        assert!(!LABEL_STATUS.is_empty());
    }

    #[test]
    fn all_metric_names_use_snake_case() {
        for m in [
            METRIC_NOTIFICATIONS_DISPATCHED,
            METRIC_NOTIFICATIONS_FAILED,
            METRIC_SUBSCRIPTIONS_ACTIVE,
            METRIC_DISPATCH_DURATION_MS,
        ] {
            assert!(!m.contains('-'), "metric contains hyphen: {m}");
            assert_eq!(m, m.to_ascii_lowercase(), "metric not lowercase: {m}");
        }
    }

    #[test]
    fn dispatched_and_failed_metrics_differ() {
        assert_ne!(METRIC_NOTIFICATIONS_DISPATCHED, METRIC_NOTIFICATIONS_FAILED);
    }

    #[test]
    fn channel_labels_are_lowercase() {
        assert_eq!(CHANNEL_PUSH, CHANNEL_PUSH.to_ascii_lowercase());
        assert_eq!(CHANNEL_EMAIL, CHANNEL_EMAIL.to_ascii_lowercase());
        assert_eq!(CHANNEL_WEBSOCKET, CHANNEL_WEBSOCKET.to_ascii_lowercase());
    }

    #[test]
    fn dispatch_duration_metric_contains_dispatch() {
        assert!(METRIC_DISPATCH_DURATION_MS.contains("dispatch"));
    }

    #[test]
    fn subscriptions_active_metric_does_not_end_with_total() {
        assert!(!METRIC_SUBSCRIPTIONS_ACTIVE.ends_with("_total"));
    }

    #[test]
    fn channel_push_and_email_are_distinct() {
        assert_ne!(CHANNEL_PUSH, CHANNEL_EMAIL);
    }
}

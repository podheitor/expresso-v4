//! Prometheus metric name constants for expresso-web.

pub const METRIC_HTTP_REQUESTS:  &str = "expresso_web_http_requests_total";
pub const METRIC_HTTP_ERRORS:    &str = "expresso_web_http_errors_total";
pub const METRIC_PROXY_LATENCY_MS: &str = "expresso_web_proxy_latency_ms";
pub const METRIC_WOPI_TOKENS:    &str = "expresso_web_wopi_tokens_issued_total";
pub const METRIC_ACTIVE_SESSIONS: &str = "expresso_web_active_sessions";

pub const LABEL_METHOD:    &str = "method";
pub const LABEL_STATUS:    &str = "status";
pub const LABEL_UPSTREAM:  &str = "upstream";

pub const UPSTREAM_AUTH:     &str = "auth";
pub const UPSTREAM_MAIL:     &str = "mail";
pub const UPSTREAM_CALENDAR: &str = "calendar";
pub const UPSTREAM_CONTACTS: &str = "contacts";
pub const UPSTREAM_DRIVE:    &str = "drive";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_requests_metric_has_expresso_prefix() {
        assert!(METRIC_HTTP_REQUESTS.starts_with("expresso_"));
    }

    #[test]
    fn http_errors_metric_has_expresso_prefix() {
        assert!(METRIC_HTTP_ERRORS.starts_with("expresso_"));
    }

    #[test]
    fn proxy_latency_metric_has_expresso_prefix() {
        assert!(METRIC_PROXY_LATENCY_MS.starts_with("expresso_"));
    }

    #[test]
    fn wopi_tokens_metric_has_expresso_prefix() {
        assert!(METRIC_WOPI_TOKENS.starts_with("expresso_"));
    }

    #[test]
    fn active_sessions_metric_has_expresso_prefix() {
        assert!(METRIC_ACTIVE_SESSIONS.starts_with("expresso_"));
    }

    #[test]
    fn counter_metrics_end_with_total() {
        assert!(METRIC_HTTP_REQUESTS.ends_with("_total"));
        assert!(METRIC_HTTP_ERRORS.ends_with("_total"));
        assert!(METRIC_WOPI_TOKENS.ends_with("_total"));
    }

    #[test]
    fn label_method_value() {
        assert_eq!(LABEL_METHOD, "method");
    }

    #[test]
    fn label_status_value() {
        assert_eq!(LABEL_STATUS, "status");
    }

    #[test]
    fn label_upstream_value() {
        assert_eq!(LABEL_UPSTREAM, "upstream");
    }

    #[test]
    fn upstream_auth_value() {
        assert_eq!(UPSTREAM_AUTH, "auth");
    }

    #[test]
    fn upstream_mail_value() {
        assert_eq!(UPSTREAM_MAIL, "mail");
    }

    #[test]
    fn upstream_calendar_value() {
        assert_eq!(UPSTREAM_CALENDAR, "calendar");
    }

    #[test]
    fn upstream_contacts_value() {
        assert_eq!(UPSTREAM_CONTACTS, "contacts");
    }

    #[test]
    fn upstream_drive_value() {
        assert_eq!(UPSTREAM_DRIVE, "drive");
    }

    #[test]
    fn all_upstreams_are_distinct() {
        let ups = [UPSTREAM_AUTH, UPSTREAM_MAIL, UPSTREAM_CALENDAR, UPSTREAM_CONTACTS, UPSTREAM_DRIVE];
        let mut sorted = ups.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 5);
    }

    #[test]
    fn all_metrics_nonempty() {
        for m in [
            METRIC_HTTP_REQUESTS, METRIC_HTTP_ERRORS, METRIC_PROXY_LATENCY_MS,
            METRIC_WOPI_TOKENS, METRIC_ACTIVE_SESSIONS,
        ] {
            assert!(!m.is_empty());
        }
    }

    #[test]
    fn all_metrics_lowercase_snake_case() {
        for m in [
            METRIC_HTTP_REQUESTS, METRIC_HTTP_ERRORS, METRIC_PROXY_LATENCY_MS,
            METRIC_WOPI_TOKENS, METRIC_ACTIVE_SESSIONS,
        ] {
            assert!(!m.contains('-'), "metric contains hyphen: {m}");
            assert_eq!(m, m.to_ascii_lowercase(), "metric not lowercase: {m}");
        }
    }

    #[test]
    fn web_metrics_contain_web() {
        for m in [
            METRIC_HTTP_REQUESTS, METRIC_HTTP_ERRORS, METRIC_PROXY_LATENCY_MS,
            METRIC_WOPI_TOKENS, METRIC_ACTIVE_SESSIONS,
        ] {
            assert!(m.contains("web"), "metric missing 'web': {m}");
        }
    }

    #[test]
    fn metrics_are_all_distinct() {
        let names = [
            METRIC_HTTP_REQUESTS, METRIC_HTTP_ERRORS, METRIC_PROXY_LATENCY_MS,
            METRIC_WOPI_TOKENS, METRIC_ACTIVE_SESSIONS,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 5);
    }

    #[test]
    fn labels_are_distinct() {
        let labels = [LABEL_METHOD, LABEL_STATUS, LABEL_UPSTREAM];
        let mut sorted = labels.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn upstreams_are_lowercase() {
        for up in [UPSTREAM_AUTH, UPSTREAM_MAIL, UPSTREAM_CALENDAR, UPSTREAM_CONTACTS, UPSTREAM_DRIVE] {
            assert_eq!(up, up.to_ascii_lowercase());
        }
    }

    #[test]
    fn active_sessions_metric_does_not_end_with_total() {
        assert!(!METRIC_ACTIVE_SESSIONS.ends_with("_total"));
    }

    #[test]
    fn proxy_latency_metric_does_not_end_with_total() {
        assert!(!METRIC_PROXY_LATENCY_MS.ends_with("_total"));
    }

    #[test]
    fn wopi_tokens_metric_contains_wopi() {
        assert!(METRIC_WOPI_TOKENS.contains("wopi"));
    }

    #[test]
    fn upstream_drive_and_upstream_mail_differ() {
        assert_ne!(UPSTREAM_DRIVE, UPSTREAM_MAIL);
    }

    #[test]
    fn http_requests_metric_contains_http() {
        assert!(METRIC_HTTP_REQUESTS.contains("http"));
    }
}

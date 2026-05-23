//! Auth service Prometheus metrics labels and helpers.

/// Metric label for successful authentication flows.
pub const AUTH_FLOW_SUCCESS: &str = "success";
/// Metric label for failed authentication flows.
pub const AUTH_FLOW_FAILURE: &str = "failure";
/// Metric label for the OIDC callback endpoint.
pub const ENDPOINT_CALLBACK: &str = "callback";
/// Metric label for the logout endpoint.
pub const ENDPOINT_LOGOUT: &str = "logout";
/// Metric label for the token-refresh endpoint.
pub const ENDPOINT_REFRESH: &str = "refresh";
/// Metric label for the userinfo endpoint.
pub const ENDPOINT_USERINFO: &str = "userinfo";

/// Returns the metric namespace shared by all auth service metrics.
pub fn namespace() -> &'static str {
    "expresso_auth"
}

/// Returns the full metric name for a given base name.
pub fn metric_name(base: &str) -> String {
    format!("{}_{}", namespace(), base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_flow_success_label_value() {
        assert_eq!(AUTH_FLOW_SUCCESS, "success");
    }

    #[test]
    fn auth_flow_failure_label_value() {
        assert_eq!(AUTH_FLOW_FAILURE, "failure");
    }

    #[test]
    fn endpoint_callback_label_value() {
        assert_eq!(ENDPOINT_CALLBACK, "callback");
    }

    #[test]
    fn endpoint_logout_label_value() {
        assert_eq!(ENDPOINT_LOGOUT, "logout");
    }

    #[test]
    fn endpoint_refresh_label_value() {
        assert_eq!(ENDPOINT_REFRESH, "refresh");
    }

    #[test]
    fn endpoint_userinfo_label_value() {
        assert_eq!(ENDPOINT_USERINFO, "userinfo");
    }

    #[test]
    fn namespace_returns_expresso_auth() {
        assert_eq!(namespace(), "expresso_auth");
    }

    #[test]
    fn metric_name_prefixes_with_namespace() {
        assert_eq!(metric_name("requests_total"), "expresso_auth_requests_total");
    }

    #[test]
    fn metric_name_callback_requests() {
        assert_eq!(metric_name("callback_requests"), "expresso_auth_callback_requests");
    }

    #[test]
    fn metric_name_logout_requests() {
        assert_eq!(metric_name("logout_requests"), "expresso_auth_logout_requests");
    }

    #[test]
    fn success_and_failure_labels_are_distinct() {
        assert_ne!(AUTH_FLOW_SUCCESS, AUTH_FLOW_FAILURE);
    }

    #[test]
    fn all_endpoint_labels_are_distinct() {
        let labels = [ENDPOINT_CALLBACK, ENDPOINT_LOGOUT, ENDPOINT_REFRESH, ENDPOINT_USERINFO];
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn metric_name_contains_base() {
        let base = "token_exchanges";
        assert!(metric_name(base).contains(base));
    }

    #[test]
    fn namespace_is_nonempty() {
        assert!(!namespace().is_empty());
    }

    #[test]
    fn metric_name_is_nonempty_for_nonempty_base() {
        assert!(!metric_name("latency_seconds").is_empty());
    }

    #[test]
    fn namespace_contains_no_spaces() {
        assert!(!namespace().contains(' '));
    }

    #[test]
    fn auth_flow_success_is_lowercase() {
        assert_eq!(AUTH_FLOW_SUCCESS, AUTH_FLOW_SUCCESS.to_lowercase());
    }

    #[test]
    fn auth_flow_failure_is_lowercase() {
        assert_eq!(AUTH_FLOW_FAILURE, AUTH_FLOW_FAILURE.to_lowercase());
    }

    #[test]
    fn endpoint_labels_are_lowercase() {
        for label in [ENDPOINT_CALLBACK, ENDPOINT_LOGOUT, ENDPOINT_REFRESH, ENDPOINT_USERINFO] {
            assert_eq!(label, label.to_lowercase());
        }
    }

    #[test]
    fn metric_name_separates_namespace_and_base_with_underscore() {
        let result = metric_name("foo");
        assert!(result.starts_with("expresso_auth_"));
        assert!(result.ends_with("foo"));
    }

    #[test]
    fn metric_name_does_not_double_namespace() {
        let result = metric_name("bar");
        assert!(!result.starts_with("expresso_auth_expresso_auth_"));
    }

    #[test]
    fn all_endpoint_labels_are_nonempty() {
        for label in [ENDPOINT_CALLBACK, ENDPOINT_LOGOUT, ENDPOINT_REFRESH, ENDPOINT_USERINFO] {
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn metric_name_logout_contains_logout() {
        assert!(metric_name("logout_count").contains("logout"));
    }

    #[test]
    fn namespace_has_no_trailing_underscore() {
        assert!(!namespace().ends_with('_'));
    }

    #[test]
    fn metric_name_starts_with_namespace_underscore() {
        assert!(metric_name("x").starts_with(&format!("{}_", namespace())));
    }

    #[test]
    fn namespace_is_lowercase() {
        assert_eq!(namespace(), namespace().to_lowercase());
    }
}

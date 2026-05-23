//! Prometheus metric name constants for expresso-tenant-migrate.

pub const METRIC_USERS_MIGRATED:  &str = "expresso_tenant_migrate_users_migrated_total";
pub const METRIC_USERS_SKIPPED:   &str = "expresso_tenant_migrate_users_skipped_total";
pub const METRIC_USERS_FAILED:    &str = "expresso_tenant_migrate_users_failed_total";
pub const METRIC_REALMS_VISITED:  &str = "expresso_tenant_migrate_realms_visited_total";
pub const METRIC_MIGRATE_DURATION_MS: &str = "expresso_tenant_migrate_duration_ms";

pub const LABEL_REALM:  &str = "realm";
pub const LABEL_REASON: &str = "reason";

pub const REASON_CONFLICT:     &str = "conflict";
pub const REASON_AUTH_FAILURE: &str = "auth_failure";
pub const REASON_API_ERROR:    &str = "api_error";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn users_migrated_has_expresso_prefix() {
        assert!(METRIC_USERS_MIGRATED.starts_with("expresso_"));
    }

    #[test]
    fn users_skipped_has_expresso_prefix() {
        assert!(METRIC_USERS_SKIPPED.starts_with("expresso_"));
    }

    #[test]
    fn users_failed_has_expresso_prefix() {
        assert!(METRIC_USERS_FAILED.starts_with("expresso_"));
    }

    #[test]
    fn realms_visited_has_expresso_prefix() {
        assert!(METRIC_REALMS_VISITED.starts_with("expresso_"));
    }

    #[test]
    fn duration_metric_has_expresso_prefix() {
        assert!(METRIC_MIGRATE_DURATION_MS.starts_with("expresso_"));
    }

    #[test]
    fn counter_metrics_end_with_total() {
        assert!(METRIC_USERS_MIGRATED.ends_with("_total"));
        assert!(METRIC_USERS_SKIPPED.ends_with("_total"));
        assert!(METRIC_USERS_FAILED.ends_with("_total"));
        assert!(METRIC_REALMS_VISITED.ends_with("_total"));
    }

    #[test]
    fn label_realm_value() {
        assert_eq!(LABEL_REALM, "realm");
    }

    #[test]
    fn label_reason_value() {
        assert_eq!(LABEL_REASON, "reason");
    }

    #[test]
    fn reason_conflict_value() {
        assert_eq!(REASON_CONFLICT, "conflict");
    }

    #[test]
    fn reason_auth_failure_value() {
        assert_eq!(REASON_AUTH_FAILURE, "auth_failure");
    }

    #[test]
    fn reason_api_error_value() {
        assert_eq!(REASON_API_ERROR, "api_error");
    }

    #[test]
    fn all_reasons_are_distinct() {
        let reasons = [REASON_CONFLICT, REASON_AUTH_FAILURE, REASON_API_ERROR];
        let mut sorted = reasons.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn all_metric_names_nonempty() {
        for m in [
            METRIC_USERS_MIGRATED, METRIC_USERS_SKIPPED, METRIC_USERS_FAILED,
            METRIC_REALMS_VISITED, METRIC_MIGRATE_DURATION_MS,
        ] {
            assert!(!m.is_empty());
        }
    }

    #[test]
    fn all_metrics_lowercase_snake_case() {
        for m in [
            METRIC_USERS_MIGRATED, METRIC_USERS_SKIPPED, METRIC_USERS_FAILED,
            METRIC_REALMS_VISITED, METRIC_MIGRATE_DURATION_MS,
        ] {
            assert!(!m.contains('-'), "metric contains hyphen: {m}");
            assert_eq!(m, m.to_ascii_lowercase(), "metric not lowercase: {m}");
        }
    }

    #[test]
    fn users_migrated_contains_migrate() {
        assert!(METRIC_USERS_MIGRATED.contains("migrate"));
    }

    #[test]
    fn duration_metric_contains_duration() {
        assert!(METRIC_MIGRATE_DURATION_MS.contains("duration"));
    }

    #[test]
    fn metrics_are_all_distinct() {
        let names = [
            METRIC_USERS_MIGRATED, METRIC_USERS_SKIPPED, METRIC_USERS_FAILED,
            METRIC_REALMS_VISITED, METRIC_MIGRATE_DURATION_MS,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 5);
    }

    #[test]
    fn reasons_are_lowercase_snake_case() {
        for r in [REASON_CONFLICT, REASON_AUTH_FAILURE, REASON_API_ERROR] {
            assert!(!r.contains('-'), "reason contains hyphen: {r}");
            assert_eq!(r, r.to_ascii_lowercase(), "reason not lowercase: {r}");
        }
    }

    #[test]
    fn labels_are_distinct() {
        assert_ne!(LABEL_REALM, LABEL_REASON);
    }

    #[test]
    fn users_failed_contains_failed() {
        assert!(METRIC_USERS_FAILED.contains("failed"));
    }

    #[test]
    fn duration_metric_does_not_end_with_total() {
        assert!(!METRIC_MIGRATE_DURATION_MS.ends_with("_total"));
    }

    #[test]
    fn users_skipped_contains_skipped() {
        assert!(METRIC_USERS_SKIPPED.contains("skipped"));
    }

    #[test]
    fn realms_visited_contains_realms() {
        assert!(METRIC_REALMS_VISITED.contains("realms"));
    }

    #[test]
    fn users_migrated_contains_migrated() {
        assert!(METRIC_USERS_MIGRATED.contains("migrated"));
    }
}

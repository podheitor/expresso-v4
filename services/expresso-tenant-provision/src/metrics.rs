//! Prometheus metric name constants for expresso-tenant-provision.

pub const METRIC_REALMS_CREATED:   &str = "expresso_tenant_provision_realms_created_total";
pub const METRIC_CLIENTS_CREATED:  &str = "expresso_tenant_provision_clients_created_total";
pub const METRIC_ROLES_CREATED:    &str = "expresso_tenant_provision_roles_created_total";
pub const METRIC_USERS_CREATED:    &str = "expresso_tenant_provision_users_created_total";
pub const METRIC_PROVISION_ERRORS: &str = "expresso_tenant_provision_errors_total";
pub const METRIC_PROVISION_DURATION_MS: &str = "expresso_tenant_provision_duration_ms";

pub const LABEL_REALM:  &str = "realm";
pub const LABEL_STEP:   &str = "step";

pub const STEP_REALM:  &str = "realm";
pub const STEP_CLIENT: &str = "client";
pub const STEP_ROLE:   &str = "role";
pub const STEP_USER:   &str = "user";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn realms_created_has_expresso_prefix() {
        assert!(METRIC_REALMS_CREATED.starts_with("expresso_"));
    }

    #[test]
    fn clients_created_has_expresso_prefix() {
        assert!(METRIC_CLIENTS_CREATED.starts_with("expresso_"));
    }

    #[test]
    fn roles_created_has_expresso_prefix() {
        assert!(METRIC_ROLES_CREATED.starts_with("expresso_"));
    }

    #[test]
    fn users_created_has_expresso_prefix() {
        assert!(METRIC_USERS_CREATED.starts_with("expresso_"));
    }

    #[test]
    fn errors_metric_has_expresso_prefix() {
        assert!(METRIC_PROVISION_ERRORS.starts_with("expresso_"));
    }

    #[test]
    fn duration_metric_has_expresso_prefix() {
        assert!(METRIC_PROVISION_DURATION_MS.starts_with("expresso_"));
    }

    #[test]
    fn counter_metrics_end_with_total() {
        assert!(METRIC_REALMS_CREATED.ends_with("_total"));
        assert!(METRIC_CLIENTS_CREATED.ends_with("_total"));
        assert!(METRIC_ROLES_CREATED.ends_with("_total"));
        assert!(METRIC_USERS_CREATED.ends_with("_total"));
        assert!(METRIC_PROVISION_ERRORS.ends_with("_total"));
    }

    #[test]
    fn label_realm_value() {
        assert_eq!(LABEL_REALM, "realm");
    }

    #[test]
    fn label_step_value() {
        assert_eq!(LABEL_STEP, "step");
    }

    #[test]
    fn step_realm_value() {
        assert_eq!(STEP_REALM, "realm");
    }

    #[test]
    fn step_client_value() {
        assert_eq!(STEP_CLIENT, "client");
    }

    #[test]
    fn step_role_value() {
        assert_eq!(STEP_ROLE, "role");
    }

    #[test]
    fn step_user_value() {
        assert_eq!(STEP_USER, "user");
    }

    #[test]
    fn all_steps_are_distinct() {
        let steps = [STEP_REALM, STEP_CLIENT, STEP_ROLE, STEP_USER];
        let mut sorted = steps.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4);
    }

    #[test]
    fn all_metric_names_nonempty() {
        for m in [
            METRIC_REALMS_CREATED, METRIC_CLIENTS_CREATED, METRIC_ROLES_CREATED,
            METRIC_USERS_CREATED, METRIC_PROVISION_ERRORS, METRIC_PROVISION_DURATION_MS,
        ] {
            assert!(!m.is_empty());
        }
    }

    #[test]
    fn all_metrics_lowercase_snake_case() {
        for m in [
            METRIC_REALMS_CREATED, METRIC_CLIENTS_CREATED, METRIC_ROLES_CREATED,
            METRIC_USERS_CREATED, METRIC_PROVISION_ERRORS, METRIC_PROVISION_DURATION_MS,
        ] {
            assert!(!m.contains('-'), "metric contains hyphen: {m}");
            assert_eq!(m, m.to_ascii_lowercase(), "metric not lowercase: {m}");
        }
    }

    #[test]
    fn provision_metrics_contain_provision() {
        for m in [
            METRIC_REALMS_CREATED, METRIC_CLIENTS_CREATED, METRIC_ROLES_CREATED,
            METRIC_USERS_CREATED, METRIC_PROVISION_ERRORS, METRIC_PROVISION_DURATION_MS,
        ] {
            assert!(m.contains("provision"), "metric missing 'provision': {m}");
        }
    }

    #[test]
    fn duration_metric_contains_duration() {
        assert!(METRIC_PROVISION_DURATION_MS.contains("duration"));
    }

    #[test]
    fn metrics_are_all_distinct() {
        let names = [
            METRIC_REALMS_CREATED, METRIC_CLIENTS_CREATED, METRIC_ROLES_CREATED,
            METRIC_USERS_CREATED, METRIC_PROVISION_ERRORS, METRIC_PROVISION_DURATION_MS,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 6);
    }

    #[test]
    fn labels_are_distinct() {
        assert_ne!(LABEL_REALM, LABEL_STEP);
    }

    #[test]
    fn steps_are_lowercase() {
        for s in [STEP_REALM, STEP_CLIENT, STEP_ROLE, STEP_USER] {
            assert_eq!(s, s.to_ascii_lowercase());
        }
    }

    #[test]
    fn duration_metric_does_not_end_with_total() {
        assert!(!METRIC_PROVISION_DURATION_MS.ends_with("_total"));
    }

    #[test]
    fn errors_metric_contains_errors() {
        assert!(METRIC_PROVISION_ERRORS.contains("errors"));
    }

    #[test]
    fn realms_created_contains_realms() {
        assert!(METRIC_REALMS_CREATED.contains("realms"));
    }

    #[test]
    fn step_client_and_step_user_differ() {
        assert_ne!(STEP_CLIENT, STEP_USER);
    }
}

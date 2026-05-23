//! Error taxonomy for expresso-tenant-provision.

use std::fmt;

#[derive(Debug)]
pub enum ProvisionError {
    Auth(String),
    RealmAlreadyExists(String),
    ClientCreateFailed { client_id: String, reason: String },
    RoleCreateFailed   { role: String, reason: String },
    UserCreateFailed   { username: String, reason: String },
    ApiFailure { status: u16, body: String },
    Validation(String),
    Internal(String),
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProvisionError::Auth(m) =>
                write!(f, "auth error: {m}"),
            ProvisionError::RealmAlreadyExists(r) =>
                write!(f, "realm already exists: {r}"),
            ProvisionError::ClientCreateFailed { client_id, reason } =>
                write!(f, "client create failed [{client_id}]: {reason}"),
            ProvisionError::RoleCreateFailed { role, reason } =>
                write!(f, "role create failed [{role}]: {reason}"),
            ProvisionError::UserCreateFailed { username, reason } =>
                write!(f, "user create failed [{username}]: {reason}"),
            ProvisionError::ApiFailure { status, body } =>
                write!(f, "api failure {status}: {body}"),
            ProvisionError::Validation(m) =>
                write!(f, "validation error: {m}"),
            ProvisionError::Internal(m) =>
                write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for ProvisionError {}

pub fn is_idempotent_safe(e: &ProvisionError) -> bool {
    matches!(e, ProvisionError::RealmAlreadyExists(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_display_contains_message() {
        let e = ProvisionError::Auth("bad credentials".into());
        assert!(e.to_string().contains("bad credentials"));
    }

    #[test]
    fn realm_already_exists_display_contains_realm() {
        let e = ProvisionError::RealmAlreadyExists("acme".into());
        assert!(e.to_string().contains("acme"));
    }

    #[test]
    fn client_create_failed_display_contains_client_id() {
        let e = ProvisionError::ClientCreateFailed {
            client_id: "expresso-web".into(), reason: "conflict".into()
        };
        assert!(e.to_string().contains("expresso-web"));
    }

    #[test]
    fn role_create_failed_display_contains_role() {
        let e = ProvisionError::RoleCreateFailed {
            role: "admin".into(), reason: "duplicate".into()
        };
        assert!(e.to_string().contains("admin"));
    }

    #[test]
    fn user_create_failed_display_contains_username() {
        let e = ProvisionError::UserCreateFailed {
            username: "alice".into(), reason: "email taken".into()
        };
        assert!(e.to_string().contains("alice"));
    }

    #[test]
    fn api_failure_display_contains_status() {
        let e = ProvisionError::ApiFailure { status: 502, body: "gateway error".into() };
        assert!(e.to_string().contains("502"));
    }

    #[test]
    fn validation_display_contains_message() {
        let e = ProvisionError::Validation("realm name empty".into());
        assert!(e.to_string().contains("realm name empty"));
    }

    #[test]
    fn internal_display_contains_message() {
        let e = ProvisionError::Internal("unexpected nil".into());
        assert!(e.to_string().contains("unexpected nil"));
    }

    #[test]
    fn auth_display_starts_with_auth_error() {
        let e = ProvisionError::Auth("x".into());
        assert!(e.to_string().starts_with("auth error:"));
    }

    #[test]
    fn realm_already_exists_display_starts_with_realm() {
        let e = ProvisionError::RealmAlreadyExists("r".into());
        assert!(e.to_string().starts_with("realm already exists:"));
    }

    #[test]
    fn validation_display_starts_with_validation() {
        let e = ProvisionError::Validation("v".into());
        assert!(e.to_string().starts_with("validation error:"));
    }

    #[test]
    fn internal_display_starts_with_internal() {
        let e = ProvisionError::Internal("i".into());
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn is_idempotent_safe_true_for_realm_already_exists() {
        let e = ProvisionError::RealmAlreadyExists("r".into());
        assert!(is_idempotent_safe(&e));
    }

    #[test]
    fn is_idempotent_safe_false_for_auth_error() {
        let e = ProvisionError::Auth("x".into());
        assert!(!is_idempotent_safe(&e));
    }

    #[test]
    fn is_idempotent_safe_false_for_validation() {
        let e = ProvisionError::Validation("x".into());
        assert!(!is_idempotent_safe(&e));
    }

    #[test]
    fn is_idempotent_safe_false_for_internal() {
        let e = ProvisionError::Internal("x".into());
        assert!(!is_idempotent_safe(&e));
    }

    #[test]
    fn is_idempotent_safe_false_for_api_failure() {
        let e = ProvisionError::ApiFailure { status: 500, body: "b".into() };
        assert!(!is_idempotent_safe(&e));
    }

    #[test]
    fn client_create_failed_display_contains_reason() {
        let e = ProvisionError::ClientCreateFailed {
            client_id: "c".into(), reason: "403 forbidden".into()
        };
        assert!(e.to_string().contains("403 forbidden"));
    }

    #[test]
    fn role_create_failed_display_contains_reason() {
        let e = ProvisionError::RoleCreateFailed {
            role: "r".into(), reason: "already exists".into()
        };
        assert!(e.to_string().contains("already exists"));
    }

    #[test]
    fn user_create_failed_display_contains_reason() {
        let e = ProvisionError::UserCreateFailed {
            username: "u".into(), reason: "email conflict".into()
        };
        assert!(e.to_string().contains("email conflict"));
    }

    #[test]
    fn all_display_nonempty() {
        let variants: Vec<Box<dyn std::error::Error>> = vec![
            Box::new(ProvisionError::Auth("a".into())),
            Box::new(ProvisionError::RealmAlreadyExists("r".into())),
            Box::new(ProvisionError::Validation("v".into())),
            Box::new(ProvisionError::Internal("i".into())),
        ];
        for e in variants {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn api_failure_display_contains_body() {
        let e = ProvisionError::ApiFailure { status: 503, body: "service unavailable".into() };
        assert!(e.to_string().contains("service unavailable"));
    }

    #[test]
    fn is_idempotent_safe_false_for_client_create_failed() {
        let e = ProvisionError::ClientCreateFailed { client_id: "c".into(), reason: "r".into() };
        assert!(!is_idempotent_safe(&e));
    }

    #[test]
    fn is_idempotent_safe_false_for_user_create_failed() {
        let e = ProvisionError::UserCreateFailed { username: "u".into(), reason: "r".into() };
        assert!(!is_idempotent_safe(&e));
    }

    #[test]
    fn is_idempotent_safe_false_for_role_create_failed() {
        let e = ProvisionError::RoleCreateFailed { role: "admin".into(), reason: "dup".into() };
        assert!(!is_idempotent_safe(&e));
    }

    #[test]
    fn realm_already_exists_display_contains_already_exists() {
        let e = ProvisionError::RealmAlreadyExists("r".into());
        assert!(e.to_string().contains("already exists"));
    }
}

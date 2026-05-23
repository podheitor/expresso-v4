//! Error taxonomy for expresso-tenant-migrate.

use std::fmt;

#[derive(Debug)]
pub enum MigrateError {
    Auth(String),
    RealmNotFound(String),
    UserConflict(String),
    ApiFailure { status: u16, body: String },
    Validation(String),
    Internal(String),
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrateError::Auth(m)           => write!(f, "auth error: {m}"),
            MigrateError::RealmNotFound(r)  => write!(f, "realm not found: {r}"),
            MigrateError::UserConflict(u)   => write!(f, "user conflict: {u}"),
            MigrateError::ApiFailure { status, body } =>
                write!(f, "api failure {status}: {body}"),
            MigrateError::Validation(m)     => write!(f, "validation error: {m}"),
            MigrateError::Internal(m)       => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for MigrateError {}

pub fn is_retryable(e: &MigrateError) -> bool {
    matches!(e, MigrateError::ApiFailure { status, .. } if *status >= 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_display_contains_message() {
        let e = MigrateError::Auth("bad token".into());
        assert!(e.to_string().contains("bad token"));
    }

    #[test]
    fn realm_not_found_display_contains_realm() {
        let e = MigrateError::RealmNotFound("acme-corp".into());
        assert!(e.to_string().contains("acme-corp"));
    }

    #[test]
    fn user_conflict_display_contains_username() {
        let e = MigrateError::UserConflict("alice".into());
        assert!(e.to_string().contains("alice"));
    }

    #[test]
    fn api_failure_display_contains_status_and_body() {
        let e = MigrateError::ApiFailure { status: 503, body: "overloaded".into() };
        let s = e.to_string();
        assert!(s.contains("503"));
        assert!(s.contains("overloaded"));
    }

    #[test]
    fn validation_display_contains_message() {
        let e = MigrateError::Validation("missing realm name".into());
        assert!(e.to_string().contains("missing realm name"));
    }

    #[test]
    fn internal_display_contains_message() {
        let e = MigrateError::Internal("unexpected state".into());
        assert!(e.to_string().contains("unexpected state"));
    }

    #[test]
    fn auth_display_starts_with_auth_error() {
        let e = MigrateError::Auth("x".into());
        assert!(e.to_string().starts_with("auth error:"));
    }

    #[test]
    fn realm_not_found_display_starts_with_realm() {
        let e = MigrateError::RealmNotFound("r".into());
        assert!(e.to_string().starts_with("realm not found:"));
    }

    #[test]
    fn user_conflict_display_starts_with_user() {
        let e = MigrateError::UserConflict("u".into());
        assert!(e.to_string().starts_with("user conflict:"));
    }

    #[test]
    fn validation_display_starts_with_validation() {
        let e = MigrateError::Validation("v".into());
        assert!(e.to_string().starts_with("validation error:"));
    }

    #[test]
    fn internal_display_starts_with_internal() {
        let e = MigrateError::Internal("i".into());
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn api_failure_display_starts_with_api_failure() {
        let e = MigrateError::ApiFailure { status: 500, body: "b".into() };
        assert!(e.to_string().starts_with("api failure"));
    }

    #[test]
    fn is_retryable_true_for_5xx() {
        let e = MigrateError::ApiFailure { status: 503, body: "".into() };
        assert!(is_retryable(&e));
    }

    #[test]
    fn is_retryable_true_for_500() {
        let e = MigrateError::ApiFailure { status: 500, body: "".into() };
        assert!(is_retryable(&e));
    }

    #[test]
    fn is_retryable_false_for_4xx() {
        let e = MigrateError::ApiFailure { status: 404, body: "".into() };
        assert!(!is_retryable(&e));
    }

    #[test]
    fn is_retryable_false_for_auth_error() {
        let e = MigrateError::Auth("x".into());
        assert!(!is_retryable(&e));
    }

    #[test]
    fn is_retryable_false_for_validation_error() {
        let e = MigrateError::Validation("x".into());
        assert!(!is_retryable(&e));
    }

    #[test]
    fn is_retryable_false_for_realm_not_found() {
        let e = MigrateError::RealmNotFound("r".into());
        assert!(!is_retryable(&e));
    }

    #[test]
    fn is_retryable_false_for_user_conflict() {
        let e = MigrateError::UserConflict("u".into());
        assert!(!is_retryable(&e));
    }

    #[test]
    fn is_retryable_false_for_internal() {
        let e = MigrateError::Internal("x".into());
        assert!(!is_retryable(&e));
    }

    #[test]
    fn api_failure_status_499_is_not_retryable() {
        let e = MigrateError::ApiFailure { status: 499, body: "".into() };
        assert!(!is_retryable(&e));
    }

    #[test]
    fn display_is_nonempty_for_all_variants() {
        let variants: Vec<MigrateError> = vec![
            MigrateError::Auth("a".into()),
            MigrateError::RealmNotFound("r".into()),
            MigrateError::UserConflict("u".into()),
            MigrateError::ApiFailure { status: 500, body: "b".into() },
            MigrateError::Validation("v".into()),
            MigrateError::Internal("i".into()),
        ];
        for e in variants {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn is_retryable_true_for_503() {
        let e = MigrateError::ApiFailure { status: 503, body: "overload".into() };
        assert!(is_retryable(&e));
    }

    #[test]
    fn user_conflict_display_starts_with_user_conflict() {
        let e = MigrateError::UserConflict("u".into());
        assert!(e.to_string().starts_with("user conflict:"));
    }

    #[test]
    fn is_retryable_true_for_599() {
        let e = MigrateError::ApiFailure { status: 599, body: "timeout".into() };
        assert!(is_retryable(&e));
    }
}

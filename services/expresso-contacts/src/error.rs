//! Contacts service error types

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub type Result<T, E = ContactsError> = std::result::Result<T, E>;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum ContactsError {
    #[error("core: {0}")]
    Core(#[from] expresso_core::CoreError),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("database unavailable")]
    DatabaseUnavailable,

    #[error("contact not found: {0}")]
    ContactNotFound(Uuid),

    #[error("addressbook not found: {0}")]
    AddressbookNotFound(String),

    #[error("invalid vCard data: {0}")]
    InvalidVCard(String),

    #[error("forbidden")]
    Forbidden,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("not supported: {0}")]
    NotSupported(&'static str),
}

impl IntoResponse for ContactsError {
    fn into_response(self) -> Response {
        let (status, code, msg) = match &self {
            Self::ContactNotFound(_) => {
                (StatusCode::NOT_FOUND, "contact_not_found", self.to_string())
            }
            Self::AddressbookNotFound(_) => (
                StatusCode::NOT_FOUND,
                "addressbook_not_found",
                self.to_string(),
            ),
            Self::InvalidVCard(_) => (StatusCode::BAD_REQUEST, "invalid_vcard", self.to_string()),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request", self.to_string()),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden", self.to_string()),
            Self::DatabaseUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "db_unavailable",
                self.to_string(),
            ),
            Self::NotSupported(_) => (
                StatusCode::NOT_IMPLEMENTED,
                "not_supported",
                self.to_string(),
            ),
            Self::Database(sqlx::Error::Database(e)) if e.is_unique_violation() => (
                StatusCode::CONFLICT,
                "unique_violation",
                "recurso duplicado".into(),
            ),
            Self::Database(sqlx::Error::RowNotFound) => (
                StatusCode::NOT_FOUND,
                "not_found",
                "recurso não encontrado".into(),
            ),
            Self::Database(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "database",
                "erro interno".into(),
            ),
            Self::Core(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "erro interno".into(),
            ),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    fn status(e: ContactsError) -> u16 {
        e.into_response().status().as_u16()
    }

    #[test]
    fn contact_not_found_is_404() {
        assert_eq!(status(ContactsError::ContactNotFound(Uuid::new_v4())), 404);
    }

    #[test]
    fn addressbook_not_found_is_404() {
        assert_eq!(
            status(ContactsError::AddressbookNotFound("ab1".into())),
            404
        );
    }

    #[test]
    fn invalid_vcard_is_400() {
        assert_eq!(status(ContactsError::InvalidVCard("parse err".into())), 400);
    }

    #[test]
    fn bad_request_is_400() {
        assert_eq!(status(ContactsError::BadRequest("bad".into())), 400);
    }

    #[test]
    fn forbidden_is_403() {
        assert_eq!(status(ContactsError::Forbidden), 403);
    }

    #[test]
    fn database_unavailable_is_503() {
        assert_eq!(status(ContactsError::DatabaseUnavailable), 503);
    }

    #[test]
    fn not_supported_is_501() {
        assert_eq!(status(ContactsError::NotSupported("REPORT")), 501);
    }

    #[test]
    fn bad_request_message_in_display() {
        let e = ContactsError::BadRequest("invalid uid".into());
        assert!(format!("{e}").contains("invalid uid"));
    }

    #[test]
    fn contact_not_found_message_contains_id() {
        let id = Uuid::new_v4();
        let e = ContactsError::ContactNotFound(id);
        assert!(format!("{e}").contains(&id.to_string()));
    }

    #[test]
    fn database_unavailable_display_not_empty() {
        assert!(!format!("{}", ContactsError::DatabaseUnavailable).is_empty());
    }

    #[test]
    fn addressbook_not_found_display_contains_id() {
        let id = "00000000-0000-0000-0000-000000000000";
        let e = ContactsError::AddressbookNotFound(id.into());
        assert!(format!("{e}").contains(id));
    }

    #[test]
    fn invalid_vcard_display_contains_message() {
        let e = ContactsError::InvalidVCard("missing UID".into());
        assert!(format!("{e}").contains("missing UID"));
    }

    #[test]
    fn forbidden_display_is_forbidden() {
        let e = ContactsError::Forbidden;
        assert_eq!(format!("{e}"), "forbidden");
    }

    #[test]
    fn invalid_vcard_status_is_400() {
        assert_eq!(status(ContactsError::InvalidVCard("bad input".into())), 400);
    }

    #[test]
    fn not_supported_display_contains_operation() {
        let e = ContactsError::NotSupported("MKCOL");
        assert!(format!("{e}").contains("MKCOL"));
    }

    #[test]
    fn bad_request_is_400_variant() {
        let e = ContactsError::BadRequest("missing field".into());
        assert_eq!(status(e), 400);
    }

    #[test]
    fn forbidden_display_string_is_forbidden() {
        let e = ContactsError::Forbidden;
        assert_eq!(format!("{e}"), "forbidden");
    }

    #[test]
    fn not_supported_status_is_501() {
        let e = ContactsError::NotSupported("LOCK");
        assert_eq!(status(e), 501);
    }

    #[test]
    fn database_unavailable_is_503_variant() {
        assert_eq!(status(ContactsError::DatabaseUnavailable), 503);
    }

    #[test]
    fn bad_request_display_contains_message() {
        let e = ContactsError::BadRequest("missing field".into());
        assert!(e.to_string().contains("missing field"));
    }

    #[test]
    fn invalid_vcard_display_contains_bad_input() {
        let e = ContactsError::InvalidVCard("bad input".into());
        assert!(e.to_string().contains("bad input"));
    }

    #[test]
    fn not_supported_display_is_nonempty() {
        let e = ContactsError::NotSupported("LOCK");
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn forbidden_status_is_403() {
        assert_eq!(status(ContactsError::Forbidden), 403);
    }

    #[test]
    fn contact_not_found_status_is_404() {
        assert_eq!(
            status(ContactsError::ContactNotFound(uuid::Uuid::nil())),
            404
        );
    }

    #[test]
    fn addressbook_not_found_status_is_404() {
        assert_eq!(status(ContactsError::AddressbookNotFound("ab".into())), 404);
    }
}

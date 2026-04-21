//! Contacts service error types

use axum::{response::{IntoResponse, Response}, http::StatusCode, Json};
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
            Self::ContactNotFound(_)      => (StatusCode::NOT_FOUND,           "contact_not_found",      self.to_string()),
            Self::AddressbookNotFound(_)  => (StatusCode::NOT_FOUND,           "addressbook_not_found",  self.to_string()),
            Self::InvalidVCard(_)         => (StatusCode::BAD_REQUEST,         "invalid_vcard",          self.to_string()),
            Self::BadRequest(_)           => (StatusCode::BAD_REQUEST,         "bad_request",            self.to_string()),
            Self::Forbidden               => (StatusCode::FORBIDDEN,           "forbidden",              self.to_string()),
            Self::DatabaseUnavailable     => (StatusCode::SERVICE_UNAVAILABLE, "db_unavailable",         self.to_string()),
            Self::NotSupported(_)         => (StatusCode::NOT_IMPLEMENTED,     "not_supported",          self.to_string()),
            Self::Database(sqlx::Error::Database(e)) if e.is_unique_violation()
                                          => (StatusCode::CONFLICT,            "unique_violation",       "recurso duplicado".into()),
            Self::Database(sqlx::Error::RowNotFound)
                                          => (StatusCode::NOT_FOUND,           "not_found",              "recurso não encontrado".into()),
            Self::Database(_)             => (StatusCode::INTERNAL_SERVER_ERROR, "database",             "erro interno".into()),
            Self::Core(_)                 => (StatusCode::INTERNAL_SERVER_ERROR, "internal",             "erro interno".into()),
        };
        (status, Json(json!({"error": code, "message": msg}))).into_response()
    }
}

//! Tenant-scoped user lookup for share flows.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{ContactsError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/users", get(lookup))
}

#[derive(Debug, Deserialize)]
pub struct UserQuery { pub email: Option<String> }

#[derive(Debug, Serialize)]
pub struct UserOut { pub id: Uuid, pub email: String }

async fn lookup(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<UserQuery>,
) -> Result<Json<UserOut>> {
    let email = q.email.ok_or_else(|| ContactsError::BadRequest("email required".into()))?;
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() {
        return Err(ContactsError::BadRequest("email empty".into()));
    }
    let pool = state.db_or_unavailable()?;
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, email FROM users WHERE tenant_id = $1 AND lower(email) = $2 LIMIT 1",
    )
    .bind(ctx.tenant_id)
    .bind(&email)
    .fetch_optional(pool)
    .await?;

    match row {
        Some((id, email)) => Ok(Json(UserOut { id, email })),
        None => Err(ContactsError::BadRequest(format!("user not found: {email}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_query_email_optional() {
        let q: UserQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_email_set() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"a@ex.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("a@ex.com"));
    }

    #[test]
    fn user_query_email_null_is_none() {
        let q: UserQuery = serde_json::from_str(r#"{"email":null}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_email_with_plus_tag() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"user+tag@ex.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("user+tag@ex.com"));
    }

    #[test]
    fn user_query_absent_email_is_none() {
        let q: UserQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_empty_string_is_some() {
        let q: UserQuery = serde_json::from_str(r#"{"email":""}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some(""));
    }

    #[test]
    fn user_query_with_subdomain_email() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"user@sub.example.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("user@sub.example.com"));
    }

    #[test]
    fn user_query_null_email_is_none() {
        let q: UserQuery = serde_json::from_str(r#"{"email":null}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_email_unicode_preserved() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"hélio@example.com"}"#).unwrap();
        assert!(q.email.as_deref().unwrap().contains("hélio"));
    }

    #[test]
    fn user_query_empty_object_gives_none_email() {
        let q: UserQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_email_with_subdomain_preserved() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"alice@sub.example.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("alice@sub.example.com"));
    }

    #[test]
    fn user_query_email_with_plus_sign_preserved() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"alice+tag@example.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("alice+tag@example.com"));
    }

    #[test]
    fn user_query_email_none_when_not_provided() {
        let q: UserQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.email.is_none());
    }
}

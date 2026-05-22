//! Minimal tenant-scoped user lookup for share flows.
//! GET /api/v1/users?email=X → {id, email} or 404 when not in tenant.

use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{CalendarError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/users", get(lookup))
}

#[derive(Debug, Deserialize)]
pub struct UserQuery {
    pub email: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserOut {
    pub id:    Uuid,
    pub email: String,
}

async fn lookup(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<UserQuery>,
) -> Result<Json<UserOut>> {
    let email = q.email.ok_or_else(|| CalendarError::BadRequest("email required".into()))?;
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() {
        return Err(CalendarError::BadRequest("email empty".into()));
    }
    let pool = state.db_or_unavailable()?;
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, email FROM users WHERE tenant_id = $1 AND lower(email) = $2 LIMIT 1",
    )
    .bind(ctx.tenant_id)
    .bind(&email)
    .fetch_optional(pool)
    .await
    .map_err(CalendarError::from)?;

    match row {
        Some((id, email)) => Ok(Json(UserOut { id, email })),
        None => Err(CalendarError::BadRequest(format!("user not found: {email}"))),
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
        let q: UserQuery = serde_json::from_str(r#"{"email":"admin@ex.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("admin@ex.com"));
    }

    #[test]
    fn user_query_email_null_is_none() {
        let q: UserQuery = serde_json::from_str(r#"{"email":null}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_email_unicode() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"héitor@ex.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("héitor@ex.com"));
    }

    #[test]
    fn user_query_extra_field_ignored() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"a@x.com","extra":"ignored"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("a@x.com"));
    }

    #[test]
    fn user_query_empty_string_email_is_some() {
        let q: UserQuery = serde_json::from_str(r#"{"email":""}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some(""));
    }

    #[test]
    fn user_query_absent_email_is_none() {
        let q: UserQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_unicode_email_stored() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"usuário@empresa.com"}"#).unwrap();
        assert!(q.email.as_deref().unwrap().contains("usuário"));
    }

    #[test]
    fn user_query_null_email_is_none() {
        let q: UserQuery = serde_json::from_str(r#"{"email":null}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_email_preserved() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"admin@corp.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("admin@corp.com"));
    }

    #[test]
    fn user_query_missing_field_is_none() {
        let q: UserQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.email.is_none());
    }

    #[test]
    fn user_query_email_with_subdomain_preserved() {
        let q: UserQuery = serde_json::from_str(r#"{"email":"user@mail.corp.example.com"}"#).unwrap();
        assert_eq!(q.email.as_deref(), Some("user@mail.corp.example.com"));
    }
}

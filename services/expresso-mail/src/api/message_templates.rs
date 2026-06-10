//! Message templates — named canned responses (subject + body) for compose.
//!
//! GET    /api/v1/mail/templates       — list all templates for the user
//! POST   /api/v1/mail/templates       — create a template
//! GET    /api/v1/mail/templates/:id   — fetch a single template
//! PUT    /api/v1/mail/templates/:id   — replace a template
//! DELETE /api/v1/mail/templates/:id   — remove a template
//!
//! Per-(tenant, user); no defaults or ordering beyond name. Mirrors
//! `flag_presets` / `signatures`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

/// Cap on the body length (64 KiB) — a template is a starting point, not an
/// archive; keeps the row bounded.
const MAX_BODY_BYTES: usize = 64 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/templates", get(list_templates).post(create_template))
        .route(
            "/mail/templates/:id",
            get(get_template)
                .put(update_template)
                .delete(delete_template),
        )
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MessageTemplate {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub subject: String,
    pub body: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TemplateBody {
    pub name: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
}

const SELECT_COLS: &str = "id, tenant_id, user_id, name, subject, body, created_at, updated_at";

async fn list_templates(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<MessageTemplate>>> {
    let pool = state.db();
    let rows: Vec<MessageTemplate> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM mail_message_templates \
         WHERE tenant_id = $1 AND user_id = $2 \
         ORDER BY name ASC"
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

async fn create_template(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<TemplateBody>,
) -> Result<(StatusCode, Json<MessageTemplate>)> {
    validate(&body)?;
    let pool = state.db();
    let row: MessageTemplate = sqlx::query_as(&format!(
        "INSERT INTO mail_message_templates (tenant_id, user_id, name, subject, body) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {SELECT_COLS}"
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(body.name.trim())
    .bind(&body.subject)
    .bind(&body.body)
    .fetch_one(pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn get_template(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageTemplate>> {
    let pool = state.db();
    let row: Option<MessageTemplate> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM mail_message_templates \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(pool)
    .await?;
    row.map(Json).ok_or(MailError::NotFound)
}

async fn update_template(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<TemplateBody>,
) -> Result<Json<MessageTemplate>> {
    validate(&body)?;
    let pool = state.db();
    let row: Option<MessageTemplate> = sqlx::query_as(&format!(
        "UPDATE mail_message_templates \
            SET name = $4, subject = $5, body = $6, updated_at = now() \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3 \
          RETURNING {SELECT_COLS}"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(body.name.trim())
    .bind(&body.subject)
    .bind(&body.body)
    .fetch_optional(pool)
    .await?;
    row.map(Json).ok_or(MailError::NotFound)
}

async fn delete_template(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db();
    let r = sqlx::query(
        "DELETE FROM mail_message_templates \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(MailError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn validate(body: &TemplateBody) -> Result<()> {
    if body.name.trim().is_empty() {
        return Err(MailError::BadRequest("name must not be empty".into()));
    }
    if body.name.len() > 100 {
        return Err(MailError::BadRequest(
            "name must be <= 100 characters".into(),
        ));
    }
    if body.subject.len() > 998 {
        return Err(MailError::BadRequest(
            "subject must be <= 998 characters".into(),
        ));
    }
    if body.body.len() > MAX_BODY_BYTES {
        return Err(MailError::BadRequest("body too large (max 64 KiB)".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(name: &str, body_len: usize) -> TemplateBody {
        TemplateBody {
            name: name.into(),
            subject: "Re: assunto".into(),
            body: "x".repeat(body_len),
        }
    }

    #[test]
    fn validate_accepts_normal_template() {
        assert!(validate(&body("Boas-vindas", 200)).is_ok());
    }

    #[test]
    fn validate_rejects_blank_name() {
        assert!(validate(&body("   ", 10)).is_err());
    }

    #[test]
    fn validate_rejects_oversized_body() {
        assert!(validate(&body("x", MAX_BODY_BYTES + 1)).is_err());
    }
}

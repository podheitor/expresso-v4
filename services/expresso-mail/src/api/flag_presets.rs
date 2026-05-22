//! Flag presets — named sets of IMAP flags for quick-apply actions.
//!
//! GET  /api/v1/mail/flag-presets          — list all presets for user
//! POST /api/v1/mail/flag-presets          — create a new preset
//! GET  /api/v1/mail/flag-presets/:id      — fetch single preset
//! PUT  /api/v1/mail/flag-presets/:id      — replace preset flags/name
//! DELETE /api/v1/mail/flag-presets/:id    — remove preset

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::types::Json as SqlxJson;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/flag-presets",     get(list_presets).post(create_preset))
        .route("/mail/flag-presets/:id", get(get_preset).put(update_preset).delete(delete_preset))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct FlagPreset {
    pub id:         Uuid,
    pub tenant_id:  Uuid,
    pub user_id:    Uuid,
    pub name:       String,
    pub flags:      SqlxJson<Vec<String>>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct PresetBody {
    pub name:  String,
    pub flags: Vec<String>,
}

async fn list_presets(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<FlagPreset>>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<FlagPreset> = sqlx::query_as(
        "SELECT id, tenant_id, user_id, name, flags, created_at, updated_at \
         FROM mail_flag_presets \
         WHERE tenant_id = $1 AND user_id = $2 \
         ORDER BY name ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

async fn create_preset(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<PresetBody>,
) -> Result<(StatusCode, Json<FlagPreset>)> {
    validate_preset(&body)?;
    let pool = state.db_or_unavailable()?;
    let row: FlagPreset = sqlx::query_as(
        "INSERT INTO mail_flag_presets (tenant_id, user_id, name, flags) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, tenant_id, user_id, name, flags, created_at, updated_at",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&body.name)
    .bind(SqlxJson(body.flags))
    .fetch_one(pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn get_preset(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<FlagPreset>> {
    let pool = state.db_or_unavailable()?;
    let row: Option<FlagPreset> = sqlx::query_as(
        "SELECT id, tenant_id, user_id, name, flags, created_at, updated_at \
         FROM mail_flag_presets \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(pool)
    .await?;
    row.map(Json).ok_or(MailError::NotFound)
}

async fn update_preset(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<PresetBody>,
) -> Result<Json<FlagPreset>> {
    validate_preset(&body)?;
    let pool = state.db_or_unavailable()?;
    let row: Option<FlagPreset> = sqlx::query_as(
        "UPDATE mail_flag_presets \
            SET name = $4, flags = $5, updated_at = now() \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3 \
          RETURNING id, tenant_id, user_id, name, flags, created_at, updated_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&body.name)
    .bind(SqlxJson(body.flags))
    .fetch_optional(pool)
    .await?;
    row.map(Json).ok_or(MailError::NotFound)
}

async fn delete_preset(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let r = sqlx::query(
        "DELETE FROM mail_flag_presets \
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

fn validate_preset(body: &PresetBody) -> Result<()> {
    if body.name.trim().is_empty() {
        return Err(MailError::BadRequest("name must not be empty".into()));
    }
    if body.name.len() > 100 {
        return Err(MailError::BadRequest("name must be <= 100 characters".into()));
    }
    if body.flags.len() > 50 {
        return Err(MailError::BadRequest("at most 50 flags per preset".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(name: &str, flag_count: usize) -> PresetBody {
        PresetBody {
            name:  name.into(),
            flags: vec!["\\Flagged".into(); flag_count],
        }
    }

    #[test]
    fn valid_preset_passes() {
        assert!(validate_preset(&body("Work", 3)).is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        assert!(validate_preset(&body("", 0)).is_err());
    }

    #[test]
    fn whitespace_only_name_rejected() {
        assert!(validate_preset(&body("   ", 0)).is_err());
    }

    #[test]
    fn name_exactly_100_chars_passes() {
        let name = "x".repeat(100);
        assert!(validate_preset(&body(&name, 1)).is_ok());
    }

    #[test]
    fn name_101_chars_rejected() {
        let name = "x".repeat(101);
        assert!(validate_preset(&body(&name, 1)).is_err());
    }

    #[test]
    fn exactly_50_flags_passes() {
        assert!(validate_preset(&body("Preset", 50)).is_ok());
    }

    #[test]
    fn fifty_one_flags_rejected() {
        assert!(validate_preset(&body("Preset", 51)).is_err());
    }

    #[test]
    fn zero_flags_passes() {
        assert!(validate_preset(&body("Empty", 0)).is_ok());
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(validate_preset(&body("   ", 0)).is_err());
    }
}

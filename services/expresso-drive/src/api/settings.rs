//! Tenant-scoped drive settings.
//!
//! GET/PUT /api/v1/drive/settings/trash-purge — read/write per-tenant trash
//! auto-purge schedule (days). Written to `tenants.config` JSONB under key
//! `trash_auto_purge_days`. Workers read this at each hourly tick to auto-purge.

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};

use crate::api::context::RequestCtx;
use crate::error::{DriveError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/drive/settings/trash-purge", get(get_trash_purge).put(put_trash_purge))
}

#[derive(Debug, Serialize)]
pub struct TrashPurgeSettings {
    /// Days after which trashed files are auto-purged. null = disabled.
    pub auto_purge_days: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TrashPurgePutBody {
    /// Days (1–3650). null or absent to disable auto-purge.
    pub auto_purge_days: Option<i64>,
}

/// GET /api/v1/drive/settings/trash-purge — read current auto-purge setting.
async fn get_trash_purge(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<TrashPurgeSettings>> {
    let pool = state.db_or_unavailable()?;
    let days: Option<i64> = sqlx::query_scalar(
        "SELECT (config->>'trash_auto_purge_days')::bigint \
         FROM tenants WHERE id = $1",
    )
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(Json(TrashPurgeSettings { auto_purge_days: days }))
}

/// PUT /api/v1/drive/settings/trash-purge — set (or clear) auto-purge.
/// Tenant-scoped: any authenticated user in the tenant can change it.
async fn put_trash_purge(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<TrashPurgePutBody>,
) -> Result<(StatusCode, Json<TrashPurgeSettings>)> {
    if let Some(d) = body.auto_purge_days {
        if !(1..=3650).contains(&d) {
            return Err(DriveError::BadRequest(
                "auto_purge_days must be between 1 and 3650".into(),
            ));
        }
    }
    let pool = state.db_or_unavailable()?;

    match body.auto_purge_days {
        Some(days) => {
            sqlx::query(
                "UPDATE tenants SET config = jsonb_set(config, '{trash_auto_purge_days}', $2::text::jsonb) \
                 WHERE id = $1",
            )
            .bind(ctx.tenant_id)
            .bind(days.to_string())
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "UPDATE tenants SET config = config - 'trash_auto_purge_days' WHERE id = $1",
            )
            .bind(ctx.tenant_id)
            .execute(pool)
            .await?;
        }
    }

    Ok((StatusCode::OK, Json(TrashPurgeSettings { auto_purge_days: body.auto_purge_days })))
}

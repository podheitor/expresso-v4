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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_purge_settings_with_days() {
        let s = TrashPurgeSettings { auto_purge_days: Some(30) };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("30"));
    }

    #[test]
    fn trash_purge_settings_disabled() {
        let s = TrashPurgeSettings { auto_purge_days: None };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(v["auto_purge_days"].is_null());
    }

    #[test]
    fn trash_purge_put_body_deserializes_some() {
        let json = r#"{"auto_purge_days":90}"#;
        let b: TrashPurgePutBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.auto_purge_days, Some(90));
    }

    #[test]
    fn trash_purge_put_body_deserializes_null() {
        let json = r#"{"auto_purge_days":null}"#;
        let b: TrashPurgePutBody = serde_json::from_str(json).unwrap();
        assert!(b.auto_purge_days.is_none());
    }

    #[test]
    fn trash_purge_put_body_deserializes_absent() {
        let json = r#"{}"#;
        let b: TrashPurgePutBody = serde_json::from_str(json).unwrap();
        assert!(b.auto_purge_days.is_none());
    }

    #[test]
    fn trash_purge_settings_serializes_some() {
        let s = TrashPurgeSettings { auto_purge_days: Some(7) };
        let v: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert_eq!(v["auto_purge_days"], 7);
    }

    #[test]
    fn trash_purge_settings_serializes_null() {
        let s = TrashPurgeSettings { auto_purge_days: None };
        let v: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert!(v["auto_purge_days"].is_null());
    }

    #[test]
    fn trash_purge_settings_roundtrip() {
        let s = TrashPurgeSettings { auto_purge_days: Some(30) };
        let back: TrashPurgeSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.auto_purge_days, Some(30));
    }

    #[test]
    fn trash_purge_settings_null_days_roundtrip() {
        let s = TrashPurgeSettings { auto_purge_days: None };
        let back: TrashPurgeSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.auto_purge_days.is_none());
    }

    #[test]
    fn trash_purge_put_body_some_days() {
        let b: TrashPurgePutBody = serde_json::from_str(r#"{"auto_purge_days":30}"#).unwrap();
        assert_eq!(b.auto_purge_days, Some(30));
    }

    #[test]
    fn trash_purge_settings_none_when_absent() {
        let s = TrashPurgeSettings { auto_purge_days: None };
        assert!(s.auto_purge_days.is_none());
    }

    #[test]
    fn trash_purge_settings_90_days_preserved() {
        let s = TrashPurgeSettings { auto_purge_days: Some(90) };
        assert_eq!(s.auto_purge_days, Some(90));
    }

    #[test]
    fn trash_purge_settings_none_auto_purge() {
        let s = TrashPurgeSettings { auto_purge_days: None };
        assert!(s.auto_purge_days.is_none());
    }

    #[test]
    fn trash_purge_settings_seven_days_preserved() {
        let s = TrashPurgeSettings { auto_purge_days: Some(7) };
        assert_eq!(s.auto_purge_days, Some(7));
    }

    #[test]
    fn trash_purge_put_body_none_days() {
        let b: TrashPurgePutBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.auto_purge_days.is_none());
    }

    #[test]
    fn trash_purge_settings_max_days_preserved() {
        let s = TrashPurgeSettings { auto_purge_days: Some(3650) };
        let v: serde_json::Value = serde_json::to_value(&s).unwrap();
        assert_eq!(v["auto_purge_days"], 3650);
    }
}

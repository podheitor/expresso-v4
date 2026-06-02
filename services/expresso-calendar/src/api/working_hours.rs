//! Per-user working (business) hours.
//!
//! Routes:
//!   GET /api/v1/working-hours  — the caller's weekly working hours
//!   PUT /api/v1/working-hours  — replace the caller's whole week set
//!
//! A working-hours set is a list of (weekday, start_minute, end_minute) windows
//! in the user's timezone (minutes-from-midnight). PUT is a full replace (delete
//! all then insert) inside one tenant tx, so the set always reflects exactly
//! what the client sent. Scheduler integration (prefer in-hours slots) is a
//! follow-up; this sprint is the store + API.

use axum::{extract::State, routing::get, Json, Router};
use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::api::context::RequestCtx;
use crate::error::{CalendarError, Result};
use crate::state::AppState;

const MINUTES_PER_DAY: i32 = 1440;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WorkingHour {
    pub weekday: i16,
    pub start_minute: i32,
    pub end_minute: i32,
}

#[derive(Debug, Deserialize)]
pub struct WorkingHoursBody {
    pub hours: Vec<WorkingHour>,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/working-hours", get(list).put(replace))
}

/// GET /api/v1/working-hours — the caller's windows, ordered by weekday.
async fn list(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<WorkingHour>>> {
    let pool = state.db_or_unavailable()?;
    let rows: Vec<WorkingHour> = sqlx::query_as(
        "SELECT weekday, start_minute, end_minute FROM user_working_hours \
         WHERE tenant_id = $1 AND user_id = $2 ORDER BY weekday, start_minute",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

/// PUT /api/v1/working-hours — replace the caller's whole week set atomically.
async fn replace(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<WorkingHoursBody>,
) -> Result<Json<Vec<WorkingHour>>> {
    validate(&body.hours)?;
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;
    sqlx::query("DELETE FROM user_working_hours WHERE tenant_id = $1 AND user_id = $2")
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .execute(&mut *tx)
        .await?;
    for h in &body.hours {
        sqlx::query(
            "INSERT INTO user_working_hours \
                 (tenant_id, user_id, weekday, start_minute, end_minute) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(ctx.tenant_id)
        .bind(ctx.user_id)
        .bind(h.weekday)
        .bind(h.start_minute)
        .bind(h.end_minute)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Json(body.hours))
}

/// Validate the windows: weekday 0..6, minutes in range, start < end, and no
/// duplicate weekday (the PK would reject it, but a clear 400 beats a 500).
fn validate(hours: &[WorkingHour]) -> Result<()> {
    let mut seen = [false; 7];
    for h in hours {
        if !(0..=6).contains(&h.weekday) {
            return Err(CalendarError::BadRequest(format!(
                "weekday out of range: {}",
                h.weekday
            )));
        }
        if !(0..=MINUTES_PER_DAY).contains(&h.start_minute)
            || !(0..=MINUTES_PER_DAY).contains(&h.end_minute)
            || h.end_minute <= h.start_minute
        {
            return Err(CalendarError::BadRequest(
                "invalid window: need 0<=start<end<=1440".into(),
            ));
        }
        let idx = h.weekday as usize;
        if seen[idx] {
            return Err(CalendarError::BadRequest(format!(
                "duplicate weekday: {}",
                h.weekday
            )));
        }
        seen[idx] = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wh(weekday: i16, start: i32, end: i32) -> WorkingHour {
        WorkingHour {
            weekday,
            start_minute: start,
            end_minute: end,
        }
    }

    #[test]
    fn validate_accepts_typical_week() {
        let hours: Vec<_> = (1..=5).map(|d| wh(d, 9 * 60, 17 * 60)).collect();
        assert!(validate(&hours).is_ok());
    }

    #[test]
    fn validate_rejects_bad_weekday() {
        assert!(validate(&[wh(7, 540, 1020)]).is_err());
        assert!(validate(&[wh(-1, 540, 1020)]).is_err());
    }

    #[test]
    fn validate_rejects_inverted_window() {
        assert!(validate(&[wh(1, 1020, 540)]).is_err());
        assert!(validate(&[wh(1, 600, 600)]).is_err());
    }

    #[test]
    fn validate_rejects_out_of_range_minutes() {
        assert!(validate(&[wh(1, 0, 1441)]).is_err());
    }

    #[test]
    fn validate_rejects_duplicate_weekday() {
        assert!(validate(&[wh(1, 540, 600), wh(1, 700, 800)]).is_err());
    }

    #[test]
    fn validate_accepts_empty() {
        assert!(validate(&[]).is_ok());
    }

    #[test]
    fn body_deser() {
        let b: WorkingHoursBody = serde_json::from_str(
            r#"{"hours":[{"weekday":1,"start_minute":540,"end_minute":1020}]}"#,
        )
        .unwrap();
        assert_eq!(b.hours.len(), 1);
        assert_eq!(b.hours[0].weekday, 1);
    }
}

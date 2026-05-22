//! VALARM REST endpoints — sprint #401 + PATCH/GET single (sprint #412)
//! + cross-event upcoming list (sprint #418).
//!
//! Routes:
//!   GET    /api/v1/calendars/:cal_id/events/:event_id/alarms
//!   POST   /api/v1/calendars/:cal_id/events/:event_id/alarms
//!   DELETE /api/v1/calendars/:cal_id/events/:event_id/alarms              (sprint #422)
//!   GET    /api/v1/calendars/:cal_id/events/:event_id/alarms/:alarm_uid
//!   PATCH  /api/v1/calendars/:cal_id/events/:event_id/alarms/:alarm_uid
//!   DELETE /api/v1/calendars/:cal_id/events/:event_id/alarms/:alarm_uid
//!   GET    /api/v1/calendars/:cal_id/alarms/upcoming?within_hours=N
//!   GET    /api/v1/calendars/:cal_id/alarms/count?delivered=false        (sprint #428)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::api::events::assert_can_write;
use crate::error::{CalendarError, Result};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct EventAlarm {
    pub id:          Uuid,
    pub event_id:    Uuid,
    pub calendar_id: Uuid,
    pub tenant_id:   Uuid,
    pub uid:         String,
    pub action:      String,
    pub trigger_rel: Option<String>,
    pub trigger_abs: Option<OffsetDateTime>,
    pub description: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at:  OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct PatchAlarmBody {
    action:      Option<String>,
    trigger_rel: Option<String>,
    trigger_abs: Option<OffsetDateTime>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateAlarmBody {
    uid:         Option<String>,
    action:      Option<String>,
    trigger_rel: Option<String>,
    trigger_abs: Option<OffsetDateTime>,
    description: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/calendars/:cal_id/events/:event_id/alarms",
            get(list_alarms).post(create_alarm).delete(delete_all_alarms),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:event_id/alarms/:alarm_uid",
            get(get_alarm).patch(patch_alarm).delete(delete_alarm),
        )
        .route(
            "/api/v1/calendars/:cal_id/alarms/upcoming",
            get(list_upcoming_alarms),
        )
        .route(
            "/api/v1/calendars/:cal_id/alarms/count",
            get(count_alarms),
        )
}

#[derive(Debug, Deserialize)]
struct CountQuery {
    /// Quando definido, filtra por `delivered_at IS NULL` (false) ou `IS NOT NULL` (true).
    /// Omitido ⇒ sem filtro (conta todos os alarmes do calendário).
    delivered: Option<bool>,
}

#[derive(Debug, Serialize)]
struct AlarmsCount {
    count: i64,
}

/// GET /api/v1/calendars/:cal_id/alarms/count?delivered=false — retorna apenas
/// a contagem de alarmes em todos os events do calendário (sprint #428).
/// `?delivered=false` filtra pelos pendentes; `?delivered=true` pelos já entregues;
/// omitido conta todos. Útil pra badge "alarms pendentes" no header do calendário.
async fn count_alarms(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q): Query<CountQuery>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let cal_exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM calendars WHERE id = $1 AND tenant_id = $2",
    )
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if cal_exists.is_none() {
        return Err(CalendarError::CalendarNotFound(cal_id.to_string()));
    }

    let delivered_filter = match q.delivered {
        Some(true)  => "AND delivered_at IS NOT NULL",
        Some(false) => "AND delivered_at IS NULL",
        None        => "",
    };

    let sql = format!(
        "SELECT COUNT(*) FROM calendar_event_alarms \
         WHERE tenant_id = $1 AND calendar_id = $2 {delivered_filter}"
    );
    let (count,): (i64,) = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(cal_id)
        .fetch_one(pool)
        .await?;

    Ok(Json(AlarmsCount { count }))
}

#[derive(Debug, Deserialize)]
struct UpcomingQuery {
    /// Janela em horas a partir de now() (default 24, max 168 = 7 dias).
    within_hours: Option<u32>,
}

/// GET /api/v1/calendars/:cal_id/alarms/upcoming — lista alarmes não-entregues
/// com `trigger_abs` entre now() e now()+within_hours, em todos os events do
/// calendário (sprint #418).
async fn list_upcoming_alarms(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q): Query<UpcomingQuery>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let cal_exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM calendars WHERE id = $1 AND tenant_id = $2",
    )
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if cal_exists.is_none() {
        return Err(CalendarError::CalendarNotFound(cal_id.to_string()));
    }

    let hours = q.within_hours.unwrap_or(24).clamp(1, 168) as i64;

    let alarms: Vec<EventAlarm> = sqlx::query_as(
        "SELECT a.id, a.event_id, a.calendar_id, a.tenant_id, a.uid, a.action, \
                a.trigger_rel, a.trigger_abs, a.description, a.created_at \
         FROM calendar_event_alarms a \
         WHERE a.tenant_id = $1 \
           AND a.calendar_id = $2 \
           AND a.delivered_at IS NULL \
           AND a.trigger_abs IS NOT NULL \
           AND a.trigger_abs >= now() \
           AND a.trigger_abs <= now() + ($3 || ' hours')::interval \
         ORDER BY a.trigger_abs ASC",
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(hours.to_string())
    .fetch_all(pool)
    .await?;

    Ok(Json(alarms))
}

async fn list_alarms(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, event_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    // Verify event belongs to calendar+tenant
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM calendar_events WHERE id = $1 AND calendar_id = $2 AND tenant_id = $3",
    )
    .bind(event_id)
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(CalendarError::EventNotFound(event_id));
    }

    let alarms: Vec<EventAlarm> = sqlx::query_as(
        "SELECT id, event_id, calendar_id, tenant_id, uid, action, trigger_rel, trigger_abs, description, created_at \
         FROM calendar_event_alarms \
         WHERE tenant_id = $1 AND event_id = $2 \
         ORDER BY created_at ASC",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .fetch_all(pool)
    .await?;

    Ok(Json(alarms))
}

async fn create_alarm(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, event_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<CreateAlarmBody>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    // Verify event belongs to calendar+tenant
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM calendar_events WHERE id = $1 AND calendar_id = $2 AND tenant_id = $3",
    )
    .bind(event_id)
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(CalendarError::EventNotFound(event_id));
    }

    let uid = body.uid.unwrap_or_else(|| Uuid::new_v4().to_string());
    let action = body.action.unwrap_or_else(|| "DISPLAY".into());

    if body.trigger_rel.is_none() && body.trigger_abs.is_none() {
        return Err(CalendarError::BadRequest(
            "trigger_rel or trigger_abs is required".into(),
        ));
    }

    let alarm: EventAlarm = sqlx::query_as(
        "INSERT INTO calendar_event_alarms \
             (event_id, calendar_id, tenant_id, uid, action, trigger_rel, trigger_abs, description) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, event_id, calendar_id, tenant_id, uid, action, trigger_rel, trigger_abs, description, created_at",
    )
    .bind(event_id)
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .bind(&uid)
    .bind(&action)
    .bind(&body.trigger_rel)
    .bind(body.trigger_abs)
    .bind(&body.description)
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(alarm)))
}

async fn delete_alarm(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, event_id, alarm_uid)): Path<(Uuid, Uuid, String)>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let deleted = sqlx::query(
        "DELETE FROM calendar_event_alarms \
         WHERE tenant_id = $1 AND event_id = $2 AND uid = $3",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .bind(&alarm_uid)
    .execute(pool)
    .await?;

    if deleted.rows_affected() == 0 {
        return Err(CalendarError::AlarmNotFound(event_id));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/calendars/:cal_id/events/:event_id/alarms — apaga todos os alarmes
/// do evento de uma vez (sprint #422). Retorna 204 sempre que o evento existe, mesmo
/// se a contagem deletada for zero (idempotência).
async fn delete_all_alarms(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, event_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM calendar_events WHERE id = $1 AND calendar_id = $2 AND tenant_id = $3",
    )
    .bind(event_id)
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(CalendarError::EventNotFound(event_id));
    }

    sqlx::query(
        "DELETE FROM calendar_event_alarms \
         WHERE tenant_id = $1 AND event_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .execute(pool)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/calendars/:cal_id/events/:event_id/alarms/:alarm_uid — fetch a single alarm.
async fn get_alarm(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, event_id, alarm_uid)): Path<(Uuid, Uuid, String)>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM calendar_events WHERE id = $1 AND calendar_id = $2 AND tenant_id = $3",
    )
    .bind(event_id)
    .bind(cal_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(CalendarError::EventNotFound(event_id));
    }

    let alarm: Option<EventAlarm> = sqlx::query_as(
        "SELECT id, event_id, calendar_id, tenant_id, uid, action, trigger_rel, trigger_abs, description, created_at \
         FROM calendar_event_alarms \
         WHERE tenant_id = $1 AND event_id = $2 AND uid = $3",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .bind(&alarm_uid)
    .fetch_optional(pool)
    .await?;

    match alarm {
        Some(a) => Ok(Json(a)),
        None    => Err(CalendarError::AlarmNotFound(event_id)),
    }
}

/// PATCH /api/v1/calendars/:cal_id/events/:event_id/alarms/:alarm_uid — update an alarm.
///
/// Only fields provided in the body are updated; omitted fields are kept as-is.
async fn patch_alarm(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, event_id, alarm_uid)): Path<(Uuid, Uuid, String)>,
    Json(body): Json<PatchAlarmBody>,
) -> Result<impl IntoResponse> {
    if body.action.is_none()
        && body.trigger_rel.is_none()
        && body.trigger_abs.is_none()
        && body.description.is_none()
    {
        return Err(CalendarError::BadRequest("no fields to update".into()));
    }

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated: Option<EventAlarm> = sqlx::query_as(
        "UPDATE calendar_event_alarms SET \
             action      = COALESCE($4, action), \
             trigger_rel = CASE WHEN $5::text IS NOT NULL THEN $5 ELSE trigger_rel END, \
             trigger_abs = CASE WHEN $6::timestamptz IS NOT NULL THEN $6 ELSE trigger_abs END, \
             description = CASE WHEN $7::text IS NOT NULL THEN $7 ELSE description END \
         WHERE tenant_id = $1 AND event_id = $2 AND uid = $3 \
         RETURNING id, event_id, calendar_id, tenant_id, uid, action, trigger_rel, trigger_abs, description, created_at",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .bind(&alarm_uid)
    .bind(&body.action)
    .bind(&body.trigger_rel)
    .bind(body.trigger_abs)
    .bind(&body.description)
    .fetch_optional(pool)
    .await?;

    match updated {
        Some(a) => Ok(Json(a)),
        None    => Err(CalendarError::AlarmNotFound(event_id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_alarm_body_all_optional() {
        let json = r#"{}"#;
        let b: CreateAlarmBody = serde_json::from_str(json).unwrap();
        assert!(b.uid.is_none());
        assert!(b.action.is_none());
        assert!(b.trigger_rel.is_none());
        assert!(b.description.is_none());
    }

    #[test]
    fn patch_alarm_body_partial_update() {
        let json = r#"{"action":"DISPLAY"}"#;
        let b: PatchAlarmBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.action.as_deref(), Some("DISPLAY"));
        assert!(b.trigger_rel.is_none());
    }

    #[test]
    fn count_query_delivered_filter() {
        let json = r#"{"delivered":false}"#;
        let q: CountQuery = serde_json::from_str(json).unwrap();
        assert_eq!(q.delivered, Some(false));
    }

    #[test]
    fn count_query_no_filter() {
        let q: CountQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.delivered.is_none());
    }

    #[test]
    fn create_alarm_body_all_optional_fields() {
        let json = r#"{"trigger_rel":"-PT15M","action":"AUDIO","description":"Ring"}"#;
        let b: CreateAlarmBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.trigger_rel.as_deref(), Some("-PT15M"));
        assert_eq!(b.action.as_deref(), Some("AUDIO"));
        assert_eq!(b.description.as_deref(), Some("Ring"));
    }

    #[test]
    fn patch_alarm_body_all_none_by_default() {
        let b: PatchAlarmBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.action.is_none());
        assert!(b.trigger_rel.is_none());
    }

    #[test]
    fn patch_alarm_body_action_email_set() {
        let b: PatchAlarmBody = serde_json::from_str(r#"{"action":"EMAIL"}"#).unwrap();
        assert_eq!(b.action.as_deref(), Some("EMAIL"));
    }

    #[test]
    fn patch_alarm_body_trigger_rel_set() {
        let b: PatchAlarmBody = serde_json::from_str(r#"{"trigger_rel":"-PT15M"}"#).unwrap();
        assert_eq!(b.trigger_rel.as_deref(), Some("-PT15M"));
    }

    #[test]
    fn patch_alarm_body_description_set() {
        let b: PatchAlarmBody = serde_json::from_str(r#"{"description":"Reminder"}"#).unwrap();
        assert_eq!(b.description.as_deref(), Some("Reminder"));
    }

    #[test]
    fn patch_alarm_body_all_none_when_empty() {
        let b: PatchAlarmBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.action.is_none());
        assert!(b.description.is_none());
    }
}

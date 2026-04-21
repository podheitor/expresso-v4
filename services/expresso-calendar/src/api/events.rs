//! Event REST endpoints (JSON out, text/calendar in for POST/PUT).

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Event, EventQuery, EventRepo};
use crate::error::{CalendarError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/calendars/:cal_id/events",
            post(create).get(list),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id",
            get(get_one).put(update).delete(delete),
        )
}

/// POST body is raw iCalendar (VCALENDAR wrapping one VEVENT).
async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    if raw.trim().is_empty() {
        return Err(CalendarError::BadRequest("empty body".into()));
    }
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).create(ctx.tenant_id, cal_id, &raw).await?;

    let etag = format!("\"{}\"", ev.etag);
    let location = format!("/api/v1/calendars/{}/events/{}", ev.calendar_id, ev.id);

    let mut resp = (StatusCode::CREATED, Json(ev)).into_response();
    resp.headers_mut().insert(header::ETAG,     HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&location).unwrap());
    Ok(resp)
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q): Query<EventQuery>,
) -> Result<Json<Vec<Event>>> {
    let pool = state.db_or_unavailable()?;
    let events = EventRepo::new(pool).list(ctx.tenant_id, cal_id, &q).await?;
    Ok(Json(events))
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}\"", ev.etag);
    let mut resp = Json(ev).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    Ok(resp)
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
    raw: String,
) -> Result<Response> {
    if raw.trim().is_empty() {
        return Err(CalendarError::BadRequest("empty body".into()));
    }
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;
    let etag = format!("\"{}\"", ev.etag);
    let mut resp = Json(ev).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    Ok(resp)
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    EventRepo::new(pool).delete(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

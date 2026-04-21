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
        .route(
            "/api/v1/calendars/:cal_id/export.ics",
            get(export_ics),
        )
        .route(
            "/api/v1/calendars/:cal_id/import",
            post(import_ics),
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


/// GET /api/v1/calendars/:cal_id/export.ics — returns all events as a single
/// VCALENDAR (text/calendar). Unauthenticated CalDAV clients can also fetch
/// raw calendar via CalDAV REPORT; this endpoint is for simple downloads.
async fn export_ics(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
) -> Result<Response> {
    use crate::domain::ical;

    let pool = state.db_or_unavailable()?;
    let events = EventRepo::new(pool)
        .list(ctx.tenant_id, cal_id, &crate::domain::EventQuery::default())
        .await?;

    let blocks: Vec<String> = events
        .iter()
        .filter_map(|e| ical::extract_vevent_block(&e.ical_raw))
        .collect();
    let body = ical::wrap_vcalendar(&blocks);

    let mut resp = (StatusCode::OK, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"calendar.ics\""),
    );
    Ok(resp)
}

/// POST /api/v1/calendars/:cal_id/import — accepts a VCALENDAR body with one
/// or more VEVENTs. Each VEVENT is upserted individually. Returns a summary
/// `{"imported": N, "failed": M, "errors": [..]}`. 4xx errors per-event are
/// captured but don't abort the batch.
async fn import_ics(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    use crate::domain::ical;

    if raw.trim().is_empty() {
        return Err(CalendarError::BadRequest("empty body".into()));
    }
    let blocks = ical::split_vcalendar_to_events(&raw);
    if blocks.is_empty() {
        return Err(CalendarError::BadRequest("no VEVENT blocks found in payload".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = EventRepo::new(pool);

    let mut imported: usize = 0;
    let mut errors: Vec<String> = Vec::new();
    for (idx, block) in blocks.iter().enumerate() {
        match repo.create(ctx.tenant_id, cal_id, block).await {
            Ok(_) => imported += 1,
            Err(e) => errors.push(format!("event[{idx}]: {e}")),
        }
    }

    let body = serde_json::json!({
        "imported": imported,
        "failed":   errors.len(),
        "errors":   errors,
    });
    Ok((StatusCode::OK, Json(body)).into_response())
}

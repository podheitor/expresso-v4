//! Event REST endpoints (JSON out, text/calendar in for POST/PUT).

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Event, EventQuery, EventRepo};
use crate::error::{CalendarError, Result};
use crate::state::AppState;

/// Cap pro VCALENDAR de UM evento (create/update). Eventos reais com
/// participantes/VALARM/recurrence ficam em poucos KiB; 256 KiB cobre
/// até agendas insanas. Acima disso é abuso — engasga storage,
/// parser, e cada delivery iTIP downstream.
pub const MAX_EVENT_ICS_BYTES: usize = 256 * 1024;

/// Cap pro VCALENDAR de IMPORT em batch (multi-VEVENT). Mais largo
/// pra cobrir migrações reais (anos de calendário compactado num
/// único upload), mas ainda finito — 2 MiB cobre dezenas de milhares
/// de eventos típicos.
pub const MAX_IMPORT_ICS_BYTES: usize = 2 * 1024 * 1024;

/// Gate: require OWNER/WRITE/ADMIN on the calendar, else 403.
pub(crate) async fn assert_can_write(
    pool: &expresso_core::DbPool,
    tenant_id: uuid::Uuid,
    cal_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<()> {
    let repo = crate::domain::CalendarRepo::new(pool);
    let lvl = repo.access_level(tenant_id, cal_id, user_id).await?;
    match lvl.as_deref() {
        Some("OWNER") | Some("WRITE") | Some("ADMIN") => Ok(()),
        Some("READ") => Err(crate::error::CalendarError::Forbidden),
        Some(_)      => Err(crate::error::CalendarError::Forbidden),
        None         => Err(crate::error::CalendarError::CalendarNotFound(cal_id.to_string())),
    }
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/calendars/:cal_id/events",
            post(create).get(list),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-count",
            get(count_events),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-histogram",
            get(events_histogram),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-digest",
            get(events_digest),
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
        .route(
            "/api/v1/calendars/:cal_id/events/:id/itip/request.ics",
            get(itip_request),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/rsvp",
            post(rsvp),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/attendees",
            get(list_attendees),
        )
}

/// POST body is raw iCalendar (VCALENDAR wrapping one VEVENT).
async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    validate_ics(&raw, MAX_EVENT_ICS_BYTES)?;
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    let ev = EventRepo::new(pool).create(ctx.tenant_id, cal_id, &raw).await?;

    state.events().publish(crate::events::Event::EventCreated {
        tenant_id: ctx.tenant_id, event_id: ev.id, summary: ev.summary.clone(),
    });
    state.events().publish_imip(ev.clone(), "REQUEST");

    let etag = format!("\"{}\"", ev.etag);
    let location = format!("/api/v1/calendars/{}/events/{}", ev.calendar_id, ev.id);

    let mut resp = (StatusCode::CREATED, Json(ev)).into_response();
    resp.headers_mut().insert(header::ETAG,     HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&location).unwrap());
    Ok(resp)
}

/// GET /api/v1/calendars/:cal_id/events-count?from=&to= — conta eventos do calendário
/// (sprint #432). Usa os mesmos filtros from/to de `list` mas sem paginação nem
/// caching headers; útil pra badges/dashboards. Path com hífen evita colisão com
/// `events/:id` (lição do sprint #427). RLS filtra tenant; não verifica access_level
/// porque list também não verifica — quem não tem visibilidade vê 0.
async fn count_events(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q): Query<EventQuery>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let (count,): (i64,) = sqlx::query_as(
        r#"SELECT COUNT(*) FROM calendar_events
            WHERE tenant_id = $1
              AND calendar_id = $2
              AND ($3::timestamptz IS NULL OR dtend   IS NULL OR dtend   >= $3)
              AND ($4::timestamptz IS NULL OR dtstart IS NULL OR dtstart <= $4)"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.from)
    .bind(q.to)
    .fetch_one(pool)
    .await?;
    Ok(Json(serde_json::json!({ "count": count })).into_response())
}

/// GET /api/v1/calendars/:cal_id/events-histogram?from=&to=&bucket=day
/// Agrupa eventos do calendário por bucket temporal (dtstart trunc) e retorna
/// {bucket, series:[{ts, count}]} ordenado ASC. Bucket aceita day (default),
/// week ou month — whitelist obrigatória antes de injetar em date_trunc()
/// (lição #435: SQL injection sem whitelist). Eventos sem dtstart são
/// ignorados (NULL não agrupa). Útil pra heatmap de calendário sem listar.
async fn events_histogram(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(params): Query<EventsHistogramParams>,
) -> Result<Response> {
    let bucket = match params.bucket.as_deref().unwrap_or("day") {
        "day" => "day",
        "week" => "week",
        "month" => "month",
        other => return Err(CalendarError::BadRequest(format!(
            "bucket must be day|week|month, got {other}"
        ))),
    };
    let pool = state.db_or_unavailable()?;
    let sql = format!(
        r#"SELECT date_trunc('{bucket}', dtstart) AS ts, COUNT(*)::bigint AS count
             FROM calendar_events
            WHERE tenant_id = $1
              AND calendar_id = $2
              AND dtstart IS NOT NULL
              AND ($3::timestamptz IS NULL OR dtstart >= $3)
              AND ($4::timestamptz IS NULL OR dtstart <= $4)
            GROUP BY ts
            ORDER BY ts ASC"#,
    );
    let rows: Vec<(OffsetDateTime, i64)> = sqlx::query_as(&sql)
        .bind(ctx.tenant_id)
        .bind(cal_id)
        .bind(params.from)
        .bind(params.to)
        .fetch_all(pool)
        .await?;
    let series: Vec<serde_json::Value> = rows.into_iter()
        .map(|(ts, count)| serde_json::json!({
            "ts":    ts.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            "count": count,
        }))
        .collect();
    Ok(Json(serde_json::json!({ "bucket": bucket, "series": series })).into_response())
}

#[derive(Debug, serde::Deserialize)]
pub struct EventsHistogramParams {
    pub from:   Option<OffsetDateTime>,
    pub to:     Option<OffsetDateTime>,
    pub bucket: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct EventsDigestParams {
    pub day: String,
}

/// GET /api/v1/calendars/:cal_id/events-digest?day=YYYY-MM-DD
/// Retorna VCALENDAR consolidado dos eventos cujo dtstart cai no dia (UTC).
/// Reusa wrap_vcalendar/extract_vevent_block do export_ics; o filtro vem via
/// EventQuery::from/to com bordas [00:00Z, 24:00Z). Path com hífen evita
/// colisão com `events/:id` (lição #427).
async fn events_digest(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(params): Query<EventsDigestParams>,
) -> Result<Response> {
    use crate::domain::ical;
    use time::{Date, Time, format_description::well_known::Iso8601};

    let date = Date::parse(&params.day, &Iso8601::DATE)
        .map_err(|_| CalendarError::BadRequest(format!("day must be YYYY-MM-DD, got {}", params.day)))?;
    let start = date.with_time(Time::MIDNIGHT).assume_utc();
    let end = start + time::Duration::days(1);

    let pool = state.db_or_unavailable()?;
    let q = crate::domain::EventQuery { from: Some(start), to: Some(end), limit: None };
    let events = EventRepo::new(pool).list(ctx.tenant_id, cal_id, &q).await?;

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
        HeaderValue::from_str(&format!("attachment; filename=\"digest-{}.ics\"", params.day))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment; filename=\"digest.ics\"")),
    );
    Ok(resp)
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q): Query<EventQuery>,
    req_headers: HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let (total, max_updated): (i64, Option<OffsetDateTime>) = sqlx::query_as(
        r#"SELECT COUNT(*), MAX(updated_at) FROM calendar_events
            WHERE tenant_id = $1
              AND calendar_id = $2
              AND ($3::timestamptz IS NULL OR dtend   IS NULL OR dtend   >= $3)
              AND ($4::timestamptz IS NULL OR dtstart IS NULL OR dtstart <= $4)"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.from)
    .bind(q.to)
    .fetch_one(pool)
    .await?;
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let events = EventRepo::new(pool).list(ctx.tenant_id, cal_id, &q).await?;
    let mut resp = (
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(events),
    ).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
    req_headers: axum::http::HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}\"", ev.etag);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    let lm = ev.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if ev.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(ev).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    raw: String,
) -> Result<Response> {
    validate_ics(&raw, MAX_EVENT_ICS_BYTES)?;
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    let ev = EventRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;

    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: ev.id,
        summary: ev.summary.clone(), sequence: ev.sequence,
    });
    state.events().publish_imip(ev.clone(), "REQUEST");

    let etag = format!("\"{}\"", ev.etag);
    let mut resp = Json(ev).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    Ok(resp)
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    EventRepo::new(pool).delete(ctx.tenant_id, id).await?;
    state.events().publish(crate::events::Event::EventCancelled {
        tenant_id: ctx.tenant_id, event_id: id,
    });
    Ok(StatusCode::NO_CONTENT)
}


/// GET /api/v1/calendars/:cal_id/export.ics — returns all events as a single
/// VCALENDAR (text/calendar). Unauthenticated CalDAV clients can also fetch
/// raw calendar via CalDAV REPORT; this endpoint is for simple downloads.
async fn export_ics(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    use crate::domain::ical;

    let pool = state.db_or_unavailable()?;

    let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(updated_at) FROM calendar_events WHERE tenant_id = $1 AND calendar_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);

    if let Some(ts) = max_ts {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }

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
    if let Some(ts) = max_ts {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
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

    validate_ics(&raw, MAX_IMPORT_ICS_BYTES)?;
    let blocks = ical::split_vcalendar_to_events(&raw);
    if blocks.is_empty() {
        return Err(CalendarError::BadRequest("no VEVENT blocks found in payload".into()));
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
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


/// GET /api/v1/calendars/:cal_id/events/:id/itip/request.ics — returns the
/// event wrapped with METHOD:REQUEST for SMTP invitation attachment.
async fn itip_request(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((_cal, id)): Path<(Uuid, Uuid)>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    use crate::domain::itip;
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;

    let etag = format!("\"{}-{}\"", ev.updated_at.unix_timestamp(), ev.id);
    let lm   = ev.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();

    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if ev.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }

    let ics = itip::build_request(&ev.ical_raw)?;
    let mut resp = (StatusCode::OK, ics).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/calendar; method=REQUEST; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"invite.ics\""),
    );
    resp.headers_mut().insert(header::ETAG,          HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

#[derive(Debug, serde::Deserialize)]
struct RsvpBody {
    email:    String,
    partstat: String,
}

/// POST /api/v1/calendars/:cal_id/events/:id/rsvp — apply a PARTSTAT to an
/// attendee inside the stored VEVENT. Returns {event, reply_ics} where
/// reply_ics is a METHOD:REPLY VCALENDAR to send back to the organizer.
async fn rsvp(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_cal, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<RsvpBody>,
) -> Result<Response> {
    use crate::domain::itip;
    if body.email.trim().is_empty() {
        return Err(CalendarError::BadRequest("`email` required".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = EventRepo::new(pool);
    let ev = repo.get(ctx.tenant_id, id).await?;

    let new_raw = itip::apply_rsvp(&ev.ical_raw, &body.email, &body.partstat)?;
    let reply   = itip::build_reply(&new_raw, &body.email, &body.partstat)?;
    let updated = repo.update(ctx.tenant_id, id, &new_raw).await?;

    let out = serde_json::json!({
        "event":     updated,
        "reply_ics": reply,
    });
    Ok((StatusCode::OK, Json(out)).into_response())
}

/// GET /api/v1/calendars/:cal_id/events/:id/attendees — parsed attendee list.
async fn list_attendees(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((_cal, id)): Path<(Uuid, Uuid)>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    use crate::domain::itip;
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    let lm = ev.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if ev.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let atts = itip::parse_attendees(&ev.ical_raw);
    let body: Vec<_> = atts.into_iter().map(|a| serde_json::json!({
        "email":    a.email,
        "cn":       a.cn,
        "role":     a.role,
        "partstat": a.partstat,
        "rsvp":     a.rsvp,
    })).collect();
    let mut resp = (StatusCode::OK, Json(body)).into_response();
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

/// Gate aplicado em todos os endpoints que aceitam VCALENDAR raw.
/// Tamanho primeiro pra rejeitar abuso antes de tocar o parser.
fn validate_ics(raw: &str, max_bytes: usize) -> Result<()> {
    if raw.trim().is_empty() {
        return Err(CalendarError::BadRequest("empty body".into()));
    }
    if raw.len() > max_bytes {
        return Err(CalendarError::BadRequest(format!(
            "ics payload too large: {} bytes (max {})",
            raw.len(), max_bytes
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        let err = format!("{:?}", validate_ics("", MAX_EVENT_ICS_BYTES).unwrap_err());
        assert!(err.contains("empty body"), "got: {err}");
        let err = format!("{:?}", validate_ics("   \n  ", MAX_EVENT_ICS_BYTES).unwrap_err());
        assert!(err.contains("empty body"), "got: {err}");
    }

    #[test]
    fn accepts_small_event() {
        let s = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:abc\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert!(validate_ics(s, MAX_EVENT_ICS_BYTES).is_ok());
    }

    #[test]
    fn rejects_oversize_event() {
        let s = "x".repeat(MAX_EVENT_ICS_BYTES + 1);
        let err = format!("{:?}", validate_ics(&s, MAX_EVENT_ICS_BYTES).unwrap_err());
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn import_cap_higher_than_event_cap() {
        // Garantir que payload entre EVENT_MAX e IMPORT_MAX passa só
        // pelo caminho de import (semântica intencional: bulk import
        // pode ser maior que evento individual).
        let s = "x".repeat(MAX_EVENT_ICS_BYTES + 1);
        assert!(validate_ics(&s, MAX_EVENT_ICS_BYTES).is_err());
        assert!(validate_ics(&s, MAX_IMPORT_ICS_BYTES).is_ok());
    }

    #[test]
    fn rejects_oversize_import() {
        let s = "x".repeat(MAX_IMPORT_ICS_BYTES + 1);
        let err = format!("{:?}", validate_ics(&s, MAX_IMPORT_ICS_BYTES).unwrap_err());
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn boundary_event_accepted() {
        let s = "x".repeat(MAX_EVENT_ICS_BYTES);
        assert!(validate_ics(&s, MAX_EVENT_ICS_BYTES).is_ok());
    }
}

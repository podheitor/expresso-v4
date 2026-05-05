//! Event REST endpoints (JSON out, text/calendar in for POST/PUT).

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete as delete_route, get, patch, post},
    Json, Router,
};
use time::OffsetDateTime;
use uuid::Uuid;

use expresso_core::begin_tenant_tx;
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
            "/api/v1/calendars/:cal_id/events-digest-range",
            get(events_digest_range),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-conflicts",
            get(events_conflicts),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-conflicts-count",
            get(events_conflicts_count),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-bulk-delete",
            post(events_bulk_delete),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-recurrence-stats",
            get(events_recurrence_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-recurrence-monthly",
            get(events_recurrence_monthly),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-instances",
            get(events_instances_bulk),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range",
            get(events_by_range_preview),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/stats",
            get(events_by_range_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/move",
            patch(events_by_range_move),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-status",
            patch(events_by_range_set_status),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/clear-rrule",
            patch(events_by_range_clear_rrule),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-summary",
            patch(events_by_range_set_summary),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-location",
            patch(events_by_range_set_location),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-description",
            patch(events_by_range_set_description),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-organizer-email",
            patch(events_by_range_set_organizer_email),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-rrule",
            patch(events_by_range_set_rrule),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-text-fields",
            patch(events_by_range_set_text_fields),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-class",
            patch(events_by_range_set_class),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-transparency",
            patch(events_by_range_set_transparency),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/cleanup-orphans",
            patch(events_by_range_cleanup_orphans),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/reindex-fts",
            patch(events_by_range_reindex_fts),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/resend-itip",
            patch(events_by_range_resend_itip),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/set-attendees",
            patch(events_by_range_set_attendees),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/export",
            get(events_by_range_export),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/attendees",
            get(events_by_range_attendees),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/organizers",
            get(events_by_range_organizers),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/locations",
            get(events_by_range_locations),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/summaries",
            get(events_by_range_summaries),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/duration-stats",
            get(events_by_range_duration_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/status-timeline",
            get(events_by_range_status_timeline),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/rrule-stats",
            get(events_by_range_rrule_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/class-stats",
            get(events_by_range_class_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/transp-stats",
            get(events_by_range_transp_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/attendee-count-stats",
            get(events_by_range_attendee_count_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/location-stats",
            get(events_by_range_location_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/organizer-stats",
            get(events_by_range_organizer_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/status-stats",
            get(events_by_range_status_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/priority-stats",
            get(events_by_range_priority_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/duration-distribution",
            get(events_by_range_duration_distribution),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/recurrence-duration-stats",
            get(events_by_range_recurrence_duration_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events-by-range/all-day-stats",
            get(events_by_range_all_day_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/class-distribution",
            get(events_class_distribution),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id",
            get(get_one).put(update).patch(patch_event).delete(delete),
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
            "/api/v1/calendars/:cal_id/events/:id/history",
            get(event_history).delete(delete_event_history),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/history/stats",
            get(event_history_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/history/:entry_id",
            get(get_history_entry),
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
        .route(
            "/api/v1/calendars/:cal_id/events/:id/instances",
            get(events_instances),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/cancel-instance",
            post(cancel_event_instance),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/exdates",
            get(list_exdates).delete(clear_exdates),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/exdates/stats",
            get(exdates_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/exdates/:instance",
            delete_route(delete_exdate),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/exdates/:instance/override",
            post(migrate_cancel_to_override),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/override-instance",
            post(override_event_instance),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides",
            get(list_overrides),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides/stats",
            get(overrides_stats),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id",
            get(get_one_override).delete(delete_override).patch(patch_override),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id/cancel",
            post(migrate_override_to_cancel),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id/touch",
            post(touch_override),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/touch",
            post(touch_master),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/touch-overrides",
            post(touch_overrides_bulk),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/touch-all",
            post(touch_all),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/touch-overrides-by-range",
            post(touch_overrides_by_range),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides-by-range",
            patch(patch_overrides_by_range),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides-by-range/preview",
            get(patch_overrides_by_range_preview),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/touch-preview",
            get(touch_preview),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/exdates-preview",
            get(exdates_preview),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/exdates-preview/stats",
            get(exdates_preview_stats),
        )
        .route(
            "/api/v1/calendars/events/search",
            get(events_search),
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

#[derive(Debug, serde::Deserialize)]
pub struct EventsDigestRangeParams {
    pub from: OffsetDateTime,
    pub to:   OffsetDateTime,
}

/// GET /api/v1/calendars/:cal_id/events-digest-range?from=&to=
/// Digest de eventos cujo dtstart cai em [from, to). Variant de events-digest
/// pra ranges arbitrários (semana, sprint, mês). Reusa EventRepo::list +
/// extract_vevent_block + wrap_vcalendar igual events-digest single-day.
async fn events_digest_range(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Path(cal_id):  Path<Uuid>,
    Query(params): Query<EventsDigestRangeParams>,
) -> Result<Response> {
    use crate::domain::ical;

    if params.from >= params.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }

    let pool = state.db_or_unavailable()?;
    let q = crate::domain::EventQuery {
        from: Some(params.from), to: Some(params.to), limit: None,
    };
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
        HeaderValue::from_static("attachment; filename=\"digest-range.ics\""),
    );
    Ok(resp)
}

#[derive(Debug, serde::Deserialize)]
pub struct EventsConflictsParams {
    pub from: OffsetDateTime,
    pub to:   OffsetDateTime,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct ConflictPairRow {
    a_id:      Uuid,
    a_summary: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    a_dtstart: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    a_dtend:   Option<OffsetDateTime>,
    b_id:      Uuid,
    b_summary: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    b_dtstart: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    b_dtend:   Option<OffsetDateTime>,
}

/// GET /api/v1/calendars/:cal_id/events-conflicts?from=&to=
/// Retorna pares de eventos que se sobrepõem temporalmente dentro do range.
/// Self-join em calendar_events com `a.id < b.id` pra evitar pares duplicados
/// (a,b)/(b,a). Overlap clássico: `a.dtstart < b.dtend AND b.dtstart < a.dtend`.
/// Eventos sem dtstart/dtend são ignorados (não há intervalo a comparar).
/// Útil pra UI de "double booking" — destaca slots conflitantes antes do envio
/// de invites. Mantemos hífen no path (events-conflicts) seguindo padrão das
/// outras rotas estáticas sob /:cal_id que conflitariam com /:event_id.
async fn events_conflicts(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Path(cal_id):  Path<Uuid>,
    Query(params): Query<EventsConflictsParams>,
) -> Result<Json<serde_json::Value>> {
    if params.from >= params.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }

    let pool = state.db_or_unavailable()?;
    let rows: Vec<ConflictPairRow> = sqlx::query_as(
        r#"SELECT a.id AS a_id, a.summary AS a_summary, a.dtstart AS a_dtstart, a.dtend AS a_dtend,
                  b.id AS b_id, b.summary AS b_summary, b.dtstart AS b_dtstart, b.dtend AS b_dtend
             FROM calendar_events a
             JOIN calendar_events b
               ON b.tenant_id   = a.tenant_id
              AND b.calendar_id = a.calendar_id
              AND b.id          > a.id
            WHERE a.tenant_id   = $1
              AND a.calendar_id = $2
              AND a.dtstart IS NOT NULL AND a.dtend IS NOT NULL
              AND b.dtstart IS NOT NULL AND b.dtend IS NOT NULL
              AND a.dtstart < b.dtend
              AND b.dtstart < a.dtend
              AND a.dtend   >  $3
              AND a.dtstart <  $4
              AND b.dtend   >  $3
              AND b.dtstart <  $4
            ORDER BY a.dtstart ASC, b.dtstart ASC"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(params.from)
    .bind(params.to)
    .fetch_all(pool)
    .await?;

    let from_s = params.from.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::new());
    let to_s   = params.to.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::new());
    Ok(Json(serde_json::json!({
        "from":      from_s,
        "to":        to_s,
        "conflicts": rows,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-conflicts-count?from=&to= — counter
/// version of `events-conflicts` (sprint #474). Mesma self-join + overlap
/// classico (`a.dtstart < b.dtend AND b.dtstart < a.dtend` clamped pelo range)
/// com `a.id < b.id` pra evitar pares duplicados, mas retorna apenas a
/// contagem de pares conflitantes em vez do payload completo. Útil pra widgets
/// "X conflitos detectados" antes do user decidir abrir a lista. Hífen no path
/// segue mesmo padrão de `events-conflicts` evitando colisão com `:event_id`.
async fn events_conflicts_count(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Path(cal_id):  Path<Uuid>,
    Query(params): Query<EventsConflictsParams>,
) -> Result<Json<serde_json::Value>> {
    if params.from >= params.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }

    let pool = state.db_or_unavailable()?;
    let count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
             FROM calendar_events a
             JOIN calendar_events b
               ON b.tenant_id   = a.tenant_id
              AND b.calendar_id = a.calendar_id
              AND b.id          > a.id
            WHERE a.tenant_id   = $1
              AND a.calendar_id = $2
              AND a.dtstart IS NOT NULL AND a.dtend IS NOT NULL
              AND b.dtstart IS NOT NULL AND b.dtend IS NOT NULL
              AND a.dtstart < b.dtend
              AND b.dtstart < a.dtend
              AND a.dtend   >  $3
              AND a.dtstart <  $4
              AND b.dtend   >  $3
              AND b.dtstart <  $4"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(params.from)
    .bind(params.to)
    .fetch_one(pool)
    .await?;

    let from_s = params.from.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::new());
    let to_s   = params.to.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::new());
    Ok(Json(serde_json::json!({
        "from":  from_s,
        "to":    to_s,
        "count": count,
    })))
}

#[derive(Debug, serde::Deserialize)]
pub struct EventsBulkDeleteParams {
    pub from: OffsetDateTime,
    pub to:   OffsetDateTime,
}

/// POST /api/v1/calendars/:cal_id/events-bulk-delete?from=&to= — apaga em massa
/// todos os eventos cujo `dtstart` ∈ `[from, to)` no calendário (sprint #457).
/// Útil pra cleanup pós-import duplicado, sweep de calendário antigo, ou
/// remoção sazonal (apagar todos os eventos do trimestre passado). Eventos sem
/// `dtstart` são preservados — não há critério temporal pra incluí-los.
/// Requer WRITE (mesmo gate do delete single). POST em vez de DELETE pra
/// evitar query string em verbo DELETE (alguns proxies/CDNs descartam body
/// e query) e marcar a operação como "irreversível, lê params".
async fn events_bulk_delete(
    State(state):  State<AppState>,
    ctx:           RequestCtx,
    Path(cal_id):  Path<Uuid>,
    Query(params): Query<EventsBulkDeleteParams>,
) -> Result<Json<serde_json::Value>> {
    if params.from >= params.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let deleted = EventRepo::new(pool)
        .delete_range(ctx.tenant_id, cal_id, params.from, params.to)
        .await?;

    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangePreviewQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    limit:  Option<i64>,
    /// Keyset cursor: RFC3339 `dtstart` of the last event from the previous page.
    /// When present, returns events with `dtstart > cursor`. Enables stable
    /// cursor pagination over large ranges without offset drift. Sprint #604.
    #[serde(default, with = "time::serde::rfc3339::option")]
    cursor: Option<OffsetDateTime>,
}

/// GET /api/v1/calendars/:cal_id/events-by-range?after=&before=&limit=&cursor=
///
/// Read-only listagem dos eventos cujo `dtstart` ∈ `[after, before)` no calendário.
/// `cursor` é o RFC3339 dtstart do último evento da página anterior — retorna
/// eventos com `dtstart > cursor` dentro do range original (keyset pagination).
/// `next_cursor` no response é o dtstart do último evento retornado, ou null se
/// não há mais páginas. `count == limit` indica possível próxima página.
/// Sprints #544 (foundation) + #604 (cursor pagination).
async fn events_by_range_preview(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangePreviewQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(1000).clamp(1, 10_000);

    // Keyset lower bound: max(after, cursor+1ns). cursor is exclusive (>), not >=.
    // We implement "> cursor" via "after_effective = cursor" + SQL "> $cursor".
    let effective_after = match (q.after, q.cursor) {
        (Some(a), Some(c)) => Some(if c > a { c } else { a }),
        (None,    Some(c)) => Some(c),
        (after,   None)    => after,
    };
    let has_cursor = q.cursor.is_some();

    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;
    // When cursor is active we use strict > on dtstart (keyset pagination).
    // Without cursor we use >= (inclusive lower bound from `after`).
    let rows: Vec<Event> = if has_cursor {
        sqlx::query_as::<_, Event>(
            r#"SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                      description, location, dtstart, dtend, rrule, status,
                      class, transp, sequence, organizer_email, created_at, updated_at
                 FROM calendar_events
                WHERE tenant_id    = $1
                  AND calendar_id  = $2
                  AND dtstart IS NOT NULL
                  AND ($3::timestamptz IS NULL OR dtstart >  $3)
                  AND ($4::timestamptz IS NULL OR dtstart <  $4)
                ORDER BY dtstart ASC, id ASC
                LIMIT $5"#,
        )
        .bind(ctx.tenant_id)
        .bind(cal_id)
        .bind(effective_after)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?
    } else {
        sqlx::query_as::<_, Event>(
            r#"SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                      description, location, dtstart, dtend, rrule, status,
                      class, transp, sequence, organizer_email, created_at, updated_at
                 FROM calendar_events
                WHERE tenant_id    = $1
                  AND calendar_id  = $2
                  AND dtstart IS NOT NULL
                  AND ($3::timestamptz IS NULL OR dtstart >= $3)
                  AND ($4::timestamptz IS NULL OR dtstart <  $4)
                ORDER BY dtstart ASC, id ASC
                LIMIT $5"#,
        )
        .bind(ctx.tenant_id)
        .bind(cal_id)
        .bind(effective_after)
        .bind(q.before)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?
    };
    tx.commit().await?;

    let next_cursor = rows.last().and_then(|ev| ev.dtstart);
    let has_more    = rows.len() as i64 == limit;

    let events: Vec<serde_json::Value> = rows.iter().map(|ev| serde_json::json!({
        "id":      ev.id,
        "uid":     ev.uid,
        "summary": ev.summary,
        "dtstart": ev.dtstart,
        "dtend":   ev.dtend,
        "rrule":   ev.rrule,
    })).collect();

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "count":       events.len(),
        "events":      events,
        "next_cursor": next_cursor,
        "has_more":    has_more,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeExportQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// Output format: "ics" (default). Reserved for future formats (e.g. "json").
    format: Option<String>,
}

/// GET /api/v1/calendars/:cal_id/events-by-range/export?after=&before=&format=ics
///
/// Exports events whose `dtstart ∈ [after, before)` as a VCALENDAR ICS file.
/// Complementary to `GET /export.ics` (full calendar) — this targets a date range.
/// `after` and `before` are RFC3339. Both optional; omitting both exports all events
/// (same as full-calendar export). `format=ics` is the only supported value.
/// Returns `text/calendar; charset=utf-8` with `Content-Disposition: attachment`.
/// Sprint #612.
async fn events_by_range_export(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeExportQuery>,
) -> Result<Response> {
    use crate::domain::ical;

    let fmt = q.format.as_deref().unwrap_or("ics");
    if fmt != "ics" {
        return Err(CalendarError::BadRequest("format must be 'ics'".into()));
    }

    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let events: Vec<Event> = sqlx::query_as::<_, Event>(
        r#"SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                  description, location, dtstart, dtend, rrule, status,
                  class, transp, sequence, organizer_email, created_at, updated_at
             FROM calendar_events
            WHERE tenant_id   = $1
              AND calendar_id = $2
              AND dtstart IS NOT NULL
              AND ($3::timestamptz IS NULL OR dtstart >= $3)
              AND ($4::timestamptz IS NULL OR dtstart <  $4)
            ORDER BY dtstart ASC, id ASC"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.after)
    .bind(q.before)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let blocks: Vec<String> = events.iter()
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
        HeaderValue::from_static("attachment; filename=\"events.ics\""),
    );
    Ok(resp)
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeAttendeesQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

/// GET /api/v1/calendars/:cal_id/events-by-range/attendees?after=&before=
///
/// Returns the union of unique attendees across all events whose `dtstart ∈ [after, before)`.
/// Parses `ATTENDEE` lines from each event's `ical_raw` using `itip::parse_attendees`.
/// De-duplicates by email (case-insensitive). Response includes the attendee's latest
/// known CN, role, and partstat (taken from the first occurrence found per email).
///
/// Useful for "who is involved in this week?" calendar summaries and scheduling UIs.
/// Read-only; no auth gate beyond tenant/user ownership of the calendar. Sprint #617.
async fn events_by_range_attendees(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeAttendeesQuery>,
) -> Result<Json<serde_json::Value>> {
    use crate::domain::itip;
    use std::collections::HashMap;

    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let events: Vec<Event> = sqlx::query_as::<_, Event>(
        r#"SELECT id, calendar_id, tenant_id, uid, etag, ical_raw, summary,
                  description, location, dtstart, dtend, rrule, status,
                  class, transp, sequence, organizer_email, created_at, updated_at
             FROM calendar_events
            WHERE tenant_id   = $1
              AND calendar_id = $2
              AND dtstart IS NOT NULL
              AND ($3::timestamptz IS NULL OR dtstart >= $3)
              AND ($4::timestamptz IS NULL OR dtstart <  $4)
            ORDER BY dtstart ASC"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.after)
    .bind(q.before)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    // De-duplicate by email (case-insensitive); first occurrence wins for metadata.
    let mut seen: HashMap<String, serde_json::Value> = HashMap::new();
    for ev in &events {
        for att in itip::parse_attendees(&ev.ical_raw) {
            let key = att.email.to_lowercase();
            seen.entry(key).or_insert_with(|| serde_json::json!({
                "email":    att.email,
                "cn":       att.cn,
                "role":     att.role,
                "partstat": att.partstat,
            }));
        }
    }

    let mut attendees: Vec<serde_json::Value> = seen.into_values().collect();
    // Stable sort by email for deterministic response order.
    attendees.sort_by(|a, b| {
        a["email"].as_str().unwrap_or("").cmp(b["email"].as_str().unwrap_or(""))
    });

    Ok(Json(serde_json::json!({
        "calendar_id":    cal_id,
        "events_scanned": events.len(),
        "count":          attendees.len(),
        "attendees":      attendees,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/organizers?after=&before=
///
/// Returns the union of unique `organizer_email` values across all events whose
/// `dtstart ∈ [after, before)`. Unlike `attendees`, organizer_email is a SQL
/// column (no ical_raw parse needed) — single query, dedup via DISTINCT.
/// Response: `{calendar_id, events_scanned, count, organizers: [email]}`.
/// Sprint #622.
async fn events_by_range_organizers(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeAttendeesQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    // Total events scanned (for the events_scanned field).
    let (events_scanned,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.after)
    .bind(q.before)
    .fetch_one(&mut *tx)
    .await?;

    // Distinct non-null organizer_email values sorted alphabetically.
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT organizer_email \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND organizer_email IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4) \
          ORDER BY organizer_email ASC",
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.after)
    .bind(q.before)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let organizers: Vec<&str> = rows.iter().map(|(e,)| e.as_str()).collect();

    Ok(Json(serde_json::json!({
        "calendar_id":    cal_id,
        "events_scanned": events_scanned,
        "count":          organizers.len(),
        "organizers":     organizers,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/locations?after=&before=
///
/// Retorna union de `location` únicos (não-nulos) no range de dtstart dado.
/// Usa SQL DISTINCT na coluna — sem parse de ical_raw, análogo ao #622 organizers.
/// Response: `{calendar_id, events_scanned, count, locations: [string]}`. Sprint #627.
async fn events_by_range_locations(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeAttendeesQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (events_scanned,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_one(&mut *tx).await?;

    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT location \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND location IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4) \
          ORDER BY location ASC",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let locations: Vec<&str> = rows.iter().map(|(l,)| l.as_str()).collect();

    Ok(Json(serde_json::json!({
        "calendar_id":    cal_id,
        "events_scanned": events_scanned,
        "count":          locations.len(),
        "locations":      locations,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/summaries?after=&before=
///
/// Retorna union de `summary` únicos (não-nulos) no range de dtstart dado.
/// Usa SQL DISTINCT na coluna — análogo ao #627 locations e #622 organizers.
/// Completa o trio subject/location/summary da família events-by-range/*.
/// Response: `{calendar_id, events_scanned, count, summaries: [string]}`. Sprint #632.
async fn events_by_range_summaries(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeAttendeesQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (events_scanned,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_one(&mut *tx).await?;

    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT summary \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND summary IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4) \
          ORDER BY summary ASC",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let summaries: Vec<&str> = rows.iter().map(|(s,)| s.as_str()).collect();

    Ok(Json(serde_json::json!({
        "calendar_id":    cal_id,
        "events_scanned": events_scanned,
        "count":          summaries.len(),
        "summaries":      summaries,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/duration-stats?after=&before=
///
/// Retorna stats de duração dos eventos com `dtstart` ∈ `[after, before)` que
/// também têm `dtend` definido. Usa `EXTRACT(EPOCH FROM (dtend - dtstart))/60`
/// para obter duração em minutos via SQL. Eventos sem `dtstart` ou sem `dtend`
/// são excluídos do cálculo (eventos sem dtend não têm duração mensurável).
/// Response: `{calendar_id, events_with_duration, avg_minutes, min_minutes,
///             max_minutes, total_minutes}`. Sprint #637.
#[derive(Debug, serde::Deserialize)]
struct EventsByRangeDurationStatsQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

async fn events_by_range_duration_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeDurationStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let row: (i64, Option<f64>, Option<f64>, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS events_with_duration, \
            AVG(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS avg_minutes, \
            MIN(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS min_minutes, \
            MAX(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS max_minutes, \
            SUM(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS total_minutes \
         FROM calendar_events \
         WHERE tenant_id   = $1 \
           AND calendar_id = $2 \
           AND dtstart IS NOT NULL \
           AND dtend   IS NOT NULL \
           AND dtend   > dtstart \
           AND ($3::timestamptz IS NULL OR dtstart >= $3) \
           AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    let (events_with_duration, avg_minutes, min_minutes, max_minutes, total_minutes) = row;

    Ok(Json(serde_json::json!({
        "calendar_id":          cal_id,
        "events_with_duration": events_with_duration,
        "avg_minutes":          avg_minutes,
        "min_minutes":          min_minutes,
        "max_minutes":          max_minutes,
        "total_minutes":        total_minutes,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/status-timeline?after=&before=
///
/// Contagem de eventos por status por dia no range `dtstart ∈ [after, before)`.
/// Cada bucket de dia inclui CONFIRMED, TENTATIVE, CANCELLED e OTHER (NULL/vazio/outro).
/// Útil pra dashboards de "evolução de confirmações ao longo do tempo".
/// Response: `{calendar_id, days: [{day, confirmed, tentative, cancelled, other}]}` ASC.
/// Sprint #642.
#[derive(Debug, serde::Deserialize)]
struct EventsByRangeStatusTimelineQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

async fn events_by_range_status_timeline(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeStatusTimelineQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let rows: Vec<(String, i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT \
            to_char(date_trunc('day', dtstart AT TIME ZONE 'UTC'), 'YYYY-MM-DD') AS day, \
            COUNT(*) FILTER (WHERE UPPER(COALESCE(NULLIF(status,''),'OTHER')) = 'CONFIRMED')::BIGINT  AS confirmed, \
            COUNT(*) FILTER (WHERE UPPER(COALESCE(NULLIF(status,''),'OTHER')) = 'TENTATIVE')::BIGINT  AS tentative, \
            COUNT(*) FILTER (WHERE UPPER(COALESCE(NULLIF(status,''),'OTHER')) = 'CANCELLED')::BIGINT  AS cancelled, \
            COUNT(*) FILTER (WHERE UPPER(COALESCE(NULLIF(status,''),'OTHER')) NOT IN \
                             ('CONFIRMED','TENTATIVE','CANCELLED'))::BIGINT                            AS other \
         FROM calendar_events \
         WHERE tenant_id   = $1 \
           AND calendar_id = $2 \
           AND dtstart IS NOT NULL \
           AND ($3::timestamptz IS NULL OR dtstart >= $3) \
           AND ($4::timestamptz IS NULL OR dtstart <  $4) \
         GROUP BY day \
         ORDER BY day ASC",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let days: Vec<serde_json::Value> = rows.into_iter().map(|(day, confirmed, tentative, cancelled, other)| {
        serde_json::json!({
            "day":       day,
            "confirmed": confirmed,
            "tentative": tentative,
            "cancelled": cancelled,
            "other":     other,
        })
    }).collect();

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "days":        days,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/rrule-stats?after=&before=
///
/// Breakdown de recorrências por frequência no range `dtstart ∈ [after, before)`.
/// Retorna `{calendar_id, total_with_rrule, total_without_rrule, by_freq: [{freq, count}]}`
/// onde `freq` ∈ {DAILY, WEEKLY, MONTHLY, YEARLY, OTHER}. A frequência é extraída da
/// coluna `rrule` via `LIKE '%FREQ=DAILY%'` etc. — sem parse in-app; a coluna `rrule`
/// é autoritativa (SET/CLEAR via events-by-range/set-rrule). `OTHER` captura RRULE
/// com FREQ ausente ou valor não-padrão (e.g. SECONDLY, MINUTELY, HOURLY).
/// Sprint #647.
#[derive(Debug, serde::Deserialize)]
struct EventsByRangeRruleStatsQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

async fn events_by_range_rrule_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total_with, total_without, daily, weekly, monthly, yearly, other): (i64, i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*) FILTER (WHERE rrule IS NOT NULL)::BIGINT                            AS with_rrule, \
                COUNT(*) FILTER (WHERE rrule IS NULL)::BIGINT                                AS without_rrule, \
                COUNT(*) FILTER (WHERE rrule LIKE '%FREQ=DAILY%')::BIGINT                   AS daily, \
                COUNT(*) FILTER (WHERE rrule LIKE '%FREQ=WEEKLY%')::BIGINT                  AS weekly, \
                COUNT(*) FILTER (WHERE rrule LIKE '%FREQ=MONTHLY%')::BIGINT                 AS monthly, \
                COUNT(*) FILTER (WHERE rrule LIKE '%FREQ=YEARLY%')::BIGINT                  AS yearly, \
                COUNT(*) FILTER (WHERE rrule IS NOT NULL \
                                   AND rrule NOT LIKE '%FREQ=DAILY%' \
                                   AND rrule NOT LIKE '%FREQ=WEEKLY%' \
                                   AND rrule NOT LIKE '%FREQ=MONTHLY%' \
                                   AND rrule NOT LIKE '%FREQ=YEARLY%')::BIGINT              AS other \
             FROM calendar_events \
             WHERE tenant_id   = $1 \
               AND calendar_id = $2 \
               AND ($3::timestamptz IS NULL OR dtstart >= $3) \
               AND ($4::timestamptz IS NULL OR dtstart <  $4)",
        )
        .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    let by_freq = vec![
        serde_json::json!({"freq": "DAILY",   "count": daily}),
        serde_json::json!({"freq": "WEEKLY",  "count": weekly}),
        serde_json::json!({"freq": "MONTHLY", "count": monthly}),
        serde_json::json!({"freq": "YEARLY",  "count": yearly}),
        serde_json::json!({"freq": "OTHER",   "count": other}),
    ];

    Ok(Json(serde_json::json!({
        "calendar_id":      cal_id,
        "total_with_rrule": total_with,
        "total_without_rrule": total_without,
        "by_freq":          by_freq,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/class-stats?after=&before=
///
/// Contagem de eventos por CLASS (PUBLIC/PRIVATE/CONFIDENTIAL/null) no range
/// `dtstart ∈ [after, before)`. Paralelo direto de rrule-stats (#647) mas
/// para a coluna `class` introduzida no #555. Retorna
/// `{calendar_id, total, public, private, confidential, unset}` onde `unset`
/// conta eventos com `class IS NULL` (RFC 5545: null → PUBLIC por default).
/// Sprint #652.
async fn events_by_range_class_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total, public, private, confidential, unset): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT                                                   AS total, \
                COUNT(*) FILTER (WHERE class = 'PUBLIC')::BIGINT                  AS public, \
                COUNT(*) FILTER (WHERE class = 'PRIVATE')::BIGINT                 AS private, \
                COUNT(*) FILTER (WHERE class = 'CONFIDENTIAL')::BIGINT            AS confidential, \
                COUNT(*) FILTER (WHERE class IS NULL)::BIGINT                     AS unset \
             FROM calendar_events \
             WHERE tenant_id   = $1 \
               AND calendar_id = $2 \
               AND ($3::timestamptz IS NULL OR dtstart >= $3) \
               AND ($4::timestamptz IS NULL OR dtstart <  $4)",
        )
        .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "calendar_id":    cal_id,
        "total":          total,
        "public":         public,
        "private":        private,
        "confidential":   confidential,
        "unset":          unset,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/transp-stats?after=&before=
///
/// Contagem por TRANSP (OPAQUE/TRANSPARENT/unset) no range `dtstart ∈ [after, before)`.
/// Análogo a class-stats (#652) para a coluna `transp` introduzida no #556.
/// Retorna `{calendar_id, total, opaque, transparent, unset}` onde `unset` conta
/// eventos com `transp IS NULL` (RFC 5545: null → OPAQUE, bloqueia free/busy).
/// Sprint #655.
async fn events_by_range_transp_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total, opaque, transparent, unset): (i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT                                                   AS total, \
                COUNT(*) FILTER (WHERE transp = 'OPAQUE')::BIGINT                 AS opaque, \
                COUNT(*) FILTER (WHERE transp = 'TRANSPARENT')::BIGINT            AS transparent, \
                COUNT(*) FILTER (WHERE transp IS NULL)::BIGINT                    AS unset \
             FROM calendar_events \
             WHERE tenant_id   = $1 \
               AND calendar_id = $2 \
               AND ($3::timestamptz IS NULL OR dtstart >= $3) \
               AND ($4::timestamptz IS NULL OR dtstart <  $4)",
        )
        .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "calendar_id":  cal_id,
        "total":        total,
        "opaque":       opaque,
        "transparent":  transparent,
        "unset":        unset,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/attendee-count-stats?after=&before=
///
/// Para cada evento em `dtstart ∈ [after, before)` conta ATTENDEEs via
/// `itip::parse_attendees` (parse in-app do ical_raw). Calcula:
/// `avg_attendees` (f64, 0.0 se nenhum evento), `max_attendees`, e
/// `events_with_attendees` / `events_without_attendees`. Sprint #660.
async fn events_by_range_attendee_count_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    use crate::domain::itip;

    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    // Fetch only ical_raw — the only column needed for attendee parsing.
    let raws: Vec<(String,)> = sqlx::query_as(
        "SELECT ical_raw \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.after)
    .bind(q.before)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let counts: Vec<usize> = raws.iter()
        .map(|(raw,)| itip::parse_attendees(raw).len())
        .collect();

    let total_events = counts.len();
    let events_with    = counts.iter().filter(|&&c| c > 0).count() as i64;
    let events_without = (total_events as i64) - events_with;
    let max_attendees  = counts.iter().copied().max().unwrap_or(0) as i64;
    let avg_attendees: f64 = if total_events == 0 {
        0.0
    } else {
        counts.iter().sum::<usize>() as f64 / total_events as f64
    };

    Ok(Json(serde_json::json!({
        "calendar_id":            cal_id,
        "total_events":           total_events,
        "avg_attendees":          avg_attendees,
        "max_attendees":          max_attendees,
        "events_with_attendees":  events_with,
        "events_without_attendees": events_without,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/location-stats?after=&before=&limit=N
///
/// Retorna `{calendar_id, with_location, without_location, top_locations:[{location,count}]}`
/// para eventos em `dtstart ∈ [after, before)`. Uma query única com COUNT FILTER + subquery
/// GROUP BY para top-N locations (limit default 20, max 200). Sprint #665.
async fn events_by_range_location_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (with_location, without_location): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE location IS NOT NULL)::BIGINT, \
            COUNT(*) FILTER (WHERE location IS NULL)::BIGINT \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_one(&mut *tx).await?;

    let top_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT location, COUNT(*)::BIGINT AS cnt \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND location IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4) \
          GROUP BY location \
          ORDER BY cnt DESC, location ASC \
          LIMIT 20",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let top_locations: Vec<serde_json::Value> = top_rows.into_iter()
        .map(|(location, count)| serde_json::json!({"location": location, "count": count}))
        .collect();

    Ok(Json(serde_json::json!({
        "calendar_id":      cal_id,
        "with_location":    with_location,
        "without_location": without_location,
        "top_locations":    top_locations,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/organizer-stats?after=&before=
///
/// Retorna `{calendar_id, with_organizer, without_organizer, top_organizers:[{organizer,count}]}`
/// para eventos em `dtstart ∈ [after, before)`. COUNT FILTER with/without + GROUP BY
/// organizer_email top-20. Análogo a location-stats (#665) mas para `organizer_email`.
/// Sprint #670.
async fn events_by_range_organizer_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (with_organizer, without_organizer): (i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*) FILTER (WHERE organizer_email IS NOT NULL)::BIGINT, \
            COUNT(*) FILTER (WHERE organizer_email IS NULL)::BIGINT \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_one(&mut *tx).await?;

    let top_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT organizer_email, COUNT(*)::BIGINT AS cnt \
           FROM calendar_events \
          WHERE tenant_id   = $1 \
            AND calendar_id = $2 \
            AND dtstart IS NOT NULL \
            AND organizer_email IS NOT NULL \
            AND ($3::timestamptz IS NULL OR dtstart >= $3) \
            AND ($4::timestamptz IS NULL OR dtstart <  $4) \
          GROUP BY organizer_email \
          ORDER BY cnt DESC, organizer_email ASC \
          LIMIT 20",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_all(&mut *tx).await?;
    tx.commit().await?;

    let top_organizers: Vec<serde_json::Value> = top_rows.into_iter()
        .map(|(organizer, count)| serde_json::json!({"organizer": organizer, "count": count}))
        .collect();

    Ok(Json(serde_json::json!({
        "calendar_id":       cal_id,
        "with_organizer":    with_organizer,
        "without_organizer": without_organizer,
        "top_organizers":    top_organizers,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/status-stats?after=&before=
///
/// COUNT FILTER por `status` (CONFIRMED/TENTATIVE/CANCELLED/other/unset) no range
/// `dtstart ∈ [after, before)`. Análogo a class-stats (#652) para a coluna `status`.
/// `other` = valores não-padrão RFC 5545; `unset` = IS NULL. Sprint #675.
async fn events_by_range_status_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total, confirmed, tentative, cancelled, other, unset): (i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT                                                          AS total, \
                COUNT(*) FILTER (WHERE status = 'CONFIRMED')::BIGINT                     AS confirmed, \
                COUNT(*) FILTER (WHERE status = 'TENTATIVE')::BIGINT                     AS tentative, \
                COUNT(*) FILTER (WHERE status = 'CANCELLED')::BIGINT                     AS cancelled, \
                COUNT(*) FILTER (WHERE status IS NOT NULL \
                                   AND status NOT IN ('CONFIRMED','TENTATIVE','CANCELLED'))::BIGINT \
                                                                                         AS other, \
                COUNT(*) FILTER (WHERE status IS NULL)::BIGINT                           AS unset \
             FROM calendar_events \
             WHERE tenant_id   = $1 \
               AND calendar_id = $2 \
               AND dtstart IS NOT NULL \
               AND ($3::timestamptz IS NULL OR dtstart >= $3) \
               AND ($4::timestamptz IS NULL OR dtstart <  $4)",
        )
        .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "total":       total,
        "confirmed":   confirmed,
        "tentative":   tentative,
        "cancelled":   cancelled,
        "other":       other,
        "unset":       unset,
    })))
}

/// GET /api/v1/calendars/:cal_id/events/class-distribution — breakdown CLASS sem filtro temporal.
///
/// Conta todos os eventos do calendário por CLASS (PUBLIC/PRIVATE/CONFIDENTIAL/unset).
/// Rollup total — sem `after`/`before`. Complementa `events-by-range/class-stats` (#652)
/// com visão acumulada sem escopo temporal. Sprint #690.
async fn events_class_distribution(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total, public, private, confidential, unset): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT                                        AS total, \
                COUNT(*) FILTER (WHERE class = 'PUBLIC')::BIGINT       AS public, \
                COUNT(*) FILTER (WHERE class = 'PRIVATE')::BIGINT      AS private, \
                COUNT(*) FILTER (WHERE class = 'CONFIDENTIAL')::BIGINT AS confidential, \
                COUNT(*) FILTER (WHERE class IS NULL)::BIGINT          AS unset \
             FROM calendar_events \
             WHERE tenant_id = $1 AND calendar_id = $2",
        )
        .bind(ctx.tenant_id).bind(cal_id)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "calendar_id":  cal_id,
        "total":        total,
        "public":       public,
        "private":      private,
        "confidential": confidential,
        "unset":        unset,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/duration-distribution?after=&before=
///
/// Histograma de duração para eventos com dtstart ∈ [after, before) e dtend definido.
/// Buckets: <1h / 1-4h / 4-8h / 1d (8h-24h) / >1d (>=24h). Eventos sem dtend excluídos.
/// Retorna `{calendar_id,total,buckets:[{range,count}]}`. Sprint #699.
async fn events_by_range_duration_distribution(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeDurationStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total, lt1h, h1_4, h4_8, h8_24, gt24h): (i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT AS total, \
                COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (dtend - dtstart)) < 3600)::BIGINT           AS lt1h, \
                COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (dtend - dtstart)) >= 3600   AND EXTRACT(EPOCH FROM (dtend - dtstart)) < 14400)::BIGINT  AS h1_4, \
                COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (dtend - dtstart)) >= 14400  AND EXTRACT(EPOCH FROM (dtend - dtstart)) < 28800)::BIGINT  AS h4_8, \
                COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (dtend - dtstart)) >= 28800  AND EXTRACT(EPOCH FROM (dtend - dtstart)) < 86400)::BIGINT  AS h8_24, \
                COUNT(*) FILTER (WHERE EXTRACT(EPOCH FROM (dtend - dtstart)) >= 86400)::BIGINT         AS gt24h \
             FROM calendar_events \
             WHERE tenant_id   = $1 \
               AND calendar_id = $2 \
               AND dtstart IS NOT NULL \
               AND dtend   IS NOT NULL \
               AND dtend   > dtstart \
               AND ($3::timestamptz IS NULL OR dtstart >= $3) \
               AND ($4::timestamptz IS NULL OR dtstart <  $4)",
        )
        .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "total":       total,
        "buckets": [
            {"range": "<1h",   "count": lt1h},
            {"range": "1-4h",  "count": h1_4},
            {"range": "4-8h",  "count": h4_8},
            {"range": "8h-1d", "count": h8_24},
            {"range": ">1d",   "count": gt24h},
        ],
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/all-day-stats?after=&before=
///
/// Classifica eventos com dtstart ∈ [after, before) em "all-day" vs "timed".
/// All-day = `dtend IS NOT NULL AND dtend::date = dtstart::date + 1` OU `dtend IS NULL AND dtstart::time = '00:00:00'`.
/// Heurística prática: evento sem hora (dtstart truncado ao dia inteiro).
/// Retorna `{calendar_id,total,all_day,timed}`. Sprint #714.
async fn events_by_range_all_day_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total, all_day, timed): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS total, \
            COUNT(*) FILTER ( \
                WHERE (dtstart AT TIME ZONE 'UTC')::time = '00:00:00' \
                  AND (dtend IS NULL OR (dtend AT TIME ZONE 'UTC')::time = '00:00:00') \
            )::BIGINT AS all_day, \
            COUNT(*) FILTER ( \
                WHERE (dtstart AT TIME ZONE 'UTC')::time <> '00:00:00' \
                   OR (dtend IS NOT NULL AND (dtend AT TIME ZONE 'UTC')::time <> '00:00:00') \
            )::BIGINT AS timed \
         FROM calendar_events \
         WHERE tenant_id   = $1 \
           AND calendar_id = $2 \
           AND dtstart IS NOT NULL \
           AND ($3::timestamptz IS NULL OR dtstart >= $3) \
           AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "total":       total,
        "all_day":     all_day,
        "timed":       timed,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/recurrence-duration-stats?after=&before=
///
/// avg/min/max/total duration em minutos — apenas eventos com rrule definido (recorrentes)
/// e dtend > dtstart. Complementa `duration-stats` (#637) com foco em eventos recorrentes.
/// Retorna `{calendar_id,recurrent_with_duration,avg_minutes,min_minutes,max_minutes,total_minutes}`. Sprint #704.
async fn events_by_range_recurrence_duration_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let row: (i64, Option<f64>, Option<f64>, Option<f64>, Option<f64>) = sqlx::query_as(
        "SELECT \
            COUNT(*)::BIGINT AS recurrent_with_duration, \
            AVG(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS avg_minutes, \
            MIN(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS min_minutes, \
            MAX(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS max_minutes, \
            SUM(EXTRACT(EPOCH FROM (dtend - dtstart)) / 60.0) AS total_minutes \
         FROM calendar_events \
         WHERE tenant_id   = $1 \
           AND calendar_id = $2 \
           AND dtstart IS NOT NULL \
           AND dtend   IS NOT NULL \
           AND dtend   > dtstart \
           AND rrule IS NOT NULL AND rrule <> '' \
           AND ($3::timestamptz IS NULL OR dtstart >= $3) \
           AND ($4::timestamptz IS NULL OR dtstart <  $4)",
    )
    .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
    .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    let (recurrent_with_duration, avg_minutes, min_minutes, max_minutes, total_minutes) = row;
    Ok(Json(serde_json::json!({
        "calendar_id":             cal_id,
        "recurrent_with_duration": recurrent_with_duration,
        "avg_minutes":             avg_minutes,
        "min_minutes":             min_minutes,
        "max_minutes":             max_minutes,
        "total_minutes":           total_minutes,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-by-range/priority-stats?after=&before=
///
/// Classifica eventos por PRIORITY (RFC 5545): 0=undefined, 1-4=high, 5=medium, 6-9=low.
/// Retorna `{calendar_id,total,high,medium,low,undefined}`. Sprint #685.
async fn events_by_range_priority_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeRruleStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let (total, high, medium, low, undefined): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT \
                COUNT(*)::BIGINT                                                      AS total, \
                COUNT(*) FILTER (WHERE priority >= 1 AND priority <= 4)::BIGINT      AS high, \
                COUNT(*) FILTER (WHERE priority = 5)::BIGINT                          AS medium, \
                COUNT(*) FILTER (WHERE priority >= 6 AND priority <= 9)::BIGINT      AS low, \
                COUNT(*) FILTER (WHERE priority IS NULL OR priority = 0)::BIGINT     AS undefined \
             FROM calendar_events \
             WHERE tenant_id   = $1 \
               AND calendar_id = $2 \
               AND dtstart IS NOT NULL \
               AND ($3::timestamptz IS NULL OR dtstart >= $3) \
               AND ($4::timestamptz IS NULL OR dtstart <  $4)",
        )
        .bind(ctx.tenant_id).bind(cal_id).bind(q.after).bind(q.before)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "total":       total,
        "high":        high,
        "medium":      medium,
        "low":         low,
        "undefined":   undefined,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeStatsQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:    Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before:   Option<OffsetDateTime>,
    /// Temporal breakdown granularity: "day", "week", or "month".
    /// When present, response includes `by_period: [{period, count}]`.
    /// Sprint #609.
    group_by: Option<String>,
}

/// GET /api/v1/calendars/:cal_id/events-by-range/stats?after=&before= —
/// agregados puros do universo `dtstart ∈ [after, before)` no calendário
/// (sprint #545, dual stats do `events-by-range` #544 — paralelo
/// filosófico do `events-recurrence-stats` #464 mas escopado ao mesmo
/// critério temporal half-open do #544/#457). Mesmo critério
/// (`dtstart >= after AND dtstart < before`, eventos sem `dtstart`
/// EXCLUÍDOS) — ambos os bounds opcionais (sem after = sem lower; sem
/// before = sem upper; sem nenhum = stats sobre todo o calendário com
/// `dtstart` definido). Single COUNT FILTER query agrega
/// `total/with_rrule/without_rrule/with_dtend/without_dtend` + breakdown
/// `by_status` em `{CONFIRMED, TENTATIVE, CANCELLED, OTHER}` —
/// status `NULL` ou string vazia agrupada como `OTHER` (fallback,
/// RFC 5545 não exige STATUS); `total` exclui eventos sem `dtstart`
/// (mesmo universo do #544 list, NÃO universo do `events-recurrence-stats`
/// #464 que agrega TUDO incluindo sem-dtstart). Diferença chave vs
/// `events-by-range` #544: aqui não há `limit` nem retorno de lista —
/// agrega no SQL via `COUNT FILTER` numa única round-trip, escalável
/// pra calendários gigantes onde listar 10k eventos seria caro mas
/// stats agregadas custam ~ms. UI dashboard usa pra responder "quantos
/// eventos nesta janela são confirmed vs cancelled?", "quantos têm
/// rrule?" sem precisar paginar a lista do #544. Read-only, NÃO requer
/// WRITE+. `after >= before` → 400 (mesma validação do #544/#457).
/// Path com `/stats` em sub-route do `events-by-range` consolida o
/// pattern "family path com sub-stats" (cf. `events/:id/exdates/stats`,
/// `events/:id/overrides/stats`). Hierárquico ao mesmo nível do
/// `events-by-range` #544 (master events), distinto da família
/// `overrides-by-range/*` (RECURRENCE-ID overrides) — preserva
/// dualidade master vs override consolidada em #543/#544.
async fn events_by_range_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;

    let row: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
              COUNT(*) FILTER (WHERE dtstart IS NOT NULL)                                AS total,
              COUNT(*) FILTER (WHERE dtstart IS NOT NULL
                                AND rrule IS NOT NULL AND rrule <> '')                   AS with_rrule,
              COUNT(*) FILTER (WHERE dtstart IS NOT NULL
                                AND (rrule IS NULL OR rrule = ''))                       AS without_rrule,
              COUNT(*) FILTER (WHERE dtstart IS NOT NULL AND dtend IS NOT NULL)          AS with_dtend,
              COUNT(*) FILTER (WHERE dtstart IS NOT NULL AND dtend IS NULL)              AS without_dtend
            FROM calendar_events
           WHERE tenant_id   = $1
             AND calendar_id = $2
             AND ($3::timestamptz IS NULL OR dtstart >= $3)
             AND ($4::timestamptz IS NULL OR dtstart <  $4)"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.after)
    .bind(q.before)
    .fetch_one(pool)
    .await?;
    let (total, with_rrule, without_rrule, with_dtend, without_dtend) = row;

    let status_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        r#"SELECT
              UPPER(COALESCE(NULLIF(status, ''), 'OTHER')) AS s,
              COUNT(*) AS c
            FROM calendar_events
           WHERE tenant_id   = $1
             AND calendar_id = $2
             AND dtstart IS NOT NULL
             AND ($3::timestamptz IS NULL OR dtstart >= $3)
             AND ($4::timestamptz IS NULL OR dtstart <  $4)
           GROUP BY s
           ORDER BY c DESC, s ASC"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.after)
    .bind(q.before)
    .fetch_all(pool)
    .await?;

    let mut by_status = serde_json::Map::new();
    for (s, c) in status_rows {
        let key = s.unwrap_or_else(|| "OTHER".into());
        let bucket = match key.as_str() {
            "CONFIRMED" | "TENTATIVE" | "CANCELLED" => key,
            _ => "OTHER".into(),
        };
        let entry = by_status.entry(bucket).or_insert(serde_json::json!(0));
        let prev = entry.as_i64().unwrap_or(0);
        *entry = serde_json::json!(prev + c);
    }

    // Optional temporal breakdown via group_by=day|week|month. Sprint #609.
    let by_period = if let Some(granularity) = &q.group_by {
        let trunc = match granularity.as_str() {
            "day"   => "day",
            "week"  => "week",
            "month" => "month",
            other   => return Err(CalendarError::BadRequest(
                format!("group_by must be 'day', 'week', or 'month'; got '{other}'")
            )),
        };
        let period_rows: Vec<(OffsetDateTime, i64)> = sqlx::query_as(
            &format!(
                r#"SELECT DATE_TRUNC('{trunc}', dtstart AT TIME ZONE 'UTC') AS period,
                          COUNT(*) AS cnt
                     FROM calendar_events
                    WHERE tenant_id   = $1
                      AND calendar_id = $2
                      AND dtstart IS NOT NULL
                      AND ($3::timestamptz IS NULL OR dtstart >= $3)
                      AND ($4::timestamptz IS NULL OR dtstart <  $4)
                    GROUP BY period
                    ORDER BY period ASC"#
            )
        )
        .bind(ctx.tenant_id)
        .bind(cal_id)
        .bind(q.after)
        .bind(q.before)
        .fetch_all(pool)
        .await?;

        let buckets: Vec<serde_json::Value> = period_rows.iter().map(|(dt, cnt)| {
            serde_json::json!({"period": dt, "count": cnt})
        }).collect();
        Some(buckets)
    } else {
        None
    };

    let mut resp = serde_json::json!({
        "calendar_id":    cal_id,
        "total":          total,
        "with_rrule":     with_rrule,
        "without_rrule":  without_rrule,
        "with_dtend":     with_dtend,
        "without_dtend":  without_dtend,
        "by_status":      by_status,
    });
    if let Some(bp) = by_period {
        resp["by_period"]    = serde_json::json!(bp);
        resp["group_by"]     = serde_json::json!(q.group_by);
    }
    Ok(Json(resp))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeMoveQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    dst:    Uuid,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/move?after=&before=&dst=
/// — bulk-move dos eventos cujo `dtstart` ∈ `[after, before)` no calendar
/// `cal_id` (origem) para o calendar `dst` (destino, mesmo tenant) (sprint
/// #546, primeira mutação real da família `events-by-range/*` depois das
/// foundations read-only #544 e #545; complementa o trio com o
/// `events-bulk-delete` #457 que opera no MESMO universo half-open mas
/// destrutivo). Reusa exatamente o critério temporal `dtstart ∈ [after,
/// before)` strict half-open do #544 (eventos sem `dtstart` EXCLUÍDOS —
/// preserva consistência de universo dentro da família). `dst` é
/// obrigatório como query param (não no body porque PATCH single-field
/// + nenhum outro payload — body-less é mais ergonômico via curl/UI).
/// Mover pra `dst == cal_id` é no-op silencioso (não 400) — UPDATE
/// match'a 0 linhas porque `WHERE calendar_id = $src` exclui o destino
/// quando coincidem; semantics "mover pro mesmo lugar não fez nada"
/// é honesta e evita validação extra. Requer WRITE+ em AMBOS calendars
/// (origem `cal_id` E destino `dst`) — assert duplo `assert_can_write`
/// pra cada lado, gate idêntico ao usado em outros endpoints WRITE+
/// dual-calendar (futuro: copy entre calendars vai usar mesma chain).
/// Single UPDATE no SQL: `etag`/`sequence`/`updated_at` PRESERVADOS no
/// nível de cada evento — semantically não houve mudança no conteúdo
/// iCalendar (calendar_id é metadata CalDAV externa ao VCALENDAR per
/// RFC 5545); UI/clients que cacheiam por ETag NÃO precisam invalidar.
/// Sem publicação de `EventUpdated` por evento — operação em massa
/// não cabe no shape per-event do channel events; consumers que querem
/// detectar movimentação devem assistir métrica/log do endpoint ou
/// re-listar o calendar. Conflito de UID no destino (`(calendar_id,
/// uid)` UNIQUE) → 409 via mapeamento existente do `unique_violation`
/// no error layer — UI pode escolher "merge" ou "skip" se conflitar.
/// `?dry=true` NÃO suportado neste sprint inicial — usuário usa
/// `events-by-range` #544 (lista) ou `/stats` #545 (agregados) pra
/// dry-discovery antes de rodar; flag dry adicionada num sprint futuro
/// se demanda surgir. `after >= before` → 400 (paralelo universal
/// #509/#517/#522/#537/#543/#544/#545). Retorna `{src, dst, moved}`
/// com `moved: u64` count das linhas afetadas.
async fn events_by_range_move(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeMoveQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    assert_can_write(pool, ctx.tenant_id, q.dst, ctx.user_id).await?;

    let moved = EventRepo::new(pool)
        .move_range(ctx.tenant_id, cal_id, q.dst, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "src":   cal_id,
        "dst":   q.dst,
        "moved": moved,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetStatusQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    status: String,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-status?after=&before=&status=
/// — bulk-set do campo `status` em todos os eventos cujo `dtstart` ∈
/// `[after, before)` no calendar `cal_id` (sprint #547, segunda mutação
/// da família `events-by-range/*` depois do bulk-move #546; paralelo
/// direto do #546 mas single-tenant single-calendar — mexe em coluna
/// `status` em vez de `calendar_id`). Aceita os 3 valores canônicos
/// CalDAV per RFC 5545 §3.8.1.11: `CONFIRMED`, `TENTATIVE`, `CANCELLED`
/// — qualquer outro → 400 com lista enumerada. Validação case-sensitive
/// upper (paralelo do bucket de status do #545 que UPPER-normaliza no
/// SQL — aqui exigimos input já normalizado pra evitar surpresa de
/// "tentative" virar "TENTATIVE" silenciosamente). Trade-off explícito
/// documentado no `set_status_range`: `ical_raw` NÃO é re-parseado/
/// reserializado, então a coluna `status` (autoritativa pra GET
/// estruturado da API) fica fresh, mas o `STATUS:` dentro do `ical_raw`
/// (visto via export ICS/download VCAL) fica stale até próximo PUT do
/// cliente — clientes CalDAV que leem o raw verão valor antigo até
/// que o evento seja reescrito; UI/clients que leem via API JSON verão
/// valor novo imediatamente. Single calendar (não dual como #546) →
/// 1 `assert_can_write` apenas. Sem publicação per-event de
/// `EventUpdated` (mesma justificativa do #546: massa). Sem `?dry=true`
/// (foundations #544/#545 cobrem discovery). `after >= before` → 400
/// (paralelo universal). Retorna `{calendar_id, status, updated}` com
/// `updated: u64` count das linhas afetadas. Próximo: bulk-set-rrule
/// (mais complexo, exige re-parsear `ical_raw` no loop) ou cancelar
/// família `events-by-range/*` aqui — set-rrule é o fechamento natural
/// das mutações single-field, mas custa N parses dos raws.
async fn events_by_range_set_status(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetStatusQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    match q.status.as_str() {
        "CONFIRMED" | "TENTATIVE" | "CANCELLED" => {}
        _ => {
            return Err(CalendarError::BadRequest(
                "status must be one of CONFIRMED, TENTATIVE, CANCELLED".into(),
            ));
        }
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_status_range(ctx.tenant_id, cal_id, &q.status, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "status":      q.status,
        "updated":     updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeClearRruleQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/clear-rrule?after=&before=
/// — bulk-clear (`rrule = NULL`) em todos os eventos cujo `dtstart` ∈
/// `[after, before)` no calendar `cal_id` (sprint #548, terceira mutação
/// da família `events-by-range/*` depois de bulk-move #546 e set-status
/// #547; variante mais simples do bulk-set-rrule que viria a seguir —
/// "limpar" é caso unário sem validação de valor, então cabe primeiro;
/// `set-rrule` ficaria pra #549+ e teria que validar a string RRULE
/// antes do UPDATE). Novo método `EventRepo::clear_rrule_range(...)`
/// paralelo direto do `set_status_range` #547 mas sem parâmetro `value`
/// (só seta NULL). Mesmo trade-off filosófico do #547 documentado no
/// método repo: `ical_raw` NÃO é re-parseado, então a coluna `rrule`
/// (autoritativa pra GET estruturado da API e queries SQL como `#464`
/// `events-recurrence-stats` que conta `rrule IS NULL`) fica fresh,
/// mas a propriedade `RRULE:` dentro do `ical_raw` permanece STALE até
/// próximo PUT do cliente; clientes CalDAV puros parseando o raw verão
/// o evento como recorrente até reescrita. Single calendar (1
/// `assert_can_write`). Sem flag `?dry=true` (foundations #544/#545
/// cobrem discovery — UI usa `?with_rrule=true` em /events-by-range
/// pra ver qual se aplicaria). `after >= before` → 400 (paralelo
/// universal). Retorna `{calendar_id, cleared}` com `cleared: u64`
/// count das linhas afetadas — note: o count INCLUI eventos que já
/// tinham `rrule = NULL` antes (UPDATE não filtra por valor prévio,
/// é "afetadas pelo critério de range" não "tiveram mudança real");
/// UI que quer count precisa pode comparar com `by_recurrence.recurring`
/// do #545 antes de chamar.
async fn events_by_range_clear_rrule(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeClearRruleQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let cleared = EventRepo::new(pool)
        .clear_rrule_range(ctx.tenant_id, cal_id, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "cleared":     cleared,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetSummaryQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    summary: String,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-summary?after=&before=&summary=
/// — bulk-set do campo `summary` em todos os eventos cujo `dtstart` ∈
/// `[after, before)` no calendar `cal_id` (sprint #549, quarta mutação
/// da família `events-by-range/*` depois de bulk-move #546, set-status
/// #547 e clear-rrule #548). Validação trivial: `summary.trim()` não vazio
/// → 400 ("summary must not be empty (use update endpoint to clear)").
/// String preservada como-é (sem trim destrutivo, paralelo do `update`
/// regular em event.rs que também passa o valor cru — UI controla
/// whitespace conforme intenção; usuário que quer `"  Reunião  "` literal
/// não é castigado). Limite de tamanho NÃO imposto aqui (DDL `summary`
/// é TEXT sem CHECK; consistência com `update` regular). Mesmo trade-off
/// do #547/#548: coluna `summary` fresh, `SUMMARY:` no `ical_raw` stale
/// até próximo PUT — UI confia na API estruturada, fallback no raw só
/// em export. Single calendar (1 `assert_can_write`). Sem `?dry=true`
/// (foundations #544/#545 cobrem discovery). `after >= before` → 400.
/// Retorna `{calendar_id, summary, updated}` com `updated: u64` count
/// das linhas afetadas — count INCLUI eventos que já tinham o mesmo
/// summary (paralelo do #547/#548, simetria com a família).
async fn events_by_range_set_summary(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetSummaryQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    if q.summary.trim().is_empty() {
        return Err(CalendarError::BadRequest(
            "summary must not be empty".into(),
        ));
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_summary_range(ctx.tenant_id, cal_id, &q.summary, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "summary":     q.summary,
        "updated":     updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetLocationQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    location: Option<String>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-location?after=&before=&location=
/// — bulk-set do campo `location` em todos os eventos cujo `dtstart` ∈
/// `[after, before)` no calendar `cal_id` (sprint #550, quinta mutação
/// da família `events-by-range/*` depois de bulk-move #546, set-status
/// #547, clear-rrule #548 e set-summary #549). Diferente do #549 que
/// rejeitou empty/whitespace summary com 400, aqui empty/whitespace OU
/// `?location` ausente é tratado como CLEAR (NULL na coluna) — RFC 5545
/// §3.8.1.7 LOCATION é opcional em VEVENT e DDL `location` é nullable,
/// portanto "sem local" é estado semanticamente válido (vs summary que
/// é hint visual primário e empty é provavelmente bug de UI). Política
/// fundida em vez de flag `?clear=true` separada porque a única semantics
/// adicional necessária é "limpar", coberta naturalmente pelo valor
/// vazio. Strings não-vazias preservadas como-é (sem trim destrutivo,
/// paralelo do #549). Limite de tamanho NÃO imposto (DDL TEXT sem CHECK,
/// consistência com `update`). Mesmo trade-off do #547/#548/#549: coluna
/// `location` fresh, `LOCATION:` no `ical_raw` stale até próximo PUT.
/// Single calendar (1 `assert_can_write`). `after >= before` → 400.
/// Retorna `{calendar_id, location, updated}` com `location: Option<String>`
/// (null no JSON quando clear) — count INCLUI eventos que já tinham o
/// mesmo location (paralelo do #547/#548/#549).
async fn events_by_range_set_location(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetLocationQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let location: Option<&str> = q.location
        .as_deref()
        .filter(|s| !s.trim().is_empty());

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_location_range(ctx.tenant_id, cal_id, location, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "location":    location,
        "updated":     updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetDescriptionQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    description: Option<String>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-description?after=&before=&description=
/// — bulk-set do campo `description` em todos os eventos cujo `dtstart` ∈
/// `[after, before)` no calendar `cal_id` (sprint #551, sexta mutação da
/// família `events-by-range/*` depois de bulk-move #546, set-status #547,
/// clear-rrule #548, set-summary #549 e set-location #550). Paralelo
/// DIRETO do #550 set-location (não do #549 set-summary) — DDL
/// `description` é nullable e RFC 5545 §3.8.1.5 DESCRIPTION é opcional
/// em VEVENT, portanto empty/whitespace OU `?description` ausente é
/// tratado como CLEAR (NULL na coluna), mesma política do #550. Strings
/// não-vazias preservadas como-é (sem trim destrutivo, paralelo do
/// #549/#550). Limite de tamanho NÃO imposto (DDL TEXT sem CHECK,
/// consistência com `update`). Mesmo trade-off do #547/#548/#549/#550:
/// coluna `description` fresh, `DESCRIPTION:` no `ical_raw` stale até
/// próximo PUT — UI confia na API estruturada. Search FTS #461 fica
/// stale (paralelo do #549/#550 — description costuma ter peso médio
/// no scoring tantivy). Single calendar (1 `assert_can_write`).
/// `after >= before` → 400. Retorna `{calendar_id, description, updated}`
/// com `description: Option<String>` (null no JSON quando clear) — count
/// INCLUI eventos que já tinham a mesma description (paralelo do
/// #547/#548/#549/#550).
async fn events_by_range_set_description(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetDescriptionQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let description: Option<&str> = q.description
        .as_deref()
        .filter(|s| !s.trim().is_empty());

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_description_range(ctx.tenant_id, cal_id, description, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "description": description,
        "updated":     updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetOrganizerEmailQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    organizer_email: Option<String>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-organizer-email?after=&before=&organizer_email=
/// — bulk-set do campo `organizer_email` em todos os eventos cujo `dtstart`
/// ∈ `[after, before)` no calendar `cal_id` (sprint #552, sétima mutação da
/// família `events-by-range/*` depois de bulk-move #546, set-status #547,
/// clear-rrule #548, set-summary #549, set-location #550 e set-description
/// #551). Paralelo DIRETO do #550 set-location / #551 set-description (não
/// do #549 set-summary) — DDL `organizer_email TEXT` é nullable e RFC 5545
/// §3.8.4.3 ORGANIZER é opcional em VEVENT, portanto empty/whitespace OU
/// `?organizer_email` ausente é tratado como CLEAR (NULL na coluna), mesma
/// política do #550/#551. Strings não-vazias preservadas como-é (sem trim
/// destrutivo, paralelo do #549/#550/#551). Validação de formato de email
/// NÃO imposta neste endpoint (consistência com `update` que aceita o que
/// o iCal traz, e com o trade-off do #547-#551 "coluna fresh, raw stale" —
/// validação seria meia-medida porque o ORGANIZER no ical_raw permanece
/// stale). DIFERENÇA SEMÂNTICA vs #549/#550/#551 (documentada no método
/// repo): organizer é metadata de PROPRIEDADE/REMETENTE iTIP (#491), não
/// TEXT-livre descritivo — mudar em massa pode reescrever "quem convidou"
/// pra eventos já comunicados externamente, mas iTIP outbound NÃO é
/// re-disparado automaticamente (paralelo do trade-off `ical_raw stale`
/// no eixo de protocolo externo). UI/admin é responsável por entender que
/// não há re-envio de REQUEST/CANCEL pós bulk-set. Mesmo trade-off interno
/// do #547-#551: coluna `organizer_email` fresh, `ORGANIZER:mailto:...`
/// no `ical_raw` stale até próximo PUT — UI confia na API estruturada.
/// Search FTS #461 NÃO afetada (organizer_email NÃO é indexado no
/// tantivy — diferente do #549/#551). Single calendar (1
/// `assert_can_write`). `after >= before` → 400. Retorna `{calendar_id,
/// organizer_email, updated}` com `organizer_email: Option<String>` (null
/// no JSON quando clear) — count INCLUI eventos que já tinham o mesmo
/// organizer (paralelo do #547-#551).
async fn events_by_range_set_organizer_email(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetOrganizerEmailQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let organizer: Option<&str> = q.organizer_email
        .as_deref()
        .filter(|s| !s.trim().is_empty());

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_organizer_email_range(ctx.tenant_id, cal_id, organizer, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id":     cal_id,
        "organizer_email": organizer,
        "updated":         updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetRruleQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    rrule: Option<String>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-rrule?after=&before=&rrule=
/// — bulk-set do campo `rrule` em todos os eventos cujo `dtstart` ∈
/// `[after, before)` no calendar `cal_id` (sprint #553, oitava mutação da
/// família `events-by-range/*` depois de bulk-move #546, set-status #547,
/// clear-rrule #548, set-summary #549, set-location #550, set-description
/// #551 e set-organizer-email #552). Complemento NATURAL do clear-rrule
/// #548 — agora a operação dual ("limpar" → "definir/substituir") com
/// VALIDAÇÃO server-side da string RRULE via `domain::rrule::Rrule::parse`
/// antes do UPDATE (rejeita FREQ desconhecida, INTERVAL não-numérico,
/// BYDAY com weekday inválido — paralelo do `EventRepo::update` que confia
/// no `ical::parse_vevent` upstream porque a string RRULE veio de um VEVENT
/// já validado; aqui ela vem direto do query string e precisa de gate). DDL
/// `rrule TEXT` é nullable e RFC 5545 §3.8.5.3 RRULE é opcional em VEVENT,
/// portanto `?rrule` ausente ou empty/whitespace é tratado como CLEAR (NULL
/// na coluna) — equivale semanticamente ao clear-rrule #548 mas via mesmo
/// endpoint (paralelo da política do #550/#551/#552). Strings não-vazias
/// passam por `Rrule::parse` que retorna `None` em syntax/FREQ unsupported
/// → 400 ("rrule failed to parse — unsupported FREQ or invalid syntax").
/// Subset suportado pelo parser interno (FREQ=DAILY|WEEKLY|MONTHLY|YEARLY,
/// INTERVAL, COUNT, UNTIL, BYDAY) é o que faz parte do gate; tokens não-
/// suportados como BYMONTHDAY/BYSETPOS são silently-ignored pelo parser
/// (linha #62-63 do rrule.rs) e portanto ACEITOS pela validação — coluna
/// armazena a string crua mesmo com tokens não suportados (mesma semantics
/// do `update` regular que não rejeita rrule com tokens desconhecidos).
/// CLASSE active-recurrence (insight #3 do sprint #552): mudança em massa
/// re-expande virtualmente as séries — `events-recurrence-stats` #464,
/// `events-recurrence-monthly` #469, `events/:id/instances` #500 e
/// `events-by-range/range-instances` #501 retornam outros conjuntos
/// imediatamente sem reindex explícito (computam on-demand). EXDATEs e
/// overrides com RECURRENCE-ID que não existem na nova expansion ficam
/// órfãos no DB (dormentes mas sem efeito) — UI/admin que quer limpeza
/// pode chamar `clear_exdates` ou listar overrides após o set-rrule.
/// Mesmo trade-off interno do #547-#552: coluna `rrule` fresh, `RRULE:`
/// no `ical_raw` stale até próximo PUT — clientes CalDAV puros parseando
/// o raw verão FREQ antiga. Search FTS #461 NÃO afetada (rrule não é
/// indexado no tantivy). Single calendar (1 `assert_can_write`).
/// `after >= before` → 400. Retorna `{calendar_id, rrule, updated}` com
/// `rrule: Option<String>` (null no JSON quando clear) — count INCLUI
/// eventos que já tinham a mesma rrule (paralelo do #547-#552).
async fn events_by_range_set_rrule(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetRruleQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let rrule: Option<&str> = q.rrule
        .as_deref()
        .filter(|s| !s.trim().is_empty());

    if let Some(s) = rrule {
        if crate::domain::rrule::Rrule::parse(s).is_none() {
            return Err(CalendarError::BadRequest(
                "rrule failed to parse — unsupported FREQ or invalid syntax".into(),
            ));
        }
    }

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_rrule_range(ctx.tenant_id, cal_id, rrule, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "rrule":       rrule,
        "updated":     updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetTextFieldsQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    summary:         Option<String>,
    #[serde(default)]
    location:        Option<String>,
    #[serde(default)]
    description:     Option<String>,
    #[serde(default)]
    organizer_email: Option<String>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-text-fields
/// Aplica em massa até 4 colunas TEXT-livre num único endpoint:
/// `?after=&before=&summary=&location=&description=&organizer_email=`
/// (sprint #554, consolidação dos 4 sprints individuais #549/#550/#551/#552 num
/// único bulk multi-set). Cada campo é tri-state via convenção query-string:
/// param ausente ⇒ preserve (None no Option<String>); param presente com valor
/// não-vazio ⇒ set; param presente com valor vazio (`?summary=`) ⇒ clear (NULL).
/// Detecção "presente vs ausente" feita no handler via marker — axum's
/// `serde_qs`/`Query` não distingue por padrão "summary=" de "summary ausente"
/// (ambos viram `Some("")` ou `None` dependendo da config), por isso aqui
/// usamos política simplificada: `Option<String>::None` (param ausente) ⇒
/// preserve; `Some(s)` ⇒ trim+filter, vazio ⇒ clear; não-vazio ⇒ set. Diferença
/// do #549 que rejeitava summary-vazio com 400: aqui multi-set tem semantics
/// distinta — se UI manda summary vazio dentro de form multi-field, intenção é
/// clear (paralelo do #550/#551/#552). Trade-off: NÃO é possível distinguir
/// "preserve" de "clear" via query string alone — pra "preserve summary E
/// clear location", UI omite `summary` e envia `location=`. Validação universal
/// `after >= before` → 400. Single calendar (1 `assert_can_write`). NO-OP
/// detection: se TODOS os 4 params são None, retorna 0 imediatamente sem
/// tocar DB (paralelo de "early return" do #517/#522/#537 onde range vazio
/// short-circuita). Retorna `{calendar_id, fields_set, fields_cleared, updated}`
/// — `fields_set` lista colunas explicitamente setadas com valor não-vazio,
/// `fields_cleared` lista colunas explicitamente clearedas (ambas Vec<&str>);
/// `updated` count das linhas afetadas (inclui eventos que tinham os MESMOS
/// valores — paralelo da família). Trade-off filosófico do #547-#553:
/// `ical_raw` permanece STALE até próximo PUT/UPDATE; search FTS #461 fica
/// STALE pra summaries/descriptions/locations atualizadas. Rrule INTENCIONAL-
/// MENTE NÃO incluído porque sua mutação é classe active-recurrence (#553) e
/// não passive-display — manter `set-rrule` separado preserva separação de
/// classes.
async fn events_by_range_set_text_fields(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetTextFieldsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    fn split(p: Option<&str>) -> Option<Option<&str>> {
        p.map(|s| {
            let t = s.trim();
            if t.is_empty() { None } else { Some(s) }
        })
    }
    let summary    = split(q.summary.as_deref());
    let location   = split(q.location.as_deref());
    let descr      = split(q.description.as_deref());
    let organizer  = split(q.organizer_email.as_deref());

    let nothing = summary.is_none() && location.is_none()
               && descr.is_none()   && organizer.is_none();

    let mut fields_set:     Vec<&str> = Vec::new();
    let mut fields_cleared: Vec<&str> = Vec::new();
    for (name, p) in [
        ("summary",         summary),
        ("location",        location),
        ("description",     descr),
        ("organizer_email", organizer),
    ] {
        match p {
            Some(Some(_)) => fields_set.push(name),
            Some(None)    => fields_cleared.push(name),
            None          => {}
        }
    }

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated: u64 = if nothing {
        0
    } else {
        EventRepo::new(pool)
            .set_text_fields_range(
                ctx.tenant_id, cal_id,
                summary, location, descr, organizer,
                q.after, q.before,
            )
            .await?
    };

    Ok(Json(serde_json::json!({
        "calendar_id":     cal_id,
        "fields_set":      fields_set,
        "fields_cleared":  fields_cleared,
        "updated":         updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetClassQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    class:  String,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-class?after=&before=&class=
/// — bulk-set do campo `class` (RFC 5545 §3.8.1.3) em todos os eventos cujo
/// `dtstart` ∈ `[after, before)` (sprint #555, paralelo direto do #547
/// set-status mas em coluna nova `class` adicionada na migração
/// 20260805000000_calendar_event_class.sql). Aceita os 3 valores canônicos:
/// `PUBLIC`, `PRIVATE`, `CONFIDENTIAL` — qualquer outro → 400 com lista
/// enumerada. Mesmo trade-off documentado em `set_class_range`: `ical_raw`
/// NÃO é re-parseado, então a coluna `class` (autoritativa pra GET
/// estruturado) fica fresh, mas o `CLASS:` dentro do `ical_raw` (visto via
/// export ICS / download VCAL) fica STALE até próximo PUT do cliente —
/// mesma divergência cross-channel já documentada nos 7 set-* anteriores
/// (#547/#549/#550/#551/#552/#553/#554). Single calendar → 1 `assert_can_write`.
/// `after >= before` → 400. Retorna `{calendar_id, class, updated}`.
async fn events_by_range_set_class(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetClassQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    match q.class.as_str() {
        "PUBLIC" | "PRIVATE" | "CONFIDENTIAL" => {}
        _ => {
            return Err(CalendarError::BadRequest(
                "class must be one of PUBLIC, PRIVATE, CONFIDENTIAL".into(),
            ));
        }
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_class_range(ctx.tenant_id, cal_id, &q.class, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "class":       q.class,
        "updated":     updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetTransparencyQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    transp: String,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-transparency?after=&before=&transp=
/// — bulk-set do campo `transp` (RFC 5545 §3.8.2.7) em todos os eventos cujo
/// `dtstart` ∈ `[after, before)` (sprint #556, paralelo direto do #555 set-class
/// — segunda instância da CLASSE schema-migration; nova migração
/// `20260806000000_calendar_event_transp.sql` adiciona coluna `transp TEXT
/// CHECK (transp IN ('OPAQUE','TRANSPARENT') OR transp IS NULL)`. Aceita 2
/// valores enum: `OPAQUE` (default RFC, evento bloqueia free/busy) ou
/// `TRANSPARENT` (evento NÃO bloqueia). Mesmo trade-off cross-channel: coluna
/// fica fresh, `TRANSP:` dentro do `ical_raw` fica STALE até PUT do cliente.
/// Free/busy lookup #460 NÃO consulta `transp` ainda — sprint futuro deve
/// adicionar filter `WHERE transp IS DISTINCT FROM 'TRANSPARENT'` pra excluir
/// transparentes do bloco; até lá `transp=TRANSPARENT` é meta-data informativa
/// só observável via GET estruturado e export ICS pós próximo PUT.
async fn events_by_range_set_transparency(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetTransparencyQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    match q.transp.as_str() {
        "OPAQUE" | "TRANSPARENT" => {}
        _ => {
            return Err(CalendarError::BadRequest(
                "transp must be one of OPAQUE, TRANSPARENT".into(),
            ));
        }
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let updated = EventRepo::new(pool)
        .set_transparency_range(ctx.tenant_id, cal_id, &q.transp, q.after, q.before)
        .await?;

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "transp":      q.transp,
        "updated":     updated,
    })))
}

/// GET /api/v1/calendars/:cal_id/events-recurrence-stats — particiona eventos
/// do calendário em single (sem rrule) vs recorrente (com rrule), e breakdown
/// dos recorrentes por FREQ (DAILY/WEEKLY/MONTHLY/YEARLY/OTHER) (sprint #464).
/// Útil pra dashboard "quantos eventos da agenda são repetitivos". Usa COUNT
/// FILTER pra particionar numa única query. FREQ extraído via regex via
/// `substring(rrule from 'FREQ=([A-Z]+)')` — RRULE sempre tem FREQ obrigatório
/// per RFC 5545. Retorna `{single, recurring, by_freq: {DAILY, WEEKLY, ...}}`.
/// Path com hífen evita colisão com `events/:id` (lição #427).
async fn events_recurrence_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let (single, recurring): (i64, i64) = sqlx::query_as(
        r#"SELECT
              COUNT(*) FILTER (WHERE rrule IS NULL OR rrule = '') AS single,
              COUNT(*) FILTER (WHERE rrule IS NOT NULL AND rrule <> '') AS recurring
            FROM calendar_events
            WHERE tenant_id = $1 AND calendar_id = $2"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .fetch_one(pool)
    .await?;

    let freq_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
        r#"SELECT UPPER(COALESCE(substring(rrule from 'FREQ=([A-Za-z]+)'), 'OTHER')) AS freq,
                  COUNT(*) AS c
             FROM calendar_events
            WHERE tenant_id   = $1
              AND calendar_id = $2
              AND rrule IS NOT NULL
              AND rrule <> ''
            GROUP BY freq
            ORDER BY c DESC, freq ASC"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .fetch_all(pool)
    .await?;

    let mut by_freq = serde_json::Map::new();
    for (freq, c) in freq_rows {
        let key = freq.unwrap_or_else(|| "OTHER".into());
        by_freq.insert(key, serde_json::json!(c));
    }

    Ok(Json(serde_json::json!({
        "single":    single,
        "recurring": recurring,
        "by_freq":   by_freq,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct RecurrenceMonthlyQuery {
    since:  Option<OffsetDateTime>,
    before: Option<OffsetDateTime>,
}

/// GET /api/v1/calendars/:cal_id/events-recurrence-monthly?since=&before=
/// Histogram temporal: para cada mês de `created_at` retorna `{single, recurring}`
/// (sprint #469, extensão temporal do #464). COUNT FILTER particiona single vs
/// recurring por mês numa única query. Buckets agrupados via
/// `date_trunc('month', created_at)`. Útil pra dashboard "como muda a taxa de
/// eventos repetitivos ao longo do tempo" — ex: detectar se a equipe começou a
/// criar mais reuniões recorrentes. Path com hífen evita colisão com
/// `events/:id` (lição #427). Range opcional via `since`/`before`.
async fn events_recurrence_monthly(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<RecurrenceMonthlyQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    let rows: Vec<(OffsetDateTime, i64, i64)> = sqlx::query_as(
        r#"SELECT date_trunc('month', created_at) AS bucket,
                  COUNT(*) FILTER (WHERE rrule IS NULL OR rrule = '') AS single,
                  COUNT(*) FILTER (WHERE rrule IS NOT NULL AND rrule <> '') AS recurring
             FROM calendar_events
            WHERE tenant_id   = $1
              AND calendar_id = $2
              AND ($3::timestamptz IS NULL OR created_at >= $3)
              AND ($4::timestamptz IS NULL OR created_at <  $4)
            GROUP BY bucket
            ORDER BY bucket ASC"#,
    )
    .bind(ctx.tenant_id)
    .bind(cal_id)
    .bind(q.since)
    .bind(q.before)
    .fetch_all(pool)
    .await?;

    let buckets: Vec<serde_json::Value> = rows.into_iter()
        .map(|(bucket, single, recurring)| {
            let total = single + recurring;
            let rate = if total > 0 { recurring as f64 / total as f64 } else { 0.0 };
            serde_json::json!({
                "month":     bucket,
                "single":    single,
                "recurring": recurring,
                "total":     total,
                "rate":      rate,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "buckets": buckets })))
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

/// PATCH /api/v1/calendars/:cal_id/events/:id — update individual fields without
/// replacing the full iCal body. All fields are optional; absent fields are preserved.
/// `null` clears a nullable field (summary, location, description, status).
/// `dtstart`/`dtend` accept RFC 3339. `status` accepts TENTATIVE|CONFIRMED|CANCELLED.
/// `ical_raw` is left stale until the client does a PUT — consistent with events-by-range/*.
#[derive(Debug, serde::Deserialize)]
struct PatchEventBody {
    summary:     Option<serde_json::Value>,
    location:    Option<serde_json::Value>,
    description: Option<serde_json::Value>,
    dtstart:     Option<serde_json::Value>,
    dtend:       Option<serde_json::Value>,
    status:      Option<serde_json::Value>,
}

async fn patch_event(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<PatchEventBody>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    fn str_field(v: Option<serde_json::Value>) -> Result<Option<Option<String>>> {
        match v {
            None => Ok(None),
            Some(serde_json::Value::Null) => Ok(Some(None)),
            Some(serde_json::Value::String(s)) => Ok(Some(Some(s))),
            _ => Err(CalendarError::BadRequest("field must be a string or null".into())),
        }
    }

    fn ts_field(v: Option<serde_json::Value>) -> Result<Option<Option<OffsetDateTime>>> {
        match v {
            None => Ok(None),
            Some(serde_json::Value::Null) => Ok(Some(None)),
            Some(serde_json::Value::String(s)) => {
                OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
                    .map(|dt| Some(Some(dt)))
                    .map_err(|_| CalendarError::BadRequest("dtstart/dtend must be RFC 3339".into()))
            }
            _ => Err(CalendarError::BadRequest("dtstart/dtend must be a string or null".into())),
        }
    }

    let summary     = str_field(body.summary)?;
    let location    = str_field(body.location)?;
    let description = str_field(body.description)?;
    let status      = str_field(body.status)?;
    let dtstart     = ts_field(body.dtstart)?;
    let dtend       = ts_field(body.dtend)?;

    if let Some(Some(ref s)) = status {
        let s = s.as_str();
        if s != "TENTATIVE" && s != "CONFIRMED" && s != "CANCELLED" {
            return Err(CalendarError::BadRequest(
                "status must be TENTATIVE, CONFIRMED, or CANCELLED".into(),
            ));
        }
    }

    let ev = EventRepo::new(pool)
        .patch_fields(ctx.tenant_id, id, summary, location, description, dtstart, dtend, status)
        .await?;

    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: ev.id,
        summary: ev.summary.clone(), sequence: ev.sequence,
    });

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


/// GET /api/v1/calendars/:cal_id/events/:id/history — log de mudanças de etag/sequence.
///
/// Retorna as últimas N entradas da `calendar_event_history` para o evento,
/// ordenadas da mais recente para a mais antiga. Parâmetro opcional `limit`
/// (default 50, máx 200). Cada entry: `{id, etag, sequence, op, changed_at}`.
/// 404 se o evento não pertence ao tenant/calendar. Sprint #583.
async fn event_history(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, event_id)): Path<(Uuid, Uuid)>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;
    let pool = state.db_or_unavailable()?;

    // Verify event belongs to this tenant (404 guard).
    let ev = EventRepo::new(pool)
        .get(ctx.tenant_id, event_id)
        .await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(event_id));
    }

    let limit = params.limit.unwrap_or(50).min(200) as i64;

    let rows: Vec<(Uuid, String, i32, String, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, etag, sequence, op, changed_at \
           FROM calendar_event_history \
          WHERE tenant_id = $1 AND event_id = $2 \
          ORDER BY changed_at DESC \
          LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(CalendarError::Database)?;

    let entries: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, etag, sequence, op, changed_at)| json!({
            "id":         id,
            "etag":       etag,
            "sequence":   sequence,
            "op":         op,
            "changed_at": changed_at,
        }))
        .collect();

    Ok(Json(json!({
        "event_id":    event_id,
        "calendar_id": cal_id,
        "history":     entries,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct HistoryParams {
    limit: Option<u32>,
}

/// DELETE /api/v1/calendars/:cal_id/events/:id/history — purge do log de histórico.
///
/// Remove todas as entradas de `calendar_event_history` para o evento.
/// Requer OWNER/WRITE/ADMIN no calendar. Retorna `{event_id, calendar_id, deleted}`.
/// 404 se evento não pertence ao tenant/calendar. Sprint #590.
async fn delete_event_history(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, event_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;
    let pool = state.db_or_unavailable()?;

    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool)
        .get(ctx.tenant_id, event_id)
        .await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(event_id));
    }

    let result = sqlx::query(
        "DELETE FROM calendar_event_history \
          WHERE tenant_id = $1 AND event_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .execute(pool)
    .await
    .map_err(CalendarError::Database)?;

    Ok(Json(json!({
        "event_id":    event_id,
        "calendar_id": cal_id,
        "deleted":     result.rows_affected(),
    })))
}

/// GET /api/v1/calendars/:cal_id/events/:id/history/stats — estatísticas do histórico.
///
/// Retorna `{event_id, calendar_id, total, put_count, patch_count, first_at, last_at}`.
/// `first_at`/`last_at` são null se não há entradas (evento nunca sofreu PUT/PATCH rastreado).
/// 404 se evento não pertence ao tenant/calendar. Sprint #594.
async fn event_history_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, event_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;
    let pool = state.db_or_unavailable()?;

    let ev = EventRepo::new(pool)
        .get(ctx.tenant_id, event_id)
        .await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(event_id));
    }

    let row: (i64, i64, i64, Option<OffsetDateTime>, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT COUNT(*) AS total, \
                COUNT(*) FILTER (WHERE op = 'PUT')   AS put_count, \
                COUNT(*) FILTER (WHERE op = 'PATCH') AS patch_count, \
                MIN(changed_at) AS first_at, \
                MAX(changed_at) AS last_at \
           FROM calendar_event_history \
          WHERE tenant_id = $1 AND event_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .fetch_one(pool)
    .await
    .map_err(CalendarError::Database)?;

    let (total, put_count, patch_count, first_at, last_at) = row;

    Ok(Json(json!({
        "event_id":    event_id,
        "calendar_id": cal_id,
        "total":       total,
        "put_count":   put_count,
        "patch_count": patch_count,
        "first_at":    first_at,
        "last_at":     last_at,
    })))
}

/// GET /api/v1/calendars/:cal_id/events/:id/history/:entry_id — entrada individual do histórico.
///
/// Retorna os mesmos campos do GET /history (id, etag, sequence, op, changed_at) para uma
/// única entrada identificada por UUID. 404 se o evento não pertence ao tenant/calendar ou
/// se a entrada não existe nesse evento. Sprint #598.
async fn get_history_entry(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, event_id, entry_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;
    let pool = state.db_or_unavailable()?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, event_id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(event_id));
    }

    let row: Option<(Uuid, String, i32, String, OffsetDateTime)> = sqlx::query_as(
        "SELECT id, etag, sequence, op, changed_at \
           FROM calendar_event_history \
          WHERE tenant_id = $1 AND event_id = $2 AND id = $3",
    )
    .bind(ctx.tenant_id)
    .bind(event_id)
    .bind(entry_id)
    .fetch_optional(pool)
    .await
    .map_err(CalendarError::Database)?;

    let (id, etag, sequence, op, changed_at) = row
        .ok_or(CalendarError::EventNotFound(entry_id))?;

    Ok(Json(json!({
        "id":          id,
        "event_id":    event_id,
        "calendar_id": cal_id,
        "etag":        etag,
        "sequence":    sequence,
        "op":          op,
        "changed_at":  changed_at,
    })))
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

#[derive(Debug, serde::Deserialize)]
struct InstancesParams {
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to:   OffsetDateTime,
}

#[derive(Debug, serde::Serialize)]
struct EventInstance {
    #[serde(with = "time::serde::rfc3339")]
    dtstart: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    dtend:   OffsetDateTime,
}

/// GET /api/v1/calendars/:cal_id/events/:id/instances?from=&to= — expande
/// recorrências de um evento dentro de [from, to) (sprint #478). Usa o
/// `Rrule::expand` (RFC 5545 subset: FREQ, INTERVAL, COUNT, UNTIL, BYDAY) ou
/// fallback `single_instance` quando rrule está vazia/inválida. `from < to`
/// obrigatório. Retorna `{event_id, summary, rrule, count, instances:
/// [{dtstart, dtend}]}` com instâncias clamped pra dentro do range. Útil pra
/// timeline/calendar UI que precisa renderizar série inteira sem precisar
/// re-implementar expander client-side.
async fn events_instances(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
    Query(params): Query<InstancesParams>,
) -> Result<Json<serde_json::Value>> {
    if params.from >= params.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;

    let dtstart = match ev.dtstart {
        Some(s) => s,
        None    => return Ok(Json(serde_json::json!({
            "event_id": ev.id, "summary": ev.summary,
            "rrule": ev.rrule, "count": 0, "instances": [],
        }))),
    };
    let duration = match ev.dtend {
        Some(e) if e > dtstart => e - dtstart,
        _ => time::Duration::ZERO,
    };

    let pairs: Vec<(OffsetDateTime, OffsetDateTime)> = match ev.rrule.as_deref() {
        Some(raw) if !raw.trim().is_empty() => match crate::domain::rrule::Rrule::parse(raw) {
            Some(rule) => rule.expand(dtstart, duration, params.from, params.to),
            None       => crate::domain::rrule::single_instance(dtstart, ev.dtend, params.from, params.to)
                              .into_iter().collect(),
        },
        _ => crate::domain::rrule::single_instance(dtstart, ev.dtend, params.from, params.to)
                 .into_iter().collect(),
    };

    let exdates = parse_exdates(&ev.ical_raw);
    let instances: Vec<EventInstance> = pairs.into_iter()
        .filter(|(s, _)| !exdates.iter().any(|x| x == s))
        .map(|(s, e)| EventInstance { dtstart: s, dtend: e })
        .collect();

    Ok(Json(serde_json::json!({
        "event_id":  ev.id,
        "summary":   ev.summary,
        "rrule":     ev.rrule,
        "count":     instances.len(),
        "instances": instances,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct InstancesBulkParams {
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to:   OffsetDateTime,
    /// Cap por evento expandido (proteção: rrule sem UNTIL+COUNT pode estourar).
    /// Default 500, max 5000 — gates abaixo do array do `Rrule::expand`.
    per_event_cap: Option<usize>,
}

#[derive(Debug, serde::Serialize)]
struct EventInstancesGroup {
    event_id: Uuid,
    summary:  Option<String>,
    rrule:    Option<String>,
    count:    usize,
    instances: Vec<EventInstance>,
}

/// GET /api/v1/calendars/:cal_id/events-instances?from=&to=&per_event_cap=
/// Bulk variant de `/events/:id/instances` (sprint #482, paralelo de #478):
/// expande recorrências de TODOS os eventos do calendário cujo dtstart esteja
/// antes de `to` E (dtend OR dtstart) >= `from` (mesmo filtro temporal de
/// `list`). Retorna `{from, to, total_events, total_instances, events:
/// [{event_id, summary, rrule, count, instances:[{dtstart,dtend}]}]}`. Útil
/// pra timeline/agenda mensal renderizar todas as ocorrências num único call
/// sem N+1. `per_event_cap` (default 500, max 5000) limita explosão por evento
/// caso rrule infinita escape do UNTIL/COUNT — `Rrule::expand` ignora além do
/// range, mas cap defensivo evita memory blow se `to-from` for absurdo. Path
/// com hífen evita colisão com `events/:id` (lição #427/#443/#448).
async fn events_instances_bulk(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(params): Query<InstancesBulkParams>,
) -> Result<Json<serde_json::Value>> {
    if params.from >= params.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }
    let per_cap = params.per_event_cap.unwrap_or(500).min(5000).max(1);

    let pool = state.db_or_unavailable()?;
    let q = crate::domain::EventQuery {
        from: Some(params.from),
        to:   Some(params.to),
        limit: None,
    };
    let events = EventRepo::new(pool).list(ctx.tenant_id, cal_id, &q).await?;

    let mut groups: Vec<EventInstancesGroup> = Vec::with_capacity(events.len());
    let mut total_instances: usize = 0;

    for ev in events {
        let dtstart = match ev.dtstart {
            Some(s) => s,
            None    => continue,
        };
        let duration = match ev.dtend {
            Some(e) if e > dtstart => e - dtstart,
            _ => time::Duration::ZERO,
        };

        let pairs: Vec<(OffsetDateTime, OffsetDateTime)> = match ev.rrule.as_deref() {
            Some(raw) if !raw.trim().is_empty() => match crate::domain::rrule::Rrule::parse(raw) {
                Some(rule) => rule.expand(dtstart, duration, params.from, params.to),
                None       => crate::domain::rrule::single_instance(dtstart, ev.dtend, params.from, params.to)
                                  .into_iter().collect(),
            },
            _ => crate::domain::rrule::single_instance(dtstart, ev.dtend, params.from, params.to)
                     .into_iter().collect(),
        };

        let exdates = parse_exdates(&ev.ical_raw);
        let mut instances: Vec<EventInstance> = pairs.into_iter()
            .filter(|(s, _)| !exdates.iter().any(|x| x == s))
            .take(per_cap)
            .map(|(s, e)| EventInstance { dtstart: s, dtend: e })
            .collect();

        if instances.is_empty() {
            continue;
        }
        total_instances += instances.len();

        // shrink_to_fit pra liberar capacidade extra reservada por take()
        instances.shrink_to_fit();

        groups.push(EventInstancesGroup {
            event_id:  ev.id,
            summary:   ev.summary,
            rrule:     ev.rrule,
            count:     instances.len(),
            instances,
        });
    }

    let from_s = params.from.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let to_s   = params.to.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    Ok(Json(serde_json::json!({
        "from":            from_s,
        "to":              to_s,
        "per_event_cap":   per_cap,
        "total_events":    groups.len(),
        "total_instances": total_instances,
        "events":          groups,
    })))
}

/// Parse linhas `EXDATE:...` do VEVENT (formato UTC `YYYYMMDDTHHMMSSZ`,
/// múltiplos valores separados por vírgula numa mesma linha permitidos).
/// Ignora EXDATE com TZID/parametros (subset MVP — eventos criados pelo
/// próprio backend usam UTC). Retorna timestamps que devem ser excluídos
/// da expansão de instâncias. Usado por `events_instances` /
/// `events_instances_bulk` (sprint #488).
fn parse_exdates(ical_raw: &str) -> Vec<OffsetDateTime> {
    use time::format_description::FormatItem;
    use time::macros::format_description;
    static FMT: &[FormatItem<'static>] = format_description!(
        "[year][month][day]T[hour][minute][second]Z"
    );
    let mut out = Vec::new();
    for line in ical_raw.lines() {
        let trimmed = line.trim_start();
        let upper: String = trimmed.chars().take(7).collect::<String>().to_ascii_uppercase();
        if !upper.starts_with("EXDATE:") {
            continue;
        }
        let value = &trimmed["EXDATE:".len()..];
        for tok in value.split(',') {
            let tok = tok.trim();
            if tok.is_empty() { continue; }
            if let Ok(ts) = OffsetDateTime::parse(tok, &FMT) {
                out.push(ts);
            }
        }
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct CancelInstanceBody {
    #[serde(with = "time::serde::rfc3339")]
    instance: OffsetDateTime,
}

/// POST /api/v1/calendars/:cal_id/events/:id/cancel-instance
/// Cancela 1 ocorrência específica de um evento recorrente sem afetar
/// resto da série (sprint #488). Adiciona linha `EXDATE:<dtstamp UTC>` ao
/// VEVENT — no próximo expand de instâncias, `parse_exdates` filtra essa
/// occurrence. Body: `{instance: "2026-05-15T14:00:00Z"}` (RFC 3339,
/// canonicalizado pra UTC e formato compacto antes de ser inserido).
/// Idempotente: se EXDATE já existir pra esse instante, retorna `{added:
/// false}`. 400 se evento não tem rrule (cancelar uma série não-recorrente
/// = delete o evento). 404 se evento não existe. Requer WRITE+ no
/// calendário (assert_can_write). Retorna `{event_id, instance, added}`.
async fn cancel_event_instance(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Json(body):   Json<CancelInstanceBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }
    if ev.rrule.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
        return Err(CalendarError::BadRequest(
            "event has no rrule — use DELETE to remove single instance".into()
        ));
    }

    let inst_utc = body.instance.to_offset(time::UtcOffset::UTC);
    let inst_str = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        inst_utc.year(),
        u8::from(inst_utc.month()),
        inst_utc.day(),
        inst_utc.hour(),
        inst_utc.minute(),
        inst_utc.second(),
    );

    let already: bool = parse_exdates(&ev.ical_raw)
        .iter()
        .any(|t| t.to_offset(time::UtcOffset::UTC) == inst_utc);
    if already {
        return Ok(Json(serde_json::json!({
            "event_id": ev.id,
            "instance": inst_str,
            "added":    false,
        })));
    }

    if let Some(uid) = extract_uid(&ev.ical_raw) {
        if has_recurrence_id_override(&ev.ical_raw, &uid, &inst_str) {
            return Err(CalendarError::Conflict(
                format!("instance {inst_str} has a RECURRENCE-ID override — \
                         remove via DELETE /overrides/:recurrence_id before cancelling")
            ));
        }
    }

    let new_line = format!("EXDATE:{inst_str}");
    let new_raw = inject_exdate_line(&ev.ical_raw, &new_line);

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id, summary: updated.summary.clone(),
    });

    Ok(Json(serde_json::json!({
        "event_id": ev.id,
        "instance": inst_str,
        "added":    true,
    })))
}

/// Insere `line` antes do END:VEVENT. Mantém line endings do raw original
/// (CRLF se presente, LF caso contrário).
fn inject_exdate_line(raw: &str, line: &str) -> String {
    let crlf = raw.contains("\r\n");
    let eol  = if crlf { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(raw.len() + line.len() + 2);
    let mut injected = false;
    for src_line in raw.split_inclusive('\n') {
        if !injected {
            let probe: String = src_line.trim_end().to_ascii_uppercase();
            if probe == "END:VEVENT" {
                out.push_str(line);
                out.push_str(eol);
                injected = true;
            }
        }
        out.push_str(src_line);
    }
    if !injected {
        out.push_str(line);
        out.push_str(eol);
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct ListExdatesQuery {
    #[serde(default)]
    detail: Option<String>,
    /// `?kind=utc|tzid|date-only|unknown` filtra a lista por classificação
    /// (sprint #516, extensão do #504). Só faz sentido com `detail=full`
    /// porque o modo `summary` já degenera pra UTC-only — `kind=utc` é
    /// no-op aceito; qualquer outro `kind` com `summary` → 400.
    #[serde(default)]
    kind: Option<String>,
    /// `?with_tzid=true|false` (sprint #523, paralelo simétrico do #518 mas
    /// pra EXDATE) filtra por presença de TZID em AND com os outros flags
    /// — `true` exige `info.tzid=Some`, `false` exige `info.tzid=None`.
    /// Independente do `kind` (#516): `kind=tzid` ⊂ `with_tzid=true` (todo
    /// `kind=tzid` tem TZID, mas `with_tzid=true` cobre TZID em formato
    /// `unknown` também — útil pra audit qualitativo). Só faz sentido com
    /// `detail=full`; em `summary` → 400.
    #[serde(default)]
    with_tzid: Option<bool>,
    /// `?with_params=true|false` (sprint #523) filtra por presença de
    /// parâmetros não-TZID na linha EXDATE (ex: `EXDATE;VALUE=DATE:...`).
    /// Mesma semântica de `with_tzid` — `true` exige `info.params=Some`,
    /// `false` exige `info.params=None`. Útil pra audit "quais EXDATEs têm
    /// parametrização não-padrão" (`?with_params=true`). Só faz sentido com
    /// `detail=full`; em `summary` → 400.
    #[serde(default)]
    with_params: Option<bool>,
}

/// GET /api/v1/calendars/:cal_id/events/:id/exdates — lista EXDATEs do
/// VEVENT no formato compacto UTC (`YYYYMMDDTHHMMSSZ`) e RFC3339 paralelo
/// pra UI (sprint #491, inverso de #488). Reusa `parse_exdates` no modo
/// default (`?detail=summary`) retornando `{event_id, count,
/// exdates:[{compact, rfc3339}]}` — só EXDATEs UTC parseáveis.
///
/// `?detail=full` (sprint #511, paralelo simétrico do #503) usa
/// `parse_exdates_rich` cobrindo TAMBÉM linhas com TZID, parametros e
/// formatos não-UTC que `parse_exdates` ignora silenciosamente; cada item
/// ganha `tzid?`, `params?`, `kind` (`"utc"|"tzid"|"date-only"|"unknown"`)
/// e `raw_value` (token original). Útil pra debug/inspeção quando ICS
/// importado tem EXDATEs em formato não-MVP.
///
/// `?kind=utc|tzid|date-only|unknown` (sprint #516) filtra a lista pelo
/// `kind` parseado — útil pra audit "quais EXDATEs estão em formato não
/// suportado pelo expander MVP" (`?detail=full&kind=unknown`) ou "quais
/// estão com TZID que precisa migrar pra UTC" (`?detail=full&kind=tzid`).
/// Só faz sentido com `detail=full`; em `summary`, único valor aceito é
/// `kind=utc` (no-op) — qualquer outro vira 400.
///
/// `?with_tzid=&with_params=` (sprint #523, paralelo simétrico do #518 mas
/// pra EXDATE) filtros booleanos qualitativos em AND com `kind`, aplicados
/// dentro do mesmo closure de filter no full branch. `true` exige campo
/// presente (`Some`), `false` exige ausência (`None`); independentes entre
/// si e do `kind`. Importante: `with_tzid` é ortogonal mas NÃO disjunto de
/// `kind=tzid` — todo `kind=tzid` tem `tzid=Some`, mas `with_tzid=true`
/// também captura items com TZID em formato malformado (`kind=unknown`)
/// que `kind=tzid` exige formato canônico. Útil pra audit qualitativo
/// genérico ("tem TZID em qualquer formato"). Só fazem sentido com
/// `detail=full` — em `summary` → 400.
///
/// Não requer WRITE (read-only). 400 em valor de detail/kind desconhecido.
/// 404 se evento não existe.
async fn list_exdates(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<ListExdatesQuery>,
) -> Result<Json<serde_json::Value>> {
    let full = match q.detail.as_deref() {
        None | Some("") | Some("summary") => false,
        Some("full") => true,
        Some(other) => return Err(CalendarError::BadRequest(
            format!("detail must be 'summary' or 'full', got '{other}'")
        )),
    };
    let kind_filter: Option<&str> = match q.kind.as_deref() {
        None | Some("") => None,
        Some(k @ ("utc" | "tzid" | "date-only" | "unknown")) => Some(k),
        Some(other) => return Err(CalendarError::BadRequest(
            format!("kind must be 'utc', 'tzid', 'date-only' or 'unknown', got '{other}'")
        )),
    };
    if !full && matches!(kind_filter, Some(k) if k != "utc") {
        return Err(CalendarError::BadRequest(
            "kind filter other than 'utc' requires detail=full".into()
        ));
    }
    if !full && (q.with_tzid.is_some() || q.with_params.is_some()) {
        return Err(CalendarError::BadRequest(
            "with_tzid/with_params filters require detail=full".into()
        ));
    }
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;

    let items: Vec<serde_json::Value> = if full {
        parse_exdates_rich(&ev.ical_raw).into_iter().filter(|info| {
            if let Some(k) = kind_filter { if info.kind != k { return false; } }
            if let Some(want) = q.with_tzid   { if info.tzid.is_some()   != want { return false; } }
            if let Some(want) = q.with_params { if info.params.is_some() != want { return false; } }
            true
        }).map(|info| {
            let (compact, rfc) = match info.parsed_utc {
                Some(t) => {
                    let utc = t.to_offset(time::UtcOffset::UTC);
                    let c = format!(
                        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
                        utc.year(), u8::from(utc.month()), utc.day(),
                        utc.hour(), utc.minute(), utc.second(),
                    );
                    let r = utc.format(&time::format_description::well_known::Rfc3339)
                        .unwrap_or_default();
                    (serde_json::Value::String(c), serde_json::Value::String(r))
                }
                None => (serde_json::Value::Null, serde_json::Value::Null),
            };
            let mut item = serde_json::json!({
                "compact":   compact,
                "rfc3339":   rfc,
                "kind":      info.kind,
                "raw_value": info.raw_value,
            });
            if let Some(obj) = item.as_object_mut() {
                obj.insert("tzid".into(),
                    info.tzid.map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null));
                obj.insert("params".into(),
                    info.params.map(serde_json::Value::String)
                        .unwrap_or(serde_json::Value::Null));
            }
            item
        }).collect()
    } else {
        parse_exdates(&ev.ical_raw).iter().map(|t| {
            let utc = t.to_offset(time::UtcOffset::UTC);
            let compact = format!(
                "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
                utc.year(), u8::from(utc.month()), utc.day(),
                utc.hour(), utc.minute(), utc.second(),
            );
            let rfc = utc.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            serde_json::json!({ "compact": compact, "rfc3339": rfc })
        }).collect()
    };

    Ok(Json(serde_json::json!({
        "event_id": ev.id,
        "count":    items.len(),
        "exdates":  items,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct ExdatesStatsQuery {
    /// `?after=&before=` (sprint #522, paralelo simétrico do #520) restringe
    /// o agregado a uma janela temporal half-open `[after, before)` aplicada
    /// só sobre EXDATEs UTC parseáveis (`kind=utc` com `parsed_utc=Some`).
    /// EXDATEs não-parseáveis (TZID-based, date-only, unknown) são pulados
    /// silenciosamente do agregado quando algum bound é dado — sem range,
    /// todos entram (shape original preservado). 400 se `after >= before`.
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?kind=utc|tzid|date-only|unknown` (sprint #522, paralelo simétrico
    /// do #516 mas em stats) restringe o agregado a um único kind. Composto
    /// com `?after=&before=`: range é aplicado APÓS kind retain, mas só
    /// EXDATEs UTC parseáveis sobreviverem ao range filter (consistente com
    /// #517/#520) — `kind=tzid|date-only|unknown` + range = sempre 0.
    #[serde(default)]
    kind: Option<String>,
    /// `?with_tzid=true|false` (sprint #524, paralelo simétrico do #523 mas
    /// em stats — fechando o trio EXDATE como já fechamos overrides com
    /// #519/#520/#521). Filtra por presença/ausência de TZID. Composto AND
    /// com kind/range: aplicado como retain adicional após os existentes.
    /// Ortogonalidade: `kind=tzid` ⊂ `with_tzid=true` — kind=tzid implica
    /// TZID, mas `with_tzid=true` também captura `kind=unknown` com TZID
    /// malformado. Combinações impossíveis (e.g. `kind=utc&with_tzid=true`)
    /// são aceitas e simplesmente retornam `total=0`.
    #[serde(default)]
    with_tzid: Option<bool>,
    /// `?with_params=true|false` (sprint #524). Filtra por presença/ausência
    /// de outros parâmetros iCal (RANGE, VALUE, etc.) na linha EXDATE.
    /// Composto AND com kind/range/with_tzid no mesmo retain pass.
    #[serde(default)]
    with_params: Option<bool>,
    /// `?top_tzid=N` (sprint #533, paralelo simétrico do #532 mas no list-stats
    /// endpoint do #531) trunca o `tzid_breakdown` pra apenas as N TZIDs mais
    /// frequentes (sort by count desc, ties broken por insertion order do `Vec`
    /// association list — i.e. ordem de aparição no walker pós-retains do
    /// #522/#524) + adiciona `tzid_other_count: usize` agregando soma das counts
    /// das TZIDs descartadas. Fecha dualidade `top_tzid` cross-stats em ambos
    /// stats endpoints da família EXDATE: preview-stats #532 + list-stats #533.
    /// Diferença vs preview-stats #532: aqui o cardinality já está naturalmente
    /// reduzida pelo filter chain server-side (`kind=tzid&after=&before=&
    /// with_tzid=true`), portanto top_tzid é menos crítico — mas oferecido
    /// pra consistência cross-endpoint e UI dashboard que pode usar list-stats
    /// sem filters quando quer overview agregado total. `top_tzid=0` → 400
    /// (sem alternativa equivalente do `include_non_utc=false` aqui — em
    /// list-stats `tzid_breakdown` é sempre presente; pra agregado sem
    /// breakdown, `?kind=utc` reduz `by_kind.tzid=0` que produz `tzid_breakdown
    /// = {}` sem omitir o campo). Sem flag (None) preserva 100% shape #531.
    #[serde(default)]
    top_tzid: Option<usize>,
    /// `?sort_tzid=count_desc|count_asc|name_asc|name_desc` (sprint #534,
    /// presentation flag complementar ao `top_tzid` #533) emite o array
    /// adjacente `tzid_breakdown_order: [tzid1, tzid2, ...]` listando as
    /// chaves do `tzid_breakdown` na ordem solicitada. NÃO altera o objeto
    /// `tzid_breakdown` em si — `serde_json::Map` sem feature
    /// `preserve_order` serializa em ordem alfabética determinística, então
    /// a única forma de transmitir ordem custom em JSON Object é via array
    /// adjacente que a UI itera buscando counts por chave. Aplicado em DUAS
    /// fases compostas com `top_tzid`: (1) selecionar top-N por count desc
    /// (semantics original do #532+#533); (2) ordenar o set retido pelos N
    /// elementos via `sort_tzid`. Variantes: `count_desc`/`count_asc`
    /// ordenam por count com ties broken por insertion order do walker;
    /// `name_asc`/`name_desc` ordenam alfabeticamente pelo TZID (resolve
    /// gap "UI quer ordem alfabética" documentado em #530/#531 — embora
    /// JSON object já entregue isso de graça, `name_asc` torna explícito
    /// e composto com top_tzid produz "top-N alfabeticamente"). Outros
    /// valores → 400. Sem flag (None) omite `tzid_breakdown_order` e
    /// preserva 100% shape do #533.
    #[serde(default)]
    sort_tzid: Option<String>,
    /// `?min_count=N` (sprint #535, dual filosófico ao `top_tzid` #533)
    /// filtra do `tzid_breakdown` qualquer TZID com count < N — `top_tzid`
    /// trunca CARDINALIDADE da CABEÇA (top-N por count desc), `min_count`
    /// filtra LONG-TAIL da CAUDA (TZIDs raros). Combinados oferecem janela
    /// arbitrária no histograma: `min_count=2&top_tzid=10` = "top-10 entre
    /// TZIDs com pelo menos 2 ocorrências cada". Composição em 3 fases
    /// (ordem fixa, escolhida pra preservar semantics intuitiva): (1)
    /// `min_count` filtra long-tail; (2) `top_tzid` seleciona top-N do
    /// universo filtrado; (3) `sort_tzid` ordena o set retido. Emite
    /// `tzid_filtered_count` (paralelo do `tzid_other_count` do #532+#533
    /// mas reportando o que foi excluído pela cauda em vez da cabeça
    /// truncada) somente quando `min_count.is_some()`. `min_count=0` →
    /// 400 (no-op, todo TZID sobrevive — usar ausência do flag pra esse
    /// efeito); `min_count=1` é aceito mesmo sendo igualmente no-op (por
    /// definição todo TZID no breakdown apareceu pelo menos 1 vez), porque
    /// é a primeira fronteira "real" e UI pode querer emitir
    /// `tzid_filtered_count=0` explícito como confirmação. Sem flag (None)
    /// preserva 100% shape do #534.
    #[serde(default)]
    min_count: Option<usize>,
    /// `?include_kind_breakdown=true` (sprint #536, expansão dimensional do
    /// `tzid_breakdown` do #531) emite o objeto adjacente
    /// `tzid_breakdown_by_kind: {tz: {tzid: N, unknown: M}}` particionando o
    /// count de cada TZID retido em duas sub-categorias: tokens cujo
    /// raw_value parseia como `YYYYMMDDTHHMMSS` local (canônico, contado
    /// como `tzid`) vs tokens com TZID presente mas formato local inválido
    /// (malformado, contado como `unknown`). Diferente do `by_kind` do #522
    /// (que classifica TUDO em utc/tzid/date-only/unknown e onde
    /// `kind="unknown"` implica TZID ausente), aqui `unknown` é uma
    /// sub-categoria DENTRO de `kind="tzid"`: o item já passou pela
    /// validação de TZID-presente em `parse_exdates_rich` mas o token
    /// (e.g. `20991301T256000`) não é um datetime local válido. Útil pra
    /// audit "qual fração das EXDATEs com TZID está em formato canônico
    /// vs corrompida no campo local". Invariant: `sum(canonical +
    /// malformed) == tzid_breakdown[tz]` para cada `tz` retido. Aplicado
    /// APÓS toda a chain de presentation (`min_count` → `top_tzid` →
    /// `sort_tzid`) — só os TZIDs retidos no `kept` final aparecem no
    /// objeto adjacente, na MESMA ordem alfabética do JSON Object (a
    /// ordem custom continua só no `tzid_breakdown_order` do #534). Sem
    /// flag (None ou false) omite o objeto e preserva 100% shape do #535.
    /// `include_kind_breakdown=true` mas `tzid_breakdown` vazio (e.g.
    /// `kind=utc` ou todos filtraram-out) emite `tzid_breakdown_by_kind:
    /// {}` (mesma semantics "flag aceita, sem dados" do `tzid_other_count`
    /// quando `breakdown.len() <= n` no #533).
    #[serde(default)]
    include_kind_breakdown: Option<bool>,
}

/// GET /api/v1/calendars/:cal_id/events/:id/exdates/stats — agrega counts
/// de EXDATEs por classificação de formato (sprint #522, paralelo simétrico
/// do #519 mas pra EXDATE list com count-by-kind ao invés de count-by-
/// presence-of-fields). Retorna `{event_id, total, by_kind:{utc, tzid,
/// date_only, unknown}}` onde os 4 buckets são DISJUNTOS — soma das 4 =
/// total (invariant testável). Útil pra dashboards "qual a distribuição
/// de formatos de EXDATE no evento" (audit "quantos estão em formato
/// não-MVP" via `by_kind.unknown + by_kind.tzid + by_kind.date_only`)
/// sem puxar lista inteira do #511 e classificar client-side. Reusa
/// `parse_exdates_rich` (mesma fonte do #511/#516) sem nenhuma extensão
/// — só itera e tally. Read-only, não exige WRITE+. 404 se evento não
/// existe.
///
/// `?after=&before=` (paralelo simétrico do #520) restringe a janela
/// temporal half-open `[after, before)`. Aplicado APÓS kind retain — só
/// EXDATEs UTC parseáveis (`parsed_utc=Some`) sobrevivem; TZID-based,
/// date-only e unknown são silenciosamente excluídos quando algum bound
/// é dado (consistente com #517/#520). `total`/`by_kind` agregam só
/// sobre o subset, mantendo invariants. 400 se `after >= before`.
///
/// `?kind=utc|tzid|date-only|unknown` (paralelo simétrico do #516 mas
/// em stats) restringe a UM único kind. Composto com range: combinação
/// `kind=tzid|date-only|unknown` + range sempre retorna `total=0` por
/// design (TZID/date-only/unknown não têm `parsed_utc` pra comparar
/// com bounds).
///
/// `?with_tzid=true|false&with_params=true|false` (sprint #524, paralelo
/// simétrico do #523 mas em stats) filtra qualitativamente por presença
/// dos campos TZID/parâmetros. Aplicado AND-composto como retain adicional
/// após kind+range. Ortogonalidade `kind=tzid` ⊂ `with_tzid=true` documentada
/// no #523. Combinações impossíveis (e.g. `kind=utc&with_tzid=true`) são
/// aceitas e retornam `total=0` consistente com #522 (`kind=tzid` + range).
async fn exdates_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<ExdatesStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    if q.top_tzid == Some(0) {
        return Err(CalendarError::BadRequest(
            "top_tzid must be >= 1 (use kind=utc to force tzid_breakdown empty)".into()
        ));
    }
    if q.min_count == Some(0) {
        return Err(CalendarError::BadRequest(
            "min_count must be >= 1 (omit flag for full breakdown)".into()
        ));
    }
    let sort_mode: Option<&str> = match q.sort_tzid.as_deref() {
        None | Some("") => None,
        Some(s @ ("count_desc" | "count_asc" | "name_asc" | "name_desc")) => Some(s),
        Some(other) => return Err(CalendarError::BadRequest(
            format!("sort_tzid must be 'count_desc', 'count_asc', 'name_asc' or 'name_desc', got '{other}'")
        )),
    };
    let kind_filter: Option<&str> = match q.kind.as_deref() {
        None | Some("") => None,
        Some(k @ ("utc" | "tzid" | "date-only" | "unknown")) => Some(k),
        Some(other) => return Err(CalendarError::BadRequest(
            format!("kind must be 'utc', 'tzid', 'date-only' or 'unknown', got '{other}'")
        )),
    };
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    let mut items = parse_exdates_rich(&ev.ical_raw);
    if let Some(k) = kind_filter {
        items.retain(|info| info.kind == k);
    }
    if q.after.is_some() || q.before.is_some() {
        items.retain(|info| {
            let parsed = match info.parsed_utc {
                Some(t) => t,
                None    => return false,
            };
            if let Some(a) = q.after  { if parsed <  a { return false; } }
            if let Some(b) = q.before { if parsed >= b { return false; } }
            true
        });
    }
    if q.with_tzid.is_some() || q.with_params.is_some() {
        items.retain(|info| {
            if let Some(want) = q.with_tzid   { if info.tzid.is_some()   != want { return false; } }
            if let Some(want) = q.with_params { if info.params.is_some() != want { return false; } }
            true
        });
    }
    let total = items.len();
    let mut k_utc       = 0usize;
    let mut k_tzid      = 0usize;
    let mut k_date_only = 0usize;
    let mut k_unknown   = 0usize;
    let include_kind_breakdown = q.include_kind_breakdown.unwrap_or(false);
    let mut tzid_breakdown: Vec<(String, usize)> = Vec::new();
    // (tzid, canonical_count, malformed_count) — populado só quando
    // include_kind_breakdown=true; mantido em paralelo a tzid_breakdown
    // pra que a invariant `canonical + malformed == tzid_breakdown[tz]`
    // seja preservada por construção (mesmo loop, mesma chave).
    let mut tzid_by_kind: Vec<(String, usize, usize)> = Vec::new();
    for info in &items {
        match info.kind {
            "utc"       => k_utc       += 1,
            "tzid"      => {
                k_tzid += 1;
                if let Some(tz) = info.tzid.as_deref() {
                    match tzid_breakdown.iter().position(|(k, _)| k == tz) {
                        Some(i) => tzid_breakdown[i].1 += 1,
                        None    => tzid_breakdown.push((tz.to_string(), 1)),
                    }
                    if include_kind_breakdown {
                        let canonical = is_canonical_local_datetime(&info.raw_value);
                        match tzid_by_kind.iter().position(|(k, _, _)| k == tz) {
                            Some(i) => {
                                if canonical { tzid_by_kind[i].1 += 1; }
                                else         { tzid_by_kind[i].2 += 1; }
                            }
                            None => {
                                let (c, u) = if canonical { (1, 0) } else { (0, 1) };
                                tzid_by_kind.push((tz.to_string(), c, u));
                            }
                        }
                    }
                }
            }
            "date-only" => k_date_only += 1,
            _           => k_unknown   += 1,
        }
    }
    let (filtered, filtered_count): (Vec<(String, usize)>, usize) = match q.min_count {
        Some(m) => {
            let mut keep = Vec::with_capacity(tzid_breakdown.len());
            let mut excluded = 0usize;
            for (tz, c) in &tzid_breakdown {
                if *c >= m { keep.push((tz.clone(), *c)); } else { excluded += 1; }
            }
            (keep, excluded)
        }
        None => (tzid_breakdown.clone(), 0),
    };
    let (mut kept, other_count): (Vec<(String, usize)>, usize) = match q.top_tzid {
        Some(n) if filtered.len() > n => {
            let mut sorted = filtered.clone();
            sorted.sort_by(|a, b| b.1.cmp(&a.1));
            let other: usize = sorted.iter().skip(n).map(|(_, c)| *c).sum();
            sorted.truncate(n);
            (sorted, other)
        }
        _ => (filtered, 0),
    };
    match sort_mode {
        Some("count_desc") => kept.sort_by(|a, b| b.1.cmp(&a.1)),
        Some("count_asc")  => kept.sort_by(|a, b| a.1.cmp(&b.1)),
        Some("name_asc")   => kept.sort_by(|a, b| a.0.cmp(&b.0)),
        Some("name_desc")  => kept.sort_by(|a, b| b.0.cmp(&a.0)),
        _ => {}
    }
    let mut breakdown_obj = serde_json::Map::new();
    for (tz, n) in &kept {
        breakdown_obj.insert(tz.clone(), serde_json::json!(n));
    }
    let mut payload = serde_json::json!({
        "event_id":       ev.id,
        "total":          total,
        "by_kind": {
            "utc":       k_utc,
            "tzid":      k_tzid,
            "date_only": k_date_only,
            "unknown":   k_unknown,
        },
        "tzid_breakdown": serde_json::Value::Object(breakdown_obj),
    });
    if q.top_tzid.is_some() {
        payload["tzid_other_count"] = serde_json::json!(other_count);
    }
    if q.min_count.is_some() {
        payload["tzid_filtered_count"] = serde_json::json!(filtered_count);
    }
    if sort_mode.is_some() {
        let order: Vec<serde_json::Value> = kept.iter()
            .map(|(tz, _)| serde_json::Value::String(tz.clone()))
            .collect();
        payload["tzid_breakdown_order"] = serde_json::Value::Array(order);
    }
    if include_kind_breakdown {
        let mut by_kind_obj = serde_json::Map::new();
        for (tz, _) in &kept {
            let (c, u) = tzid_by_kind.iter()
                .find(|(k, _, _)| k == tz)
                .map(|(_, c, u)| (*c, *u))
                .unwrap_or((0, 0));
            by_kind_obj.insert(tz.clone(), serde_json::json!({
                "tzid":    c,
                "unknown": u,
            }));
        }
        payload["tzid_breakdown_by_kind"] = serde_json::Value::Object(by_kind_obj);
    }
    Ok(Json(payload))
}

/// Variante "rica" do `parse_exdates` que captura TAMBÉM linhas com TZID,
/// parametros e formatos não-UTC (date-only, RFC3339 com offset não-Z).
/// Cada token EXDATE vira um `ExdateInfo` com `kind` classificando o
/// formato. Usada exclusivamente pelo `list_exdates?detail=full` (#511) —
/// `parse_exdates` plain continua sendo a fonte autoritativa pro filter
/// de expansão (subset MVP UTC-only).
fn parse_exdates_rich(ical_raw: &str) -> Vec<ExdateInfo> {
    use time::format_description::FormatItem;
    use time::macros::format_description;
    static FMT_UTC: &[FormatItem<'static>] = format_description!(
        "[year][month][day]T[hour][minute][second]Z"
    );
    static FMT_DATE: &[FormatItem<'static>] = format_description!(
        "[year][month][day]"
    );
    let mut out = Vec::new();
    for line in ical_raw.lines() {
        let trimmed = line.trim_start();
        let upper6: String = trimmed.chars().take(6).collect::<String>().to_ascii_uppercase();
        if !upper6.starts_with("EXDATE") {
            continue;
        }
        // EXDATE pode ser "EXDATE:..." ou "EXDATE;PARAM=...:..."
        let after_name = &trimmed["EXDATE".len()..];
        let (params_str, value): (Option<String>, &str) = match after_name.chars().next() {
            Some(':') => (None, &after_name[1..]),
            Some(';') => {
                if let Some(colon_idx) = after_name.find(':') {
                    (Some(after_name[1..colon_idx].to_string()), &after_name[colon_idx + 1..])
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        let tzid = params_str.as_ref().and_then(|p| {
            for kv in p.split(';') {
                let kv_trim = kv.trim();
                let upper_kv: String = kv_trim.chars().take(5).collect::<String>().to_ascii_uppercase();
                if upper_kv.starts_with("TZID=") {
                    return Some(kv_trim[5..].to_string());
                }
            }
            None
        });
        for tok in value.split(',') {
            let tok = tok.trim();
            if tok.is_empty() { continue; }
            let (kind, parsed): (&'static str, Option<OffsetDateTime>) =
                if let Ok(ts) = OffsetDateTime::parse(tok, &FMT_UTC) {
                    ("utc", Some(ts))
                } else if tzid.is_some() {
                    ("tzid", None)
                } else if time::Date::parse(tok, &FMT_DATE).is_ok() {
                    ("date-only", None)
                } else {
                    ("unknown", None)
                };
            out.push(ExdateInfo {
                raw_value:   tok.to_string(),
                tzid:        tzid.clone(),
                params:      params_str.clone(),
                parsed_utc:  parsed,
                kind,
            });
        }
    }
    out
}

struct ExdateInfo {
    raw_value:   String,
    tzid:        Option<String>,
    params:      Option<String>,
    parsed_utc:  Option<OffsetDateTime>,
    kind:        &'static str,
}

/// Sprint #536 — classifica raw_value de EXDATE com `kind="tzid"` em
/// canônico (`YYYYMMDDTHHMMSS` local datetime válido) vs malformado.
/// `parse_exdates_rich` rotula como `kind="tzid"` qualquer linha com TZID
/// presente, independente da validade do token; aqui validamos o formato
/// local pra suportar o drill-down `tzid_breakdown_by_kind` do #536 sem
/// alterar a taxonomia central de kinds (que tem ripple em #516/#522/#529).
fn is_canonical_local_datetime(s: &str) -> bool {
    use time::format_description::FormatItem;
    use time::macros::format_description;
    static FMT_LOCAL: &[FormatItem<'static>] = format_description!(
        "[year][month][day]T[hour][minute][second]"
    );
    time::PrimitiveDateTime::parse(s.trim(), &FMT_LOCAL).is_ok()
}

/// DELETE /api/v1/calendars/:cal_id/events/:id/exdates — remove TODAS as
/// linhas EXDATE do VEVENT, restaurando todas as ocorrências canceladas
/// (sprint #491, bulk inverso de #488). Idempotente: `{removed: 0}` se
/// não houver EXDATE. Requer WRITE+. 400 se evento não tem rrule (não há
/// instâncias pra restaurar). Re-salva via `EventRepo::update`.
async fn clear_exdates(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }

    let before = parse_exdates(&ev.ical_raw).len();
    if before == 0 {
        return Ok(Json(serde_json::json!({
            "event_id": ev.id, "removed": 0,
        })));
    }

    let new_raw = strip_exdate_lines(&ev.ical_raw);
    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id": ev.id,
        "removed":  before,
    })))
}

/// DELETE /api/v1/calendars/:cal_id/events/:id/exdates/:instance — remove
/// UMA EXDATE específica (sprint #491, inverso pontual de #488).
/// `:instance` aceita formato compacto `YYYYMMDDTHHMMSSZ` ou RFC3339; é
/// canonicalizado pra UTC compact antes de match. Idempotente: `{removed:
/// false}` se não existir. Requer WRITE+. Reescreve linhas EXDATE
/// preservando outros valores na mesma linha (split por vírgula).
async fn delete_exdate(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id, instance)): Path<(Uuid, Uuid, String)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }

    let target = parse_one_exdate(&instance).ok_or_else(|| CalendarError::BadRequest(
        format!("instance must be RFC3339 or YYYYMMDDTHHMMSSZ, got {instance}")
    ))?;
    let target_utc = target.to_offset(time::UtcOffset::UTC);

    let exists: bool = parse_exdates(&ev.ical_raw)
        .iter()
        .any(|t| t.to_offset(time::UtcOffset::UTC) == target_utc);
    if !exists {
        let compact = format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            target_utc.year(), u8::from(target_utc.month()), target_utc.day(),
            target_utc.hour(), target_utc.minute(), target_utc.second(),
        );
        return Ok(Json(serde_json::json!({
            "event_id": ev.id, "instance": compact, "removed": false,
        })));
    }

    let new_raw = remove_exdate_value(&ev.ical_raw, target_utc);
    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    let compact = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        target_utc.year(), u8::from(target_utc.month()), target_utc.day(),
        target_utc.hour(), target_utc.minute(), target_utc.second(),
    );
    Ok(Json(serde_json::json!({
        "event_id": ev.id, "instance": compact, "removed": true,
    })))
}

/// Aceita "YYYYMMDDTHHMMSSZ" compact ou RFC3339 e devolve OffsetDateTime.
fn parse_one_exdate(raw: &str) -> Option<OffsetDateTime> {
    use time::format_description::FormatItem;
    use time::macros::format_description;
    static FMT: &[FormatItem<'static>] = format_description!(
        "[year][month][day]T[hour][minute][second]Z"
    );
    let trimmed = raw.trim();
    if let Ok(t) = OffsetDateTime::parse(trimmed, &FMT) {
        return Some(t);
    }
    OffsetDateTime::parse(trimmed, &time::format_description::well_known::Rfc3339).ok()
}

/// Remove TODAS as linhas começando com `EXDATE:` (case-insensitive). Mantém
/// EOL original. Não toca em linhas com TZID/parametros (`EXDATE;TZID=...`)
/// porque o subset MVP só lê UTC sem parametros (parse_exdates ignora).
fn strip_exdate_lines(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for src_line in raw.split_inclusive('\n') {
        let probe: String = src_line.trim_start().chars().take(7).collect::<String>().to_ascii_uppercase();
        if probe.starts_with("EXDATE:") {
            continue;
        }
        out.push_str(src_line);
    }
    out
}

/// Remove um valor EXDATE específico mantendo outros valores na mesma linha
/// (EXDATE permite vírgula-separados). Se a linha ficar vazia após remoção,
/// remove a linha inteira. Match por equivalência UTC (re-parsing pra
/// normalizar). EOL preservado.
fn remove_exdate_value(raw: &str, target_utc: OffsetDateTime) -> String {
    use time::format_description::FormatItem;
    use time::macros::format_description;
    static FMT: &[FormatItem<'static>] = format_description!(
        "[year][month][day]T[hour][minute][second]Z"
    );
    let mut out = String::with_capacity(raw.len());
    for src_line in raw.split_inclusive('\n') {
        let trimmed_start = src_line.trim_start();
        let probe: String = trimmed_start.chars().take(7).collect::<String>().to_ascii_uppercase();
        if !probe.starts_with("EXDATE:") {
            out.push_str(src_line);
            continue;
        }
        // Detect EOL of this line
        let (body, eol) = if let Some(stripped) = src_line.strip_suffix("\r\n") {
            (stripped, "\r\n")
        } else if let Some(stripped) = src_line.strip_suffix('\n') {
            (stripped, "\n")
        } else {
            (src_line, "")
        };
        // body still has indent + "EXDATE:" prefix
        let lead_len = body.len() - trimmed_start.len();
        let lead = &body[..lead_len];
        let value = &trimmed_start["EXDATE:".len()..];
        let kept: Vec<&str> = value.split(',')
            .filter(|tok| {
                let t = tok.trim();
                if t.is_empty() { return false; }
                match OffsetDateTime::parse(t, &FMT) {
                    Ok(parsed) => parsed.to_offset(time::UtcOffset::UTC) != target_utc,
                    Err(_) => true,
                }
            })
            .collect();
        if kept.is_empty() {
            // dropping the whole line
            continue;
        }
        out.push_str(lead);
        out.push_str("EXDATE:");
        out.push_str(&kept.join(","));
        out.push_str(eol);
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct OverrideInstanceBody {
    /// Original instance dtstart (RFC3339 UTC).
    #[serde(with = "time::serde::rfc3339")]
    instance: OffsetDateTime,
    /// Campos opcionais a sobrescrever na occurrence — pelo menos um precisa
    /// vir, senão o override é no-op.
    summary:     Option<String>,
    description: Option<String>,
    location:    Option<String>,
    /// Se omitido, override mantém dtstart=instance.
    #[serde(default, with = "time::serde::rfc3339::option")]
    dtstart:     Option<OffsetDateTime>,
    /// Se omitido, override não emite DTEND (cliente herda do master).
    #[serde(default, with = "time::serde::rfc3339::option")]
    dtend:       Option<OffsetDateTime>,
}

/// POST /api/v1/calendars/:cal_id/events/:id/override-instance
/// Cria override para uma ocorrência específica de um evento recorrente
/// sem cancelar (sprint #495, extensão do #488). Em RFC 5545 isso é feito
/// via VEVENT separado no mesmo VCALENDAR com UID idêntico ao master +
/// linha `RECURRENCE-ID:<dtstamp UTC>` apontando pra ocorrência original.
/// Implementação MVP: injeta novo bloco VEVENT antes de END:VCALENDAR
/// reusando o UID extraído do master. Body permite sobrescrever
/// summary/description/location/dtstart/dtend (pelo menos 1 obrigatório).
/// Idempotência: 409 se override pra esse RECURRENCE-ID já existe (use
/// PUT no master pra editar in-place — fora do escopo MVP). 400 se evento
/// não tem rrule. 404 se evento não existe. Requer WRITE+. Retorna
/// `{event_id, instance, recurrence_id, sequence}`.
async fn override_event_instance(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Json(body):   Json<OverrideInstanceBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }
    if ev.rrule.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
        return Err(CalendarError::BadRequest(
            "event has no rrule — overrides only apply to recurring series".into()
        ));
    }
    if body.summary.is_none() && body.description.is_none()
        && body.location.is_none() && body.dtstart.is_none() && body.dtend.is_none()
    {
        return Err(CalendarError::BadRequest(
            "at least one of summary/description/location/dtstart/dtend required".into()
        ));
    }

    let inst_utc = body.instance.to_offset(time::UtcOffset::UTC);
    let recurrence_id = format_compact_utc(inst_utc);

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot create override".into()
    ))?;

    let exdated: bool = parse_exdates(&ev.ical_raw)
        .iter()
        .any(|t| t.to_offset(time::UtcOffset::UTC) == inst_utc);
    if exdated {
        return Err(CalendarError::Conflict(
            format!("instance {recurrence_id} is cancelled via EXDATE — \
                     remove via DELETE /exdates/:instance before overriding")
        ));
    }

    if has_recurrence_id_override(&ev.ical_raw, &uid, &recurrence_id) {
        return Err(CalendarError::Conflict(
            format!("override for RECURRENCE-ID:{recurrence_id} already exists")
        ));
    }

    let dtstart = body.dtstart.unwrap_or(inst_utc).to_offset(time::UtcOffset::UTC);
    let mut block = String::new();
    let eol = if ev.ical_raw.contains("\r\n") { "\r\n" } else { "\n" };

    block.push_str("BEGIN:VEVENT");                block.push_str(eol);
    block.push_str(&format!("UID:{uid}"));          block.push_str(eol);
    block.push_str(&format!("RECURRENCE-ID:{recurrence_id}")); block.push_str(eol);
    block.push_str(&format!("DTSTAMP:{}", format_compact_utc(OffsetDateTime::now_utc())));
    block.push_str(eol);
    block.push_str(&format!("DTSTART:{}", format_compact_utc(dtstart))); block.push_str(eol);
    if let Some(end) = body.dtend {
        block.push_str(&format!("DTEND:{}", format_compact_utc(end.to_offset(time::UtcOffset::UTC))));
        block.push_str(eol);
    }
    if let Some(s) = body.summary.as_deref() {
        block.push_str(&format!("SUMMARY:{}", escape_ics_text(s))); block.push_str(eol);
    }
    if let Some(s) = body.description.as_deref() {
        block.push_str(&format!("DESCRIPTION:{}", escape_ics_text(s))); block.push_str(eol);
    }
    if let Some(s) = body.location.as_deref() {
        block.push_str(&format!("LOCATION:{}", escape_ics_text(s))); block.push_str(eol);
    }
    block.push_str("END:VEVENT"); block.push_str(eol);

    let new_raw = inject_before_end_vcalendar(&ev.ical_raw, &block);

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":      ev.id,
        "instance":      recurrence_id,
        "recurrence_id": recurrence_id,
        "sequence":      updated.sequence,
    })))
}

/// `YYYYMMDDTHHMMSSZ` — formato compact UTC dos VEVENT properties.
fn format_compact_utc(t: OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        t.year(), u8::from(t.month()), t.day(),
        t.hour(), t.minute(), t.second(),
    )
}

/// Extrai o UID do primeiro bloco VEVENT do ical raw. Linha "UID:..."
/// case-insensitive no nome da prop.
fn extract_uid(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim_start();
        let upper: String = trimmed.chars().take(4).collect::<String>().to_ascii_uppercase();
        if upper.starts_with("UID:") {
            return Some(trimmed["UID:".len()..].trim().to_string());
        }
    }
    None
}

/// True se o ical raw já contém um VEVENT com (UID,RECURRENCE-ID) pareados.
/// Walk simples por blocos VEVENT (BEGIN:VEVENT...END:VEVENT) checando
/// ambas linhas dentro de cada bloco. Match por UID exato + RECURRENCE-ID
/// compact (caller já normalizou).
fn has_recurrence_id_override(raw: &str, uid: &str, recurrence_id: &str) -> bool {
    let mut in_event = false;
    let mut found_uid = false;
    let mut found_recid = false;
    for line in raw.lines() {
        let trimmed = line.trim_start();
        let upper_head: String = trimmed.chars().take(14).collect::<String>().to_ascii_uppercase();
        if upper_head.starts_with("BEGIN:VEVENT") {
            in_event = true; found_uid = false; found_recid = false; continue;
        }
        if upper_head.starts_with("END:VEVENT") {
            if in_event && found_uid && found_recid { return true; }
            in_event = false; continue;
        }
        if !in_event { continue; }
        let upper_short: String = trimmed.chars().take(4).collect::<String>().to_ascii_uppercase();
        if upper_short.starts_with("UID:") {
            let v = trimmed["UID:".len()..].trim();
            if v == uid { found_uid = true; }
        } else {
            let upper_long: String = trimmed.chars().take(14).collect::<String>().to_ascii_uppercase();
            if upper_long.starts_with("RECURRENCE-ID:") {
                let v = trimmed["RECURRENCE-ID:".len()..].trim();
                if v == recurrence_id { found_recid = true; }
            }
        }
    }
    false
}

/// Injeta `block` antes da última linha END:VCALENDAR. Mantém EOL original.
fn inject_before_end_vcalendar(raw: &str, block: &str) -> String {
    let crlf = raw.contains("\r\n");
    let eol  = if crlf { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(raw.len() + block.len() + 2);
    let mut injected = false;
    for src_line in raw.split_inclusive('\n') {
        if !injected {
            let probe: String = src_line.trim_end().to_ascii_uppercase();
            if probe == "END:VCALENDAR" {
                out.push_str(block);
                injected = true;
            }
        }
        out.push_str(src_line);
    }
    if !injected {
        out.push_str(block);
        out.push_str(eol);
    }
    out
}

/// Escape RFC 5545 minimal: `\` → `\\`, `;` → `\;`, `,` → `\,`, newlines → `\n`.
fn escape_ics_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ';'  => out.push_str("\\;"),
            ','  => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c    => out.push(c),
        }
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct ListOverridesQuery {
    /// `?detail=full` inclui description+location em cada override (paridade
    /// com get-one #500). Default `summary` mantém shape original do #496
    /// (summary/dtstart/dtend só) — payload leve. Qualquer outro valor é
    /// rejeitado com 400.
    #[serde(default)]
    detail: Option<String>,
    /// `?after=&before=` (sprint #517) filtra a lista por RECURRENCE-ID
    /// no intervalo half-open `[after, before)` — paralelo do range filter
    /// do touch-overrides-by-range (#509). Ambos opcionais; sem nenhum
    /// = sem filtro. RECURRENCE-IDs não-parseáveis (não-UTC) são pulados
    /// silenciosamente quando algum bound é dado. 400 se `after >= before`.
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?has_summary=&has_dtstart=&has_dtend=` (sprint #518) filtros
    /// qualitativos de presença — true exige campo presente, false exige
    /// ausência. Combinados em AND. Útil pra UI segmentar overrides "só
    /// rename de título" (has_summary=true&has_dtstart=false&has_dtend=false)
    /// vs. "só reschedule" (has_summary=false&has_dtstart=true). Aplicado
    /// após range filter. Sem nenhum dos 3 = sem filtro de presença.
    #[serde(default)]
    has_summary: Option<bool>,
    #[serde(default)]
    has_dtstart: Option<bool>,
    #[serde(default)]
    has_dtend:   Option<bool>,
}

/// GET /api/v1/calendars/:cal_id/events/:id/overrides — lista os
/// RECURRENCE-ID overrides existentes no VCALENDAR (sprint #496, paralelo
/// ao EXDATE list #491). Retorna `{event_id, count, overrides:[{compact,
/// rfc3339, summary?, dtstart?, dtend?}]}` por default. Com `?detail=full`
/// (sprint #503) adiciona `description?` + `location?` em cada item pra
/// paridade com get-one (#500) — útil pra UI que precisa exibir lista
/// completa sem N+1 GETs por override. Reusa `extract_uid` do master e
/// walk por blocos VEVENT pareando UID + RECURRENCE-ID.
///
/// `?after=&before=` (sprint #517, paralelo do touch-overrides-by-range
/// #509) filtra a lista por RECURRENCE-ID no intervalo half-open
/// `[after, before)` — útil pra UI que só quer overrides de uma janela
/// (ex: "esta semana", "próximo mês"). Ambos opcionais; ausência total
/// preserva 100% shape do #496. RECURRENCE-IDs não-parseáveis como UTC
/// (ex: TZID-based) são pulados silenciosamente quando algum bound é
/// dado — sem range, todos os overrides aparecem (mesmo formato exótico).
/// 400 se `after >= before`.
///
/// `?has_summary=&has_dtstart=&has_dtend=` (sprint #518, variant
/// qualitativa do #517) filtros booleanos de presença em AND — true
/// exige o campo presente no override, false exige ausência. Combinados
/// segmentam categorias semânticas: "só rename de título"
/// (has_summary=true&has_dtstart=false&has_dtend=false) vs. "só
/// reschedule" (has_summary=false&has_dtstart=true). Aplicados após o
/// range filter — `count` final reflete intersecção dos dois passes.
/// Cada flag é independentemente opcional. `description`/`location` ficam
/// de fora porque só estão presentes em `?detail=full` (assimétrico).
///
/// Read-only, não exige WRITE+. 404 se evento não existe. 400 se detail
/// desconhecido ou `after >= before`.
async fn list_overrides(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<ListOverridesQuery>,
) -> Result<Json<serde_json::Value>> {
    let full = match q.detail.as_deref() {
        None | Some("") | Some("summary") => false,
        Some("full") => true,
        Some(other) => return Err(CalendarError::BadRequest(
            format!("detail must be 'summary' or 'full', got '{other}'")
        )),
    };
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    let uid = extract_uid(&ev.ical_raw).unwrap_or_default();
    let mut items = list_recurrence_id_overrides(&ev.ical_raw, &uid, full);
    if q.after.is_some() || q.before.is_some() {
        items.retain(|item| {
            let compact = match item.get("compact").and_then(|v| v.as_str()) {
                Some(s) => s,
                None    => return false,
            };
            let parsed = match parse_one_exdate(compact) {
                Some(t) => t,
                None    => return false,
            };
            if let Some(a) = q.after  { if parsed <  a { return false; } }
            if let Some(b) = q.before { if parsed >= b { return false; } }
            true
        });
    }
    if q.has_summary.is_some() || q.has_dtstart.is_some() || q.has_dtend.is_some() {
        items.retain(|item| {
            let present = |key: &str| item.get(key).map(|v| !v.is_null()).unwrap_or(false);
            if let Some(want) = q.has_summary { if present("summary") != want { return false; } }
            if let Some(want) = q.has_dtstart { if present("dtstart") != want { return false; } }
            if let Some(want) = q.has_dtend   { if present("dtend")   != want { return false; } }
            true
        });
    }
    Ok(Json(serde_json::json!({
        "event_id":  ev.id,
        "count":     items.len(),
        "overrides": items,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct OverridesStatsQuery {
    /// `?after=&before=` (sprint #520, composição de #517 + #519) restringe
    /// o agregado a uma janela temporal half-open `[after, before)`.
    /// RECURRENCE-IDs não-parseáveis como UTC (TZID-based, date-only,
    /// malformados) são pulados silenciosamente quando algum bound é dado
    /// — sem range, todos os overrides entram no agregado (shape original
    /// do #519 preservado). 400 se `after >= before`.
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?has_summary=&has_dtstart=&has_dtend=` (sprint #521, composição de
    /// #518 + #519) restringe o agregado por presença qualitativa em AND
    /// — true exige campo presente, false exige ausência. Cada flag opcional
    /// independente; aplicados após o range filter do #520, preservando
    /// composição. Útil pra agregar "qual a distribuição entre overrides
    /// que SÓ renomeiam título" (`has_summary=true&has_dtstart=false&
    /// has_dtend=false`) — `total`/`by_field`/`by_category` agregam só
    /// sobre o subset, mantendo invariants. Sem nenhum dos 3 ≡ shape #520.
    #[serde(default)]
    has_summary: Option<bool>,
    #[serde(default)]
    has_dtstart: Option<bool>,
    #[serde(default)]
    has_dtend:   Option<bool>,
    /// `?top_tzid=N` (sprint #539, paralelo do #532+#533 mas no overrides
    /// scope) trunca o `tzid_breakdown` do #538 pras N TZIDs mais frequentes
    /// (sort by count desc, ties broken por insertion order do `Vec`
    /// association list — ordem de aparição nos overrides retidos pós-filtro).
    /// Adiciona `tzid_other_count: usize` agregando a soma das counts das
    /// TZIDs descartadas. Flag opcional, ausência ≡ shape #538 (breakdown
    /// completo, sem `tzid_other_count`). `top_tzid=0` → 400 ("must be >= 1").
    #[serde(default)]
    top_tzid:    Option<usize>,
    /// `?sort_tzid=count_desc|count_asc|name_asc|name_desc` (sprint #540,
    /// paralelo do #534 mas no overrides scope) ordena o `tzid_breakdown`
    /// via array adjacente `tzid_breakdown_order: [tz...]`. Necessário
    /// porque `serde_json::Map` sem feature `preserve_order` usa BTreeMap
    /// interno e serializa Object em ordem alfabética determinística —
    /// alterar `.insert()` order não afeta JSON. Composto com `top_tzid`:
    /// (1) top-N selection por count desc, (2) re-ordena o set retido pelo
    /// sort_mode escolhido. Flag ausente preserva shape #539 (sem array).
    /// String vazia ou None skip; outros valores -> 400 listando opções.
    #[serde(default)]
    sort_tzid:   Option<String>,
    /// `?min_count=N` (sprint #541, paralelo do #535 mas no overrides scope)
    /// filtra a long-tail removendo do `tzid_breakdown` qualquer TZID com
    /// count < N + adiciona `tzid_filtered_count: usize` agregando soma das
    /// counts removidas (paralelo simétrico do `tzid_other_count` do #539
    /// — mas reportando o que foi excluído pela CAUDA, não pela CABEÇA).
    /// Composição em 3 fases ordem fixa: (1) min_count filtra long-tail,
    /// (2) top_tzid trunca cabeça do universo filtrado, (3) sort_tzid
    /// apresenta. `min_count=0` → 400; `min_count=1` aceito mesmo no-op.
    /// `tzid_filtered_count` SOMENTE quando `min_count.is_some()` (mesmo
    /// que filtered=0 — UI sabe que flag foi aceita); ausência preserva
    /// shape #540.
    #[serde(default)]
    min_count:   Option<usize>,
    /// `?include_kind_breakdown=true` (sprint #542, paralelo do #536 mas
    /// no overrides scope — fechando port das 5 famílias EXDATE-stats em
    /// overrides_stats: inclusão, head truncation, cosmetic ordering, tail
    /// filtering, kind drill-down) emite o objeto adjacente
    /// `tzid_breakdown_by_kind: {tz: {tzid: N, unknown: M}}` particionando
    /// o count de cada TZID retido em duas sub-categorias: tokens cujo
    /// raw_value parseia como `YYYYMMDDTHHMMSS` local datetime (canônico,
    /// `tzid`) vs malformados (`unknown`). Diferente do EXDATE drill-down
    /// (#536) que opera sobre `parse_exdates_rich` com `info.raw_value`,
    /// aqui validamos `dtstart_value`/`dtend_value` capturados pelo
    /// `list_recurrence_id_overrides` em paralelo aos `dtstart_tzid`/
    /// `dtend_tzid` (sprint #542 estendeu o parser pra emitir o token
    /// pós-colon de linhas `DTSTART;TZID=…:value`/`DTEND;…:value` num
    /// campo separado, preservando 100% do `present(dtstart)` semantics
    /// usado por `has_dtstart` filter de #518/#521 — `dtstart`/`dtend`
    /// continuam null quando há TZID, conforme antes). Invariant:
    /// `sum(canonical + malformed) == tzid_breakdown[tz]` por TZID retido.
    /// Aplicado APÓS toda a chain `min_count → top_tzid → sort_tzid` —
    /// só os retidos no `kept` final aparecem no objeto adjacente, em
    /// ordem alfabética (BTreeMap default; ordem custom continua em
    /// `tzid_breakdown_order`). Sem flag (None ou false) omite o objeto;
    /// `include_kind_breakdown=true` mas `kept` vazio emite `{}` (mesma
    /// semantics "flag aceita, sem dados" do `tzid_other_count` quando
    /// `breakdown.len() <= n`).
    #[serde(default)]
    include_kind_breakdown: Option<bool>,
}

/// GET /api/v1/calendars/:cal_id/events/:id/overrides/stats — agrega
/// counts de presença dos campos `summary`/`dtstart`/`dtend` em todos os
/// overrides do evento (sprint #519, agregado do filter qualitativo do
/// #518). Retorna `{event_id, total, by_field:{summary:{present,absent},
/// dtstart:{...}, dtend:{...}}, by_category:{none, only_summary,
/// only_dtstart, only_dtend, summary_dtstart, summary_dtend,
/// dtstart_dtend, all_three}}`. Útil pra dashboards exibirem distribuição
/// de tipos de override sem precisar puxar a lista inteira (#496/#503/
/// #517/#518) e contar client-side. `by_field` é cardinality marginal
/// (cada flag conta independentemente, soma de present+absent = total).
/// `by_category` particiona overrides em 8 buckets disjuntos por
/// combinação de presença — soma das 8 categorias = total. Description/
/// location ficam de fora (mesma assimetria do #518: só existem em
/// `?detail=full`). Read-only, não exige WRITE+. 404 se evento não existe.
///
/// `?after=&before=` (sprint #520, composição de #517 + #519) restringe
/// o agregado a uma janela temporal half-open `[after, before)` —
/// reusa o filtro range do #517 ANTES do loop de contagem. Útil pra
/// dashboards "distribuição de overrides nesta semana" sem listar tudo
/// e agregar client-side. Ambos opcionais; sem nenhum ≡ shape original
/// do #519 (todos overrides entram). RECURRENCE-IDs não-parseáveis são
/// pulados silenciosamente quando algum bound é dado (consistente com
/// #517/#509). `total` reflete só items pós-filtro; `by_field` e
/// `by_category` agregam só sobre os filtrados (invariants preservadas:
/// `present + absent = total` por campo, soma das 8 categorias = total).
/// 400 se `after >= before`.
///
/// `?has_summary=&has_dtstart=&has_dtend=` (sprint #521, composição de
/// #518 + #519) restringe o agregado por presença qualitativa em AND —
/// reusa o filtro qualitativo do #518 logo APÓS o range retain do #520
/// (sequencial, não inline). Cada flag opcional independente; combinados
/// segmentam categorias semânticas. Útil pra agregar dentro de um subset
/// qualitativo (ex: "entre os que SÓ renomeiam, quantos por janela
/// temporal" via composição com `?after=&before=`). `description`/
/// `location` ficam de fora (mesma assimetria #518: só existem em
/// `?detail=full`). Invariants preservadas pós-filtro qualitativo igual
/// ao range. Sem nenhum dos 3 ≡ shape #520.
///
/// `tzid_breakdown: {"Europe/Berlin": N, "America/Sao_Paulo": M, ...}`
/// (sprint #538, paralelo do #530+#531 mas no overrides scope) agrega
/// ocorrências de TZID em DTSTART;TZID=… E DTEND;TZID=… dos overrides
/// retidos (pós-filtros range/qualitativos do #520/#521). Cada override
/// pode contribuir 0/1/2 TZIDs — DTSTART e DTEND são contados
/// independentemente; se ambos têm a MESMA TZID, breakdown[tz] += 2.
/// `tzid_token_count` reporta a soma total das counts (= total de tokens
/// TZID presentes nos overrides retidos), preservando a invariant
/// `sum(tzid_breakdown.values()) == tzid_token_count`. Pré-requisito do
/// futuro port do `tzid_breakdown_by_kind` (#536) — esta sprint introduz
/// SÓ a base, sem flags top_tzid/sort_tzid/min_count/by_kind. Implementado
/// via `Vec<(String, usize)>` association list (cardinalidade típica
/// baixa, lookup linear mais barato que hash). Read-only, NÃO requer
/// WRITE+. Sempre presente no payload mesmo que vazio (`{}`) — diferente
/// do exdates-preview-stats #530 que omite junto com `non_utc_by_kind`
/// porque overrides_stats não tem flag de inclusão equivalente.
///
/// `?top_tzid=N` (sprint #539, paralelo do #532+#533 mas no overrides
/// scope — primeira presentation flag depois da base #538) trunca o
/// `tzid_breakdown` pras N TZIDs mais frequentes (sort by count desc,
/// ties por insertion order do `Vec` association list — ordem de
/// aparição nos overrides retidos pós-filtro range/qualitativo do
/// #520/#521). Adiciona `tzid_other_count: usize` agregando soma das
/// counts das TZIDs descartadas. `top_tzid=0` → 400 ("must be >= 1");
/// ausência ≡ shape do #538 (breakdown completo, sem `tzid_other_count`).
/// Implementação clona o `Vec<(String, usize)>` (preserva insertion
/// order pra branch sem flag), `sort_by` desc, soma `iter().skip(n)`
/// pra `other_count`, `truncate(n)` no clone — O(N log N) com N=tzids
/// distintos (cardinalidade típica baixa em overrides). `tzid_other_count`
/// SÓ aparece quando `top_tzid.is_some()` (mesmo que `breakdown.len() <= n`
/// resultando em other=0 — UI sabe que flag foi aceita). Composto com
/// filtros range/qualitativos do #520/#521 (top_tzid atua APÓS a
/// agregação, sobre o breakdown já filtrado).
///
/// `?sort_tzid=count_desc|count_asc|name_asc|name_desc` (sprint #540,
/// paralelo do #534 mas no overrides scope) emite array adjacente
/// `tzid_breakdown_order: ["tz_a", "tz_b", ...]` indicando ordem custom
/// pro `tzid_breakdown`. Necessário porque `serde_json::Map` sem feature
/// `preserve_order` usa BTreeMap interno e serializa em ordem alfabética
/// determinística — único caminho pra transmitir ordem custom em JSON
/// Object é via array adjacente. Composto com `top_tzid` em duas fases:
/// (1) top-N selection por count desc preserva semantics do #539; (2)
/// re-ordena o set retido pelo sort_mode escolhido. `count_desc` ->
/// `b.1.cmp(&a.1)`, `count_asc` -> `a.1.cmp(&b.1)`, `name_asc` ->
/// `a.0.cmp(&b.0)`, `name_desc` -> `b.0.cmp(&a.0)`. `tzid_breakdown_order`
/// SOMENTE quando `sort_tzid.is_some()`; ausente preserva shape #539.
/// `top_tzid=10&sort_tzid=name_asc` = top-10 por count APRESENTADOS
/// alfabeticamente. Valor desconhecido -> 400 listando opções.
///
/// `?min_count=N` (sprint #541, paralelo do #535 mas no overrides scope)
/// filtra long-tail removendo do `tzid_breakdown` qualquer TZID com
/// count < N. Adiciona `tzid_filtered_count: usize` agregando soma das
/// counts das TZIDs descartadas pela cauda — dual filosófico do
/// `tzid_other_count` do #539 (top_tzid trunca CABEÇA por count desc;
/// min_count filtra LONG-TAIL — combinados oferecem janela arbitrária
/// no histograma como `min_count=2&top_tzid=10` = "top-10 entre TZIDs
/// com pelo menos 2 ocorrências cada"). `min_count=0` -> 400 ("must be
/// >= 1 (omit flag for full breakdown)"); `min_count=1` aceito mesmo
/// sendo no-op (toda TZID no breakdown apareceu pelo menos 1x —
/// primeira fronteira "real", UI pode emitir `tzid_filtered_count=0`
/// como confirmação). Composição em 3 fases ordem FIXA preservando
/// semantics intuitiva: (1) `min_count` filtra long-tail removendo
/// TZIDs raros do `Vec<(String, usize)>` original ANTES do truncate;
/// (2) `top_tzid` seleciona top-N por count desc do UNIVERSO FILTRADO
/// (compostável: `top_tzid=5&min_count=10` = "top-5 entre TZIDs com
/// count>=10"); (3) `sort_tzid` ordena set retido (presentation, do
/// #540) — chain `min_count -> top_tzid -> sort_tzid` é "filter
/// universe -> select head -> present". `tzid_filtered_count` SOMENTE
/// quando `min_count.is_some()`; ausência preserva shape #540.
/// Invariant pós-filter: `sum(tzid_breakdown.values()) +
/// tzid_other_count + tzid_filtered_count == tzid_token_count`.
///
/// `?include_kind_breakdown=true` (sprint #542, paralelo do #536 mas no
/// overrides scope — fechando o port das 5 famílias de flags em
/// overrides_stats: inclusão #538, head truncation #539, cosmetic
/// ordering #540, tail filtering #541, kind drill-down #542) emite
/// `tzid_breakdown_by_kind: {tz: {tzid: N, unknown: M}}` particionando
/// cada TZID retido em canonical (datetime local válido) vs malformed.
/// `is_canonical_local_datetime` valida `dtstart_value`/`dtend_value`
/// (capturados pelo parser estendido em sprint #542 — token pós-colon
/// de linhas `DTSTART;TZID=…:value`/`DTEND;…:value`, separado dos
/// campos `dtstart`/`dtend` que continuam refletindo SÓ linhas sem
/// params). Decoupling dos campos preserva 100% do `present(dtstart)`/
/// `present(dtend)` semantics usado pelo filtro qualitativo do #518/#521
/// — adicionar TZID a um override NÃO faz `has_dtstart=true` retornar
/// (semantics intencional: `has_dtstart` filtra "DTSTART canonical sem
/// params" do MVP, TZID é separado). Invariant: `canonical + malformed
/// == tzid_breakdown[tz]`. Aplicado APÓS chain completo de presentation
/// (`min_count` → `top_tzid` → `sort_tzid`) — só TZIDs em `kept` final
/// aparecem no objeto adjacente. Sem flag (None ou false) omite objeto.
/// `include_kind_breakdown=true` mas `kept` vazio (e.g. todos filtraram-
/// out) emite `{}` (semantics "flag aceita, sem dados").
async fn overrides_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((_cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<OverridesStatsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    if q.top_tzid == Some(0) {
        return Err(CalendarError::BadRequest(
            "top_tzid must be >= 1 (omit flag for full breakdown)".into()
        ));
    }
    if q.min_count == Some(0) {
        return Err(CalendarError::BadRequest(
            "min_count must be >= 1 (omit flag for full breakdown)".into()
        ));
    }
    let sort_mode: Option<&str> = match q.sort_tzid.as_deref() {
        None | Some("") => None,
        Some(s @ ("count_desc" | "count_asc" | "name_asc" | "name_desc")) => Some(s),
        Some(other) => return Err(CalendarError::BadRequest(format!(
            "sort_tzid must be one of count_desc|count_asc|name_asc|name_desc, got '{other}'"
        ))),
    };
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    let uid = extract_uid(&ev.ical_raw).unwrap_or_default();
    let mut items = list_recurrence_id_overrides(&ev.ical_raw, &uid, false);
    if q.after.is_some() || q.before.is_some() {
        items.retain(|item| {
            let compact = match item.get("compact").and_then(|v| v.as_str()) {
                Some(s) => s,
                None    => return false,
            };
            let parsed = match parse_one_exdate(compact) {
                Some(t) => t,
                None    => return false,
            };
            if let Some(a) = q.after  { if parsed <  a { return false; } }
            if let Some(b) = q.before { if parsed >= b { return false; } }
            true
        });
    }
    if q.has_summary.is_some() || q.has_dtstart.is_some() || q.has_dtend.is_some() {
        items.retain(|item| {
            let present = |key: &str| item.get(key).map(|v| !v.is_null()).unwrap_or(false);
            if let Some(want) = q.has_summary { if present("summary") != want { return false; } }
            if let Some(want) = q.has_dtstart { if present("dtstart") != want { return false; } }
            if let Some(want) = q.has_dtend   { if present("dtend")   != want { return false; } }
            true
        });
    }
    let total = items.len();
    let mut sum_p = 0usize; let mut sum_a = 0usize;
    let mut ds_p  = 0usize; let mut ds_a  = 0usize;
    let mut de_p  = 0usize; let mut de_a  = 0usize;
    let mut c_none = 0usize;
    let mut c_s    = 0usize;
    let mut c_ds   = 0usize;
    let mut c_de   = 0usize;
    let mut c_s_ds = 0usize;
    let mut c_s_de = 0usize;
    let mut c_ds_de = 0usize;
    let mut c_all  = 0usize;
    let mut tzid_breakdown: Vec<(String, usize)> = Vec::new();
    let include_kind_breakdown = q.include_kind_breakdown.unwrap_or(false);
    // (tzid, canonical_count, malformed_count) — populado só quando
    // include_kind_breakdown=true; mantido em paralelo a tzid_breakdown
    // pra preservar invariant `canonical + malformed == tzid_breakdown[tz]`
    // por construção (mesma chave incrementada em lockstep). Paralelo do
    // tzid_by_kind do #536 mas validando dtstart_value/dtend_value
    // capturados pelo parser estendido em sprint #542.
    let mut tzid_by_kind: Vec<(String, usize, usize)> = Vec::new();
    for item in &items {
        let present = |key: &str| item.get(key).map(|v| !v.is_null()).unwrap_or(false);
        let s  = present("summary");
        let ds = present("dtstart");
        let de = present("dtend");
        if s  { sum_p += 1; } else { sum_a += 1; }
        if ds { ds_p  += 1; } else { ds_a  += 1; }
        if de { de_p  += 1; } else { de_a  += 1; }
        match (s, ds, de) {
            (false, false, false) => c_none   += 1,
            (true,  false, false) => c_s      += 1,
            (false, true,  false) => c_ds     += 1,
            (false, false, true ) => c_de     += 1,
            (true,  true,  false) => c_s_ds   += 1,
            (true,  false, true ) => c_s_de   += 1,
            (false, true,  true ) => c_ds_de  += 1,
            (true,  true,  true ) => c_all    += 1,
        }
        for (tzid_key, value_key) in [
            ("dtstart_tzid", "dtstart_value"),
            ("dtend_tzid",   "dtend_value"),
        ] {
            if let Some(tz) = item.get(tzid_key).and_then(|v| v.as_str()) {
                if !tz.is_empty() {
                    match tzid_breakdown.iter().position(|(k, _)| k == tz) {
                        Some(i) => tzid_breakdown[i].1 += 1,
                        None    => tzid_breakdown.push((tz.to_string(), 1)),
                    }
                    if include_kind_breakdown {
                        let canonical = item.get(value_key)
                            .and_then(|v| v.as_str())
                            .map(is_canonical_local_datetime)
                            .unwrap_or(false);
                        match tzid_by_kind.iter().position(|(k, _, _)| k == tz) {
                            Some(i) => {
                                if canonical { tzid_by_kind[i].1 += 1; }
                                else         { tzid_by_kind[i].2 += 1; }
                            }
                            None => {
                                let (c, u) = if canonical { (1, 0) } else { (0, 1) };
                                tzid_by_kind.push((tz.to_string(), c, u));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut tzid_token_count = 0usize;
    for (_, c) in &tzid_breakdown {
        tzid_token_count += *c;
    }
    let (filtered_universe, tzid_filtered_count) = if let Some(n) = q.min_count {
        let removed: usize = tzid_breakdown.iter()
            .filter(|(_, c)| *c < n)
            .map(|(_, c)| *c)
            .sum();
        let kept: Vec<(String, usize)> = tzid_breakdown.into_iter()
            .filter(|(_, c)| *c >= n)
            .collect();
        (kept, Some(removed))
    } else {
        (tzid_breakdown, None)
    };
    let (mut kept, tzid_other_count) = if let Some(n) = q.top_tzid {
        let mut sorted = filtered_universe.clone();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        let other: usize = sorted.iter().skip(n).map(|(_, c)| *c).sum();
        sorted.truncate(n);
        (sorted, Some(other))
    } else {
        (filtered_universe, None)
    };
    if let Some(mode) = sort_mode {
        match mode {
            "count_desc" => kept.sort_by(|a, b| b.1.cmp(&a.1)),
            "count_asc"  => kept.sort_by(|a, b| a.1.cmp(&b.1)),
            "name_asc"   => kept.sort_by(|a, b| a.0.cmp(&b.0)),
            "name_desc"  => kept.sort_by(|a, b| b.0.cmp(&a.0)),
            _ => unreachable!(),
        }
    }
    let mut breakdown_obj = serde_json::Map::new();
    for (tz, c) in &kept {
        breakdown_obj.insert(tz.clone(), serde_json::json!(c));
    }
    let mut payload = serde_json::json!({
        "event_id": ev.id,
        "total":    total,
        "by_field": {
            "summary": { "present": sum_p, "absent": sum_a },
            "dtstart": { "present": ds_p,  "absent": ds_a  },
            "dtend":   { "present": de_p,  "absent": de_a  },
        },
        "by_category": {
            "none":           c_none,
            "only_summary":   c_s,
            "only_dtstart":   c_ds,
            "only_dtend":     c_de,
            "summary_dtstart": c_s_ds,
            "summary_dtend":   c_s_de,
            "dtstart_dtend":   c_ds_de,
            "all_three":       c_all,
        },
        "tzid_breakdown":   serde_json::Value::Object(breakdown_obj),
        "tzid_token_count": tzid_token_count,
    });
    if let Some(other) = tzid_other_count {
        payload.as_object_mut().unwrap().insert(
            "tzid_other_count".into(),
            serde_json::json!(other),
        );
    }
    if let Some(filtered) = tzid_filtered_count {
        payload.as_object_mut().unwrap().insert(
            "tzid_filtered_count".into(),
            serde_json::json!(filtered),
        );
    }
    if sort_mode.is_some() {
        let order: Vec<serde_json::Value> = kept.iter()
            .map(|(tz, _)| serde_json::Value::String(tz.clone()))
            .collect();
        payload.as_object_mut().unwrap().insert(
            "tzid_breakdown_order".into(),
            serde_json::Value::Array(order),
        );
    }
    if include_kind_breakdown {
        let mut by_kind_obj = serde_json::Map::new();
        for (tz, _) in &kept {
            let (c, u) = tzid_by_kind.iter()
                .find(|(k, _, _)| k == tz)
                .map(|(_, c, u)| (*c, *u))
                .unwrap_or((0, 0));
            by_kind_obj.insert(tz.clone(), serde_json::json!({
                "tzid":    c,
                "unknown": u,
            }));
        }
        payload.as_object_mut().unwrap().insert(
            "tzid_breakdown_by_kind".into(),
            serde_json::Value::Object(by_kind_obj),
        );
    }
    Ok(Json(payload))
}

/// GET /api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id —
/// snapshot completo de UM override (sprint #500, complemento de #495
/// create + #496 list + #497 delete + #498 patch). `:recurrence_id`
/// aceita compact `YYYYMMDDTHHMMSSZ` ou RFC3339; canonicalizado pra UTC
/// compact antes do match. Retorna `{event_id, recurrence_id, rfc3339,
/// summary, description, location, dtstart, dtend, dtstamp, sequence}`.
/// Diferente do list (#496) que omite description, esta GET é o snapshot
/// completo pra editor pré-popular form. ETag/Last-Modified herdados do
/// master event (mesma origem do #get_one principal: qualquer mutação no
/// VCALENDAR refresh do master); 304 em If-None-Match/If-Modified-Since.
/// Read-only, não exige WRITE+. 404 se override não existe.
async fn get_one_override(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id, recurrence_id)): Path<(Uuid, Uuid, String)>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }

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

    let target = parse_one_exdate(&recurrence_id).ok_or_else(|| CalendarError::BadRequest(
        format!("recurrence_id must be RFC3339 or YYYYMMDDTHHMMSSZ, got {recurrence_id}")
    ))?;
    let target_compact = format_compact_utc(target);

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate override".into()
    ))?;

    let snap = pick_recurrence_id_override(&ev.ical_raw, &uid, &target_compact)
        .ok_or(CalendarError::EventNotFound(id))?;

    use time::format_description::FormatItem;
    use time::macros::format_description;
    static FMT: &[FormatItem<'static>] = format_description!(
        "[year][month][day]T[hour][minute][second]Z"
    );
    let rfc = OffsetDateTime::parse(&target_compact, &FMT).ok().and_then(|t| {
        t.format(&time::format_description::well_known::Rfc3339).ok()
    });

    let body = serde_json::json!({
        "event_id":      ev.id,
        "recurrence_id": target_compact,
        "rfc3339":       rfc,
        "summary":       snap.summary,
        "description":   snap.description,
        "location":      snap.location,
        "dtstart":       snap.dtstart,
        "dtend":         snap.dtend,
        "dtstamp":       snap.dtstamp,
        "sequence":      ev.sequence,
    });

    let mut resp = Json(body).into_response();
    resp.headers_mut().insert(header::ETAG,          HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

/// Snapshot dos campos do VEVENT override pareando UID + RECURRENCE-ID.
/// Strings raw direto do ical (não unescaped) — cliente decide; usado
/// pelo GET single (#500) que precisa de description também (list #496
/// só traz summary/dtstart/dtend).
struct OverrideSnapshot {
    summary:     Option<String>,
    description: Option<String>,
    location:    Option<String>,
    dtstart:     Option<String>,
    dtend:       Option<String>,
    dtstamp:     Option<String>,
}

fn pick_recurrence_id_override(raw: &str, uid_master: &str, target_compact: &str) -> Option<OverrideSnapshot> {
    let mut in_event   = false;
    let mut found_uid  = false;
    let mut found_rec  = false;
    let mut snap = OverrideSnapshot {
        summary: None, description: None, location: None,
        dtstart: None, dtend: None, dtstamp: None,
    };

    for line in raw.lines() {
        let trimmed = line.trim_start();
        let head: String = trimmed.chars().take(16).collect::<String>().to_ascii_uppercase();

        if head.starts_with("BEGIN:VEVENT") {
            in_event = true;
            found_uid = false;
            found_rec = false;
            snap = OverrideSnapshot {
                summary: None, description: None, location: None,
                dtstart: None, dtend: None, dtstamp: None,
            };
            continue;
        }
        if head.starts_with("END:VEVENT") {
            if in_event && found_uid && found_rec {
                return Some(snap);
            }
            in_event = false;
            continue;
        }
        if !in_event { continue; }

        if head.starts_with("UID:") {
            let v = trimmed["UID:".len()..].trim();
            if v == uid_master { found_uid = true; }
        } else if head.starts_with("RECURRENCE-ID:") {
            let v = trimmed["RECURRENCE-ID:".len()..].trim();
            if v == target_compact { found_rec = true; }
        } else if head.starts_with("SUMMARY:") {
            snap.summary = Some(trimmed["SUMMARY:".len()..].trim().to_string());
        } else if head.starts_with("DESCRIPTION:") {
            snap.description = Some(trimmed["DESCRIPTION:".len()..].trim().to_string());
        } else if head.starts_with("LOCATION:") {
            snap.location = Some(trimmed["LOCATION:".len()..].trim().to_string());
        } else if head.starts_with("DTSTART:") {
            snap.dtstart = Some(trimmed["DTSTART:".len()..].trim().to_string());
        } else if head.starts_with("DTEND:") {
            snap.dtend = Some(trimmed["DTEND:".len()..].trim().to_string());
        } else if head.starts_with("DTSTAMP:") {
            snap.dtstamp = Some(trimmed["DTSTAMP:".len()..].trim().to_string());
        }
    }
    None
}

/// POST /api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id/cancel —
/// migra um RECURRENCE-ID override pra EXDATE cancellation num único call
/// (sprint #501, substitui workflow 2-passos do #499). Equivale a:
/// (1) DELETE /overrides/:recurrence_id seguido de
/// (2) POST /cancel-instance {instance: <recurrence_id>}.
/// Vantagem: 1 só DB write + sequence bumpa 1 vez (em vez de 2) + atômico
/// no nível ICS (se algo falha entre os 2 passos, ficaria estado
/// transitório). Útil pra UI "mudei de ideia, na verdade cancela essa
/// occurrence em vez de overridar". `:recurrence_id` aceita compact
/// `YYYYMMDDTHHMMSSZ` ou RFC3339; canonicalizado pra UTC compact antes do
/// match. Requer WRITE+. 404 se override não existe (fail-loud, paralelo
/// ao #500 GET). 400 se evento não tem rrule (sem rrule não há série pra
/// cancelar instance via EXDATE — paralelo ao #488). 409 se EXDATE
/// pra mesma instance já existe — situação anômala (deveria ter sido
/// bloqueada pelos pre-checks #499) mas validação defensiva.
/// Retorna `{event_id, recurrence_id, removed_override:true,
/// added_exdate:true, sequence}`.
async fn migrate_override_to_cancel(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id, recurrence_id)): Path<(Uuid, Uuid, String)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }
    if ev.rrule.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
        return Err(CalendarError::BadRequest(
            "event has no rrule — overrides only apply to recurring series".into()
        ));
    }

    let target = parse_one_exdate(&recurrence_id).ok_or_else(|| CalendarError::BadRequest(
        format!("recurrence_id must be RFC3339 or YYYYMMDDTHHMMSSZ, got {recurrence_id}")
    ))?;
    let target_compact = format_compact_utc(target);

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate override".into()
    ))?;

    if !has_recurrence_id_override(&ev.ical_raw, &uid, &target_compact) {
        return Err(CalendarError::EventNotFound(id));
    }

    let target_utc = target.to_offset(time::UtcOffset::UTC);
    let already_exdated: bool = parse_exdates(&ev.ical_raw)
        .iter()
        .any(|t| t.to_offset(time::UtcOffset::UTC) == target_utc);
    if already_exdated {
        return Err(CalendarError::Conflict(
            format!("instance {target_compact} already has EXDATE — \
                     override and EXDATE coexisting is anomalous; \
                     remove via DELETE /overrides/:recurrence_id alone")
        ));
    }

    let without_override = remove_recurrence_id_override_block(&ev.ical_raw, &uid, &target_compact);
    let new_line = format!("EXDATE:{target_compact}");
    let new_raw = inject_exdate_line(&without_override, &new_line);

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":         ev.id,
        "recurrence_id":    target_compact,
        "removed_override": true,
        "added_exdate":     true,
        "sequence":         updated.sequence,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct MigrateCancelToOverrideBody {
    summary:     Option<String>,
    description: Option<String>,
    location:    Option<String>,
    /// Se omitido, override mantém dtstart=instance original (a EXDATE alvo).
    #[serde(default, with = "time::serde::rfc3339::option")]
    dtstart:     Option<OffsetDateTime>,
    /// Se omitido, override não emite DTEND (cliente herda do master).
    #[serde(default, with = "time::serde::rfc3339::option")]
    dtend:       Option<OffsetDateTime>,
}

/// POST /api/v1/calendars/:cal_id/events/:id/exdates/:instance/override —
/// migra um EXDATE pra RECURRENCE-ID override num único call (sprint #502,
/// inverso simétrico do #501). Equivale a:
/// (1) DELETE /exdates/:instance seguido de
/// (2) POST /override-instance {instance: <inst>, ...}.
/// Vantagem: 1 só DB write + sequence bumpa 1 vez + atomicidade ICS (sem
/// estado transitório onde a instância nem é cancelled nem overridden).
/// Útil pra "mudei de ideia, na verdade quero customizar essa occurrence
/// em vez de cancelar". `:instance` aceita compact `YYYYMMDDTHHMMSSZ` ou
/// RFC3339; canonicalizado pra UTC compact antes do match. Body permite
/// summary/description/location/dtstart/dtend (pelo menos 1 obrigatório —
/// override no-op não faz sentido). Requer WRITE+. 404 se EXDATE pra
/// instance não existe (fail-loud, paralelo ao #501). 400 se evento não
/// tem rrule. 409 se RECURRENCE-ID override pra mesma instance já existe
/// (anômalo dado pre-checks #499 mas validação defensiva). Retorna
/// `{event_id, recurrence_id, removed_exdate:true, added_override:true,
/// sequence}`.
async fn migrate_cancel_to_override(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id, instance)): Path<(Uuid, Uuid, String)>,
    Json(body):   Json<MigrateCancelToOverrideBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }
    if ev.rrule.as_deref().map(|s| s.trim().is_empty()).unwrap_or(true) {
        return Err(CalendarError::BadRequest(
            "event has no rrule — overrides only apply to recurring series".into()
        ));
    }
    if body.summary.is_none() && body.description.is_none()
        && body.location.is_none() && body.dtstart.is_none() && body.dtend.is_none()
    {
        return Err(CalendarError::BadRequest(
            "at least one of summary/description/location/dtstart/dtend required".into()
        ));
    }

    let target = parse_one_exdate(&instance).ok_or_else(|| CalendarError::BadRequest(
        format!("instance must be RFC3339 or YYYYMMDDTHHMMSSZ, got {instance}")
    ))?;
    let target_utc = target.to_offset(time::UtcOffset::UTC);
    let target_compact = format_compact_utc(target_utc);

    let exists: bool = parse_exdates(&ev.ical_raw)
        .iter()
        .any(|t| t.to_offset(time::UtcOffset::UTC) == target_utc);
    if !exists {
        return Err(CalendarError::EventNotFound(id));
    }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot create override".into()
    ))?;

    if has_recurrence_id_override(&ev.ical_raw, &uid, &target_compact) {
        return Err(CalendarError::Conflict(
            format!("override for RECURRENCE-ID:{target_compact} already exists — \
                     EXDATE and override coexisting is anomalous; \
                     remove via DELETE /exdates/:instance alone")
        ));
    }

    let without_exdate = remove_exdate_value(&ev.ical_raw, target_utc);

    let dtstart = body.dtstart.unwrap_or(target_utc).to_offset(time::UtcOffset::UTC);
    let mut block = String::new();
    let eol = if without_exdate.contains("\r\n") { "\r\n" } else { "\n" };

    block.push_str("BEGIN:VEVENT");                block.push_str(eol);
    block.push_str(&format!("UID:{uid}"));          block.push_str(eol);
    block.push_str(&format!("RECURRENCE-ID:{target_compact}")); block.push_str(eol);
    block.push_str(&format!("DTSTAMP:{}", format_compact_utc(OffsetDateTime::now_utc())));
    block.push_str(eol);
    block.push_str(&format!("DTSTART:{}", format_compact_utc(dtstart))); block.push_str(eol);
    if let Some(end) = body.dtend {
        block.push_str(&format!("DTEND:{}", format_compact_utc(end.to_offset(time::UtcOffset::UTC))));
        block.push_str(eol);
    }
    if let Some(s) = body.summary.as_deref() {
        block.push_str(&format!("SUMMARY:{}", escape_ics_text(s))); block.push_str(eol);
    }
    if let Some(s) = body.description.as_deref() {
        block.push_str(&format!("DESCRIPTION:{}", escape_ics_text(s))); block.push_str(eol);
    }
    if let Some(s) = body.location.as_deref() {
        block.push_str(&format!("LOCATION:{}", escape_ics_text(s))); block.push_str(eol);
    }
    block.push_str("END:VEVENT"); block.push_str(eol);

    let new_raw = inject_before_end_vcalendar(&without_exdate, &block);

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":       ev.id,
        "recurrence_id":  target_compact,
        "removed_exdate": true,
        "added_override": true,
        "sequence":       updated.sequence,
    })))
}

/// DELETE /api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id —
/// remove o VEVENT override correspondente (sprint #497, inverso de #495).
/// `:recurrence_id` aceita compact `YYYYMMDDTHHMMSSZ` ou RFC3339;
/// canonicalizado pra UTC compact antes do match. Idempotente: `{removed:
/// false}` se não existir. Requer WRITE+. Reescreve o ical_raw removendo
/// o bloco BEGIN:VEVENT...END:VEVENT cujo UID == master e
/// RECURRENCE-ID == alvo.
async fn delete_override(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id, recurrence_id)): Path<(Uuid, Uuid, String)>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id {
        return Err(CalendarError::EventNotFound(id));
    }

    let target = parse_one_exdate(&recurrence_id).ok_or_else(|| CalendarError::BadRequest(
        format!("recurrence_id must be RFC3339 or YYYYMMDDTHHMMSSZ, got {recurrence_id}")
    ))?;
    let target_compact = format_compact_utc(target);

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate override".into()
    ))?;

    if !has_recurrence_id_override(&ev.ical_raw, &uid, &target_compact) {
        return Ok(Json(serde_json::json!({
            "event_id":      ev.id,
            "recurrence_id": target_compact,
            "removed":       false,
        })));
    }

    let new_raw = remove_recurrence_id_override_block(&ev.ical_raw, &uid, &target_compact);
    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":      ev.id,
        "recurrence_id": target_compact,
        "removed":       true,
        "sequence":      updated.sequence,
    })))
}

/// Remove o bloco VEVENT (BEGIN:VEVENT..END:VEVENT inclusivos) cujo UID
/// confere com `uid_master` e RECURRENCE-ID confere com `target_compact`.
/// Faz buffering por bloco: acumula linhas de um VEVENT, decide ao bater
/// END:VEVENT se descarta ou flushea. Linhas fora de VEVENT (BEGIN:
/// VCALENDAR, master event antes deste, etc.) passam direto.
fn remove_recurrence_id_override_block(raw: &str, uid_master: &str, target_compact: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut buf = String::new();
    let mut in_event = false;
    let mut found_uid = false;
    let mut found_recid_match = false;

    for src_line in raw.split_inclusive('\n') {
        let trimmed = src_line.trim_start();
        let upper14: String = trimmed.chars().take(14).collect::<String>().to_ascii_uppercase();

        if upper14.starts_with("BEGIN:VEVENT") {
            in_event = true;
            found_uid = false;
            found_recid_match = false;
            buf.clear();
            buf.push_str(src_line);
            continue;
        }

        if !in_event {
            out.push_str(src_line);
            continue;
        }

        if upper14.starts_with("END:VEVENT") {
            buf.push_str(src_line);
            if !(found_uid && found_recid_match) {
                out.push_str(&buf);
            }
            in_event = false;
            buf.clear();
            continue;
        }

        let upper4:  String = trimmed.chars().take(4).collect::<String>().to_ascii_uppercase();
        if upper4.starts_with("UID:") {
            let v = trimmed["UID:".len()..].trim();
            if v == uid_master { found_uid = true; }
        } else if upper14.starts_with("RECURRENCE-ID:") {
            let v = trimmed["RECURRENCE-ID:".len()..].trim();
            if v == target_compact { found_recid_match = true; }
        }
        buf.push_str(src_line);
    }

    if !buf.is_empty() {
        out.push_str(&buf);
    }
    out
}

/// Walk pelos blocos VEVENT do raw retornando os RECURRENCE-IDs cujo UID
/// confere com `uid_master`. Para cada match coleta SUMMARY/DTSTART/DTEND
/// opcionais pra UI exibir; com `full=true` (sprint #503) adiciona também
/// DESCRIPTION/LOCATION pra paridade com get-one (#500). compact = valor
/// original do RECURRENCE-ID (preservado); rfc3339 = canonicalizado se
/// parseável como UTC compact, senão null.
fn list_recurrence_id_overrides(raw: &str, uid_master: &str, full: bool) -> Vec<serde_json::Value> {
    use time::format_description::FormatItem;
    use time::macros::format_description;
    static FMT: &[FormatItem<'static>] = format_description!(
        "[year][month][day]T[hour][minute][second]Z"
    );

    // Extrai TZID de uma linha tipo `DTSTART;TZID=Europe/Berlin;X=Y:value` —
    // retorna `Some("Europe/Berlin")` (case-preserved) ou `None` se linha
    // não tem param TZID. Recebe o trecho ENTRE `DTSTART` e `:` (sem prefixo
    // do property-name, sem o value pós-colon).
    fn parse_tzid_from_params(params_segment: &str) -> Option<String> {
        for kv in params_segment.split(';').filter(|s| !s.is_empty()) {
            let upper = kv.to_ascii_uppercase();
            if let Some(rest) = upper.strip_prefix("TZID=") {
                let take = rest.len();
                let original = &kv[kv.len()-take..];
                let v = original.trim().trim_matches('"');
                if !v.is_empty() { return Some(v.to_string()); }
            }
        }
        None
    }

    let mut out = Vec::new();
    let mut in_event = false;
    let mut found_uid = false;
    let mut cur_recid:        Option<String> = None;
    let mut cur_summary:      Option<String> = None;
    let mut cur_dtstart:      Option<String> = None;
    let mut cur_dtend:        Option<String> = None;
    let mut cur_dtstart_tzid: Option<String> = None;
    let mut cur_dtend_tzid:   Option<String> = None;
    // Sprint #542 — `*_value` capturam o token pós-colon de linhas
    // `DTSTART;TZID=…:value` / `DTEND;TZID=…:value`, separado de
    // `dtstart`/`dtend` (que continuam refletindo SÓ linhas sem params,
    // preservando semantics do has_dtstart/has_dtend de #518/#521). Usado
    // pelo `tzid_breakdown_by_kind` em overrides_stats pra validar
    // canonicalidade do datetime local sem alterar filtros qualitativos.
    let mut cur_dtstart_value: Option<String> = None;
    let mut cur_dtend_value:   Option<String> = None;
    let mut cur_description:  Option<String> = None;
    let mut cur_location:     Option<String> = None;

    for line in raw.lines() {
        let trimmed = line.trim_start();
        let upper16: String = trimmed.chars().take(16).collect::<String>().to_ascii_uppercase();
        if upper16.starts_with("BEGIN:VEVENT") {
            in_event = true;
            found_uid = false;
            cur_recid = None; cur_summary = None; cur_dtstart = None; cur_dtend = None;
            cur_dtstart_tzid = None; cur_dtend_tzid = None;
            cur_dtstart_value = None; cur_dtend_value = None;
            cur_description = None; cur_location = None;
            continue;
        }
        if upper16.starts_with("END:VEVENT") {
            if in_event && found_uid {
                if let Some(rec) = cur_recid.take() {
                    let rfc = OffsetDateTime::parse(rec.trim(), &FMT).ok().and_then(|t| {
                        t.format(&time::format_description::well_known::Rfc3339).ok()
                    });
                    let mut item = serde_json::json!({
                        "compact":       rec,
                        "rfc3339":       rfc,
                        "summary":       cur_summary.take(),
                        "dtstart":       cur_dtstart.take(),
                        "dtend":         cur_dtend.take(),
                        "dtstart_tzid":  cur_dtstart_tzid.take(),
                        "dtend_tzid":    cur_dtend_tzid.take(),
                        "dtstart_value": cur_dtstart_value.take(),
                        "dtend_value":   cur_dtend_value.take(),
                    });
                    if full {
                        let obj = item.as_object_mut().expect("json object");
                        obj.insert("description".into(),
                            cur_description.take().map(serde_json::Value::String)
                                .unwrap_or(serde_json::Value::Null));
                        obj.insert("location".into(),
                            cur_location.take().map(serde_json::Value::String)
                                .unwrap_or(serde_json::Value::Null));
                    }
                    out.push(item);
                }
            }
            in_event = false;
            continue;
        }
        if !in_event { continue; }

        if upper16.starts_with("UID:") {
            let v = trimmed["UID:".len()..].trim();
            if v == uid_master { found_uid = true; }
        } else if upper16.starts_with("RECURRENCE-ID:") {
            cur_recid = Some(trimmed["RECURRENCE-ID:".len()..].trim().to_string());
        } else if upper16.starts_with("SUMMARY:") {
            cur_summary = Some(trimmed["SUMMARY:".len()..].trim().to_string());
        } else if upper16.starts_with("DTSTART:") {
            cur_dtstart = Some(trimmed["DTSTART:".len()..].trim().to_string());
        } else if upper16.starts_with("DTEND:") {
            cur_dtend = Some(trimmed["DTEND:".len()..].trim().to_string());
        } else if upper16.starts_with("DTSTART;") {
            if let Some(colon_pos) = trimmed.find(':') {
                let params = &trimmed["DTSTART".len()..colon_pos];
                let params = params.strip_prefix(';').unwrap_or(params);
                cur_dtstart_tzid = parse_tzid_from_params(params);
                cur_dtstart_value = Some(trimmed[colon_pos+1..].trim().to_string());
            }
        } else if upper16.starts_with("DTEND;") {
            if let Some(colon_pos) = trimmed.find(':') {
                let params = &trimmed["DTEND".len()..colon_pos];
                let params = params.strip_prefix(';').unwrap_or(params);
                cur_dtend_tzid = parse_tzid_from_params(params);
                cur_dtend_value = Some(trimmed[colon_pos+1..].trim().to_string());
            }
        } else if full && upper16.starts_with("DESCRIPTION:") {
            cur_description = Some(trimmed["DESCRIPTION:".len()..].trim().to_string());
        } else if full && upper16.starts_with("LOCATION:") {
            cur_location = Some(trimmed["LOCATION:".len()..].trim().to_string());
        }
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct PatchOverrideBody {
    summary:     Option<String>,
    description: Option<String>,
    location:    Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    dtstart:     Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    dtend:       Option<OffsetDateTime>,
}

/// PATCH /api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id —
/// edita campos do VEVENT override existente sem recriar (sprint #498,
/// complemento de #495 create + #496 list + #497 delete). Body permite
/// summary/description/location/dtstart/dtend; pelo menos 1 obrigatório.
/// Campos ausentes preservam valor atual; presentes substituem (ou
/// inserem se não existiam). DTSTAMP é refrescado pra agora (RFC 5545
/// recomendado em qualquer mutação). UID/RECURRENCE-ID nunca tocados.
/// 404 se override não existe (mesma checagem do delete).
async fn patch_override(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id, recurrence_id)): Path<(Uuid, Uuid, String)>,
    Json(body):   Json<PatchOverrideBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    if body.summary.is_none() && body.description.is_none()
        && body.location.is_none() && body.dtstart.is_none() && body.dtend.is_none()
    {
        return Err(CalendarError::BadRequest(
            "at least one of summary/description/location/dtstart/dtend required".into()
        ));
    }

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let target = parse_one_exdate(&recurrence_id).ok_or_else(|| CalendarError::BadRequest(
        format!("invalid recurrence_id `{recurrence_id}` — expected RFC3339 or YYYYMMDDTHHMMSSZ")
    ))?;
    let target_compact = format_compact_utc(target);

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate override".into()
    ))?;

    if !has_recurrence_id_override(&ev.ical_raw, &uid, &target_compact) {
        return Err(CalendarError::EventNotFound(id));
    }

    let dtstart_str = body.dtstart.map(|d| format_compact_utc(d.to_offset(time::UtcOffset::UTC)));
    let dtend_str   = body.dtend.map(|d| format_compact_utc(d.to_offset(time::UtcOffset::UTC)));
    let dtstamp_now = format_compact_utc(OffsetDateTime::now_utc());

    let new_raw = patch_recurrence_id_override_block(
        &ev.ical_raw, &uid, &target_compact,
        body.summary.as_deref(),
        body.description.as_deref(),
        body.location.as_deref(),
        dtstart_str.as_deref(),
        dtend_str.as_deref(),
        &dtstamp_now,
    );

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":      ev.id,
        "recurrence_id": target_compact,
        "patched":       true,
        "sequence":      updated.sequence,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct TouchSingleQuery {
    /// `?dry=true` retorna o plano sem aplicar (sprint #513). Default false.
    /// Compartilhada entre `touch_override` (#505) e `touch_master` (#506).
    dry: Option<bool>,
}

/// POST /api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id/touch —
/// refresca SÓ o DTSTAMP do VEVENT override sem alterar nenhum campo
/// (sprint #505, complemento do quinteto CRUD #495/#496/#497/#498/#500).
/// Use case: forçar re-sync em clients iCal que cacheiam por DTSTAMP
/// (CalDAV/Apple Calendar/Outlook) sem mexer em payload visível pro usuário.
/// Implementado como `patch_recurrence_id_override_block` com TODOS os
/// campos None — só o DTSTAMP é reescrito pra agora. Como o DTSTAMP do
/// override não está nas colunas comparadas pelo `EventRepo::update`
/// (summary/location/dtstart/dtend/rrule/status/organizer do MASTER), o
/// `sequence` permanece igual — mas o ETag do master é recomputado e o
/// `updated_at` é refrescado, o que basta pra invalidar caches HTTP +
/// CalDAV (ETag-based). Sem body. 404 se override não existe; 400 se
/// recurrence_id mal-formado. Requer WRITE+. Retorna `{event_id,
/// recurrence_id, touched:true, dtstamp, etag, sequence}`.
async fn touch_override(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id, recurrence_id)): Path<(Uuid, Uuid, String)>,
    Query(q):     Query<TouchSingleQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let target = parse_one_exdate(&recurrence_id).ok_or_else(|| CalendarError::BadRequest(
        format!("invalid recurrence_id `{recurrence_id}` — expected RFC3339 or YYYYMMDDTHHMMSSZ")
    ))?;
    let target_compact = format_compact_utc(target);

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate override".into()
    ))?;

    if !has_recurrence_id_override(&ev.ical_raw, &uid, &target_compact) {
        return Err(CalendarError::EventNotFound(id));
    }

    if q.dry.unwrap_or(false) {
        return Ok(Json(serde_json::json!({
            "dry":           true,
            "event_id":      ev.id,
            "recurrence_id": target_compact,
            "touched":       true,
        })));
    }

    let dtstamp_now = format_compact_utc(OffsetDateTime::now_utc());

    let new_raw = patch_recurrence_id_override_block(
        &ev.ical_raw, &uid, &target_compact,
        None, None, None, None, None,
        &dtstamp_now,
    );

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":      ev.id,
        "recurrence_id": target_compact,
        "touched":       true,
        "dtstamp":       dtstamp_now,
        "etag":          updated.etag,
        "sequence":      updated.sequence,
    })))
}

/// POST /api/v1/calendars/:cal_id/events/:id/touch — refresca SÓ o DTSTAMP
/// do VEVENT MASTER sem alterar nenhum campo (sprint #506, paralelo do
/// `/overrides/:recurrence_id/touch` do #505 mas no master ao invés de
/// override). Use case: forçar re-sync em clients iCal que cacheiam por
/// DTSTAMP do master (CalDAV/Apple Calendar/Outlook) sem mexer em payload
/// visível pro usuário — útil pra "ressuscitar" eventos cujos clients
/// pararam de sincronizar por bug ou após restore de backup.
/// Master = bloco VEVENT que tem UID==master mas SEM RECURRENCE-ID
/// (overrides têm RECURRENCE-ID:<dtstamp>). Como o DTSTAMP não está nas
/// colunas comparadas pelo `EventRepo::update`, o `sequence` permanece
/// igual — mas o ETag é recomputado e o `updated_at` é refrescado, o que
/// basta pra invalidar caches HTTP + CalDAV (ETag-based). Sem body. 400 se
/// master sem UID. Requer WRITE+. Retorna `{event_id, touched:true,
/// dtstamp, etag, sequence}`.
async fn touch_master(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<TouchSingleQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate master block".into()
    ))?;

    if q.dry.unwrap_or(false) {
        return Ok(Json(serde_json::json!({
            "dry":      true,
            "event_id": ev.id,
            "touched":  true,
        })));
    }

    let dtstamp_now = format_compact_utc(OffsetDateTime::now_utc());
    let new_raw = patch_master_dtstamp(&ev.ical_raw, &uid, &dtstamp_now);

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &new_raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id": ev.id,
        "touched":  true,
        "dtstamp":  dtstamp_now,
        "etag":     updated.etag,
        "sequence": updated.sequence,
    })))
}

/// Reescreve apenas o DTSTAMP do bloco VEVENT MASTER (UID==`uid_master` E
/// SEM linha RECURRENCE-ID). Outros blocos VEVENT (overrides com mesmo UID
/// + RECURRENCE-ID) preservados intactos. Se DTSTAMP não existe no master,
/// adiciona antes de END:VEVENT.
fn patch_master_dtstamp(raw: &str, uid_master: &str, dtstamp_now: &str) -> String {
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(raw.len() + 64);
    let mut buf: Vec<String> = Vec::new();
    let mut in_event = false;
    let mut found_uid = false;
    let mut has_recid = false;

    for src_line in raw.split_inclusive('\n') {
        let trimmed = src_line.trim_start();
        let upper14: String = trimmed.chars().take(14).collect::<String>().to_ascii_uppercase();

        if upper14.starts_with("BEGIN:VEVENT") {
            in_event = true;
            found_uid = false;
            has_recid = false;
            buf.clear();
            buf.push(src_line.to_string());
            continue;
        }

        if !in_event {
            out.push_str(src_line);
            continue;
        }

        if upper14.starts_with("END:VEVENT") {
            if found_uid && !has_recid {
                let mut had_dtstamp = false;
                for line in &buf {
                    let head: String = line.trim_start().chars().take(8)
                        .collect::<String>().to_ascii_uppercase();
                    if head.starts_with("DTSTAMP:") {
                        out.push_str(&format!("DTSTAMP:{dtstamp_now}"));
                        out.push_str(eol);
                        had_dtstamp = true;
                    } else {
                        out.push_str(line);
                    }
                }
                if !had_dtstamp {
                    out.push_str(&format!("DTSTAMP:{dtstamp_now}"));
                    out.push_str(eol);
                }
                out.push_str(src_line);
            } else {
                for line in &buf { out.push_str(line); }
                out.push_str(src_line);
            }
            in_event = false;
            buf.clear();
            continue;
        }

        let upper4: String = trimmed.chars().take(4).collect::<String>().to_ascii_uppercase();
        if upper4.starts_with("UID:") {
            let v = trimmed["UID:".len()..].trim();
            if v == uid_master { found_uid = true; }
        } else if upper14.starts_with("RECURRENCE-ID") {
            // RECURRENCE-ID: ou RECURRENCE-ID;TZID=…: — ambos marcam override
            has_recid = true;
        }
        buf.push(src_line.to_string());
    }

    if !buf.is_empty() {
        for line in &buf { out.push_str(line); }
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct TouchOverridesBulkBody {
    instances: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct TouchOverridesBulkQuery {
    /// `?dry=true` retorna o plano sem aplicar (sprint #511). Default false.
    dry: Option<bool>,
}

/// POST /api/v1/calendars/:cal_id/events/:id/touch-overrides — bulk variant
/// do #505 (sprint #507). Body `{"instances":["20260601T120000Z",…]}` toca
/// DTSTAMP de N overrides num único write — útil pra ressuscitar série
/// inteira após bug de sync (cliente CalDAV "perdeu" todas instâncias
/// modificadas) sem N round-trips. Cada instance é validada como override
/// existente via `has_recurrence_id_override`; ausentes vão pra `not_found`
/// (não 404 individualmente — best-effort). Se NENHUMA instance bate
/// (todas no `not_found` ou lista filtrada vazia), retorna 404. Limite 1..256
/// instances. Aplica `patch_recurrence_id_override_block(..., None×5)`
/// sequencialmente in-memory, depois 1 único `EventRepo::update`. Mesma
/// semantics do #505: sequence NÃO bumpa, ETag/`updated_at` refrescam.
/// Requer WRITE+. Retorna `{event_id, touched:[…compacts…], not_found:[…],
/// dtstamp, etag, sequence}`.
///
/// `?dry=true` (sprint #511, paralelo de #510): só retorna o plano (lista
/// de compacts que SERIAM tocados + `not_found` particionado igual) sem
/// `EventRepo::update`, sem alterar ETag/`updated_at`/DTSTAMP, sem
/// publicar `EventUpdated`. Útil pra UI confirmar "instances X/Y/Z viram
/// tocadas, A/B não existem" antes de rodar. Mesma validação 400 (lista
/// 1..256, master sem UID) e 404 (touched vazio) — preserva semantics do
/// path real. Retorna `{dry:true, event_id, touched:[…], not_found:[…]}`
/// (sem etag/sequence/dtstamp).
async fn touch_overrides_bulk(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<TouchOverridesBulkQuery>,
    Json(body):   Json<TouchOverridesBulkBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    if body.instances.is_empty() || body.instances.len() > 256 {
        return Err(CalendarError::BadRequest(
            "instances must have 1..256 entries".into()
        ));
    }

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate overrides".into()
    ))?;

    let dry = q.dry.unwrap_or(false);

    if dry {
        let mut touched:   Vec<String> = Vec::new();
        let mut not_found: Vec<String> = Vec::new();
        for inst in &body.instances {
            let target = match parse_one_exdate(inst) {
                Some(t) => t,
                None    => { not_found.push(inst.clone()); continue; }
            };
            let target_compact = format_compact_utc(target);
            if touched.iter().any(|c| c == &target_compact) { continue; }
            if !has_recurrence_id_override(&ev.ical_raw, &uid, &target_compact) {
                not_found.push(target_compact);
                continue;
            }
            touched.push(target_compact);
        }
        if touched.is_empty() {
            return Err(CalendarError::EventNotFound(id));
        }
        return Ok(Json(serde_json::json!({
            "dry":       true,
            "event_id":  ev.id,
            "touched":   touched,
            "not_found": not_found,
        })));
    }

    let dtstamp_now = format_compact_utc(OffsetDateTime::now_utc());
    let mut raw         = ev.ical_raw.clone();
    let mut touched:    Vec<String> = Vec::new();
    let mut not_found:  Vec<String> = Vec::new();

    for inst in &body.instances {
        let target = match parse_one_exdate(inst) {
            Some(t) => t,
            None    => { not_found.push(inst.clone()); continue; }
        };
        let target_compact = format_compact_utc(target);
        if touched.iter().any(|c| c == &target_compact) { continue; }
        if !has_recurrence_id_override(&raw, &uid, &target_compact) {
            not_found.push(target_compact);
            continue;
        }
        raw = patch_recurrence_id_override_block(
            &raw, &uid, &target_compact,
            None, None, None, None, None,
            &dtstamp_now,
        );
        touched.push(target_compact);
    }

    if touched.is_empty() {
        return Err(CalendarError::EventNotFound(id));
    }

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":  ev.id,
        "touched":   touched,
        "not_found": not_found,
        "dtstamp":   dtstamp_now,
        "etag":      updated.etag,
        "sequence":  updated.sequence,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct TouchAllQuery {
    /// `?dry=true` retorna o plano sem aplicar (sprint #510). Default false.
    dry: Option<bool>,
}

/// POST /api/v1/calendars/:cal_id/events/:id/touch-all — refresca o
/// DTSTAMP do MASTER + de TODOS os overrides (RECURRENCE-ID) num único
/// write (sprint #508, combinação do #506 + #507). Descobre overrides
/// via `list_recurrence_id_overrides` (mesmo walker do GET de #503),
/// extrai cada `compact`, aplica `patch_recurrence_id_override_block(
/// raw, uid, compact, None×5, &dtstamp_now)` sequencialmente in-memory,
/// + `patch_master_dtstamp(raw, uid, &dtstamp_now)` no fim. 1 único
/// `EventRepo::update`. Use case: "ressuscitar" série inteira pós
/// restore de backup ou reset total de cache CalDAV sem ter que
/// listar instances client-side. Mesma semantics do #505/#506/#507:
/// sequence NÃO bumpa (DTSTAMP fora das colunas DISTINCT FROM); ETag/
/// updated_at refrescam — invalida HTTP/CalDAV cache pro VCALENDAR
/// inteiro. Sem body. 400 se master sem UID. Requer WRITE+. Retorna
/// `{event_id, master_touched:true, overrides_touched:[…compact…],
/// dtstamp, etag, sequence}`.
///
/// `?dry=true` (sprint #510): só retorna o plano (lista de compacts
/// que SERIAM tocados + master:true) sem chamar `EventRepo::update`,
/// sem alterar ETag/`updated_at`/DTSTAMP, sem publicar `EventUpdated`.
/// Útil pra UI confirmar "vai mexer em N overrides + master, ok?"
/// antes de rodar de verdade. 400 ainda fired se master sem UID.
/// Retorna `{dry:true, event_id, master_touched:true,
/// overrides_touched:[…compact…]}` (sem etag/sequence/dtstamp).
async fn touch_all(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<TouchAllQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate overrides".into()
    ))?;

    let dry = q.dry.unwrap_or(false);

    if dry {
        let mut overrides_touched: Vec<String> = Vec::new();
        let listed = list_recurrence_id_overrides(&ev.ical_raw, &uid, false);
        for item in &listed {
            let compact = match item.get("compact").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None    => continue,
            };
            if overrides_touched.iter().any(|c| c == &compact) { continue; }
            overrides_touched.push(compact);
        }
        return Ok(Json(serde_json::json!({
            "dry":               true,
            "event_id":          ev.id,
            "master_touched":    true,
            "overrides_touched": overrides_touched,
        })));
    }

    let dtstamp_now = format_compact_utc(OffsetDateTime::now_utc());
    let mut raw = ev.ical_raw.clone();
    let mut overrides_touched: Vec<String> = Vec::new();

    let listed = list_recurrence_id_overrides(&raw, &uid, false);
    for item in &listed {
        let compact = match item.get("compact").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None    => continue,
        };
        if overrides_touched.iter().any(|c| c == &compact) { continue; }
        raw = patch_recurrence_id_override_block(
            &raw, &uid, &compact,
            None, None, None, None, None,
            &dtstamp_now,
        );
        overrides_touched.push(compact);
    }

    raw = patch_master_dtstamp(&raw, &uid, &dtstamp_now);

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id":          ev.id,
        "master_touched":    true,
        "overrides_touched": overrides_touched,
        "dtstamp":           dtstamp_now,
        "etag":              updated.etag,
        "sequence":          updated.sequence,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct TouchOverridesByRangeQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?dry=true` retorna o plano sem aplicar (sprint #512). Default false.
    #[serde(default)]
    dry:    Option<bool>,
}

/// POST /api/v1/calendars/:cal_id/events/:id/touch-overrides-by-range
/// ?after=&before= — variante range do #507 sem listar instances
/// (sprint #509). Descobre overrides via `list_recurrence_id_overrides`
/// (mesmo walker do #503/#508), parseia cada `compact` via
/// `parse_one_exdate`, filtra os que caem em `[after, before)`
/// (intervalo half-open: `after` inclusive, `before` exclusive — semantics
/// padrão de range queries; ambos opcionais — sem `after` = sem lower
/// bound, sem `before` = sem upper bound, sem nenhum = todos overrides
/// ≡ #508 sem master). Toca cada match via
/// `patch_recurrence_id_override_block(raw, uid, compact, None×5,
/// &dtstamp_now)` in-memory; 1 único `EventRepo::update`. Use case:
/// "ressuscitar" só os overrides futuros (ex: `?after=2026-05-01T00Z`)
/// sem afetar histórico, ou só janela de migração específica. Mesma
/// semantics do #505/#506/#507/#508: sequence NÃO bumpa, ETag/`updated_at`
/// refrescam. 400 se `after >= before`. Requer WRITE+. Retorna
/// `{event_id, touched:[…compacts…], skipped:[…compacts fora do range…],
/// dtstamp, etag, sequence}` ou 404 se nenhum bate (compatible com
/// EventNotFound mas semantica é "no overrides in range").
///
/// `?dry=true` (sprint #512, fecha trio bulk dry-run após #510 e #511):
/// só retorna o plano (`{dry:true, event_id, touched, skipped}`) sem
/// `EventRepo::update`, sem alterar ETag/`updated_at`/DTSTAMP, sem
/// publicar `EventUpdated`. Mesmas validações 400 (`after >= before`,
/// master sem UID) e 404 (touched vazio) que path real — UI não vê
/// dry "ok" mas real "fail". Útil pra UI confirmar "vai tocar N
/// overrides em [after,before), N' fora, ok?" antes de rodar.
async fn touch_overrides_by_range(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<TouchOverridesByRangeQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate overrides".into()
    ))?;

    let dry = q.dry.unwrap_or(false);

    if dry {
        let mut touched: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let listed = list_recurrence_id_overrides(&ev.ical_raw, &uid, false);
        for item in &listed {
            let compact = match item.get("compact").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None    => continue,
            };
            if touched.iter().any(|c| c == &compact) || skipped.iter().any(|c| c == &compact) {
                continue;
            }
            let parsed = match parse_one_exdate(&compact) {
                Some(t) => t,
                None    => { skipped.push(compact); continue; }
            };
            if let Some(a) = q.after  { if parsed <  a { skipped.push(compact); continue; } }
            if let Some(b) = q.before { if parsed >= b { skipped.push(compact); continue; } }
            touched.push(compact);
        }
        if touched.is_empty() {
            return Err(CalendarError::EventNotFound(id));
        }
        return Ok(Json(serde_json::json!({
            "dry":      true,
            "event_id": ev.id,
            "touched":  touched,
            "skipped":  skipped,
        })));
    }

    let dtstamp_now = format_compact_utc(OffsetDateTime::now_utc());
    let mut raw = ev.ical_raw.clone();
    let mut touched: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    let listed = list_recurrence_id_overrides(&raw, &uid, false);
    for item in &listed {
        let compact = match item.get("compact").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None    => continue,
        };
        if touched.iter().any(|c| c == &compact) || skipped.iter().any(|c| c == &compact) {
            continue;
        }
        let parsed = match parse_one_exdate(&compact) {
            Some(t) => t,
            None    => { skipped.push(compact); continue; }
        };
        if let Some(a) = q.after  { if parsed <  a { skipped.push(compact); continue; } }
        if let Some(b) = q.before { if parsed >= b { skipped.push(compact); continue; } }

        raw = patch_recurrence_id_override_block(
            &raw, &uid, &compact,
            None, None, None, None, None,
            &dtstamp_now,
        );
        touched.push(compact);
    }

    if touched.is_empty() {
        return Err(CalendarError::EventNotFound(id));
    }

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id": ev.id,
        "touched":  touched,
        "skipped":  skipped,
        "dtstamp":  dtstamp_now,
        "etag":     updated.etag,
        "sequence": updated.sequence,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct PatchOverridesByRangeQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    dry:    Option<bool>,
}

/// PATCH /api/v1/calendars/:cal_id/events/:id/overrides-by-range
/// ?after=&before=&dry= — bulk-patch range-filtered (sprint #537,
/// paralelo do touch-by-range #509 mas com payload de mutação real
/// em vez de só DTSTAMP). Body é `PatchOverrideBody` reusado do #498
/// (summary/description/location/dtstart/dtend; pelo menos 1
/// obrigatório). Aplica `patch_recurrence_id_override_block(raw, uid,
/// compact, Some(...), &dtstamp_now)` com os mesmos campos pra TODOS
/// os overrides cujo RECURRENCE-ID cai em `[after, before)`. Use case:
/// "todos os overrides futuros precisam de novo título" / "set location
/// pra todos os overrides desta janela de migração". Mesmo half-open
/// range do #509/#517/#520; `after >= before` → 400. Sem `after`/sem
/// `before` = sem bound (≡ patch em massa de TODOS overrides), mas
/// payload obrigatório (sem campos = 400, mesma validação do #498
/// single-patch). 1 único `EventRepo::update` no fim — ETag/`updated_at`
/// refrescam, `sequence` bumpa SE algum campo do master comparado mudou
/// (mas patches só mexem em overrides, então sequence não bumpa —
/// mesma semantics do touch-by-range #509). Requer WRITE+. 404 se
/// `touched` vazio (nenhum override match'a o range).
///
/// `?dry=true` (paralelo do #512): só retorna `{dry:true, event_id,
/// touched, skipped}` sem `EventRepo::update`, sem alterar
/// ETag/`updated_at`/DTSTAMP, sem publicar `EventUpdated`. Mesmas
/// validações 400 (`after >= before`, body vazio, master sem UID) e
/// 404 (touched vazio) que path real — UI pode confirmar "patch vai
/// afetar N overrides em [after,before), N' fora, ok?" antes de rodar.
async fn patch_overrides_by_range(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<PatchOverridesByRangeQuery>,
    Json(body):   Json<PatchOverrideBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    if body.summary.is_none() && body.description.is_none()
        && body.location.is_none() && body.dtstart.is_none() && body.dtend.is_none()
    {
        return Err(CalendarError::BadRequest(
            "at least one of summary/description/location/dtstart/dtend required".into()
        ));
    }
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate overrides".into()
    ))?;

    let dtstart_str = body.dtstart.map(|d| format_compact_utc(d.to_offset(time::UtcOffset::UTC)));
    let dtend_str   = body.dtend.map(|d| format_compact_utc(d.to_offset(time::UtcOffset::UTC)));
    let dry = q.dry.unwrap_or(false);

    if dry {
        let mut touched: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let listed = list_recurrence_id_overrides(&ev.ical_raw, &uid, false);
        for item in &listed {
            let compact = match item.get("compact").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None    => continue,
            };
            if touched.iter().any(|c| c == &compact) || skipped.iter().any(|c| c == &compact) {
                continue;
            }
            let parsed = match parse_one_exdate(&compact) {
                Some(t) => t,
                None    => { skipped.push(compact); continue; }
            };
            if let Some(a) = q.after  { if parsed <  a { skipped.push(compact); continue; } }
            if let Some(b) = q.before { if parsed >= b { skipped.push(compact); continue; } }
            touched.push(compact);
        }
        if touched.is_empty() {
            return Err(CalendarError::EventNotFound(id));
        }
        return Ok(Json(serde_json::json!({
            "dry":      true,
            "event_id": ev.id,
            "touched":  touched,
            "skipped":  skipped,
        })));
    }

    let dtstamp_now = format_compact_utc(OffsetDateTime::now_utc());
    let mut raw = ev.ical_raw.clone();
    let mut touched: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    let listed = list_recurrence_id_overrides(&raw, &uid, false);
    for item in &listed {
        let compact = match item.get("compact").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None    => continue,
        };
        if touched.iter().any(|c| c == &compact) || skipped.iter().any(|c| c == &compact) {
            continue;
        }
        let parsed = match parse_one_exdate(&compact) {
            Some(t) => t,
            None    => { skipped.push(compact); continue; }
        };
        if let Some(a) = q.after  { if parsed <  a { skipped.push(compact); continue; } }
        if let Some(b) = q.before { if parsed >= b { skipped.push(compact); continue; } }

        raw = patch_recurrence_id_override_block(
            &raw, &uid, &compact,
            body.summary.as_deref(),
            body.description.as_deref(),
            body.location.as_deref(),
            dtstart_str.as_deref(),
            dtend_str.as_deref(),
            &dtstamp_now,
        );
        touched.push(compact);
    }

    if touched.is_empty() {
        return Err(CalendarError::EventNotFound(id));
    }

    let updated = EventRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;
    state.events().publish(crate::events::Event::EventUpdated {
        tenant_id: ctx.tenant_id, event_id: updated.id,
        summary: updated.summary.clone(), sequence: updated.sequence,
    });

    Ok(Json(serde_json::json!({
        "event_id": ev.id,
        "touched":  touched,
        "skipped":  skipped,
        "dtstamp":  dtstamp_now,
        "etag":     updated.etag,
        "sequence": updated.sequence,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct PatchOverridesByRangePreviewQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

/// GET /api/v1/calendars/:cal_id/events/:id/overrides-by-range/preview
/// ?after=&before= — read-only dry-preview do plano de patch-by-range
/// (sprint #543, complementa o `?dry=true` do #537 que ainda exige body
/// válido de `PatchOverrideBody`). Mesmo walker e mesma classificação
/// touched/skipped do path PATCH dry: itera `list_recurrence_id_overrides`
/// → `parse_one_exdate` → bucket por `[after, before)` half-open. Diferença
/// chave vs `?dry=true` do PATCH: este endpoint NÃO requer body algum (sem
/// validação `at_least_one_field` do #537, sem mesmo Content-Type), porque
/// o universo de overrides afetados é independente dos campos a patchar
/// (afetar = match no range; o body só decide *o que* mudar, não *quem*).
/// UI usa pra "discovery" puro: "se eu rodar patch-by-range nesta janela
/// agora, quem é afetado?" sem precisar montar body fictício antes de
/// confirmar com o usuário. Read-only, NÃO requer WRITE+ (paralelo aos
/// touch-preview #514, exdates-preview #525, exdates-preview/stats #530).
/// Retorna sempre `{event_id, touched, skipped}` (mesmo shape do dry do
/// #537 mas sem `dry:true` flag — endpoint é GET, "dry" é implícito);
/// vazio em `touched` retorna 200 com lista vazia (paralelo do
/// touch-preview que não 404, contraste com PATCH `?dry=true` que 404 em
/// touched vazio porque é alternative path do PATCH real). 400 em
/// `after >= before` ou master sem UID.
async fn patch_overrides_by_range_preview(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<PatchOverridesByRangePreviewQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate overrides".into()
    ))?;

    let mut touched: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let listed = list_recurrence_id_overrides(&ev.ical_raw, &uid, false);
    for item in &listed {
        let compact = match item.get("compact").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None    => continue,
        };
        if touched.iter().any(|c| c == &compact) || skipped.iter().any(|c| c == &compact) {
            continue;
        }
        let parsed = match parse_one_exdate(&compact) {
            Some(t) => t,
            None    => { skipped.push(compact); continue; }
        };
        if let Some(a) = q.after  { if parsed <  a { skipped.push(compact); continue; } }
        if let Some(b) = q.before { if parsed >= b { skipped.push(compact); continue; } }
        touched.push(compact);
    }

    Ok(Json(serde_json::json!({
        "event_id": ev.id,
        "touched":  touched,
        "skipped":  skipped,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct TouchPreviewQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?include_unparseable=false` esconde a lista `unparseable` do payload
    /// (ainda contabiliza no `total_overrides`). Default true preserva
    /// shape do #514 (sprint #515).
    include_unparseable: Option<bool>,
    /// `?only_parseable=true` filtra TUDO: nem `unparseable` aparece nem
    /// conta em `total_overrides` — payload reflete só o universo de
    /// RECURRENCE-IDs que touch-all efetivamente conseguiria mexer.
    /// Conflita com `include_unparseable=true` explícito → 400.
    /// Default false preserva shape do #514 (sprint #515).
    only_parseable: Option<bool>,
    /// `?detail=full` engrandece `in_range`/`out_of_range` de `Vec<String>`
    /// (só compacts, default do #514) pra `Vec<{compact,summary,dtstart,
    /// dtend,description,location}>` — pra UI confirmar visualmente quem vai
    /// ser afetado antes de touch-all (sprint #527, paralelo direto do
    /// `?detail=full` do #503 mas em preview agregado). `unparseable` continua
    /// `Vec<String>` mesmo em full porque por definição não tem campos
    /// parseáveis (RECURRENCE-ID corrompido). Valores válidos: `summary`
    /// (default, preserva shape #514) e `full`. Outro → 400.
    detail: Option<String>,
}

/// GET /api/v1/calendars/:cal_id/events/:id/touch-preview?after=&before=
/// `[&include_unparseable=false][&only_parseable=true][&detail=full]` —
/// consolida em 1 chamada o que SERIA tocado por `touch-all` (#508) +
/// `touch-overrides-by-range` (#509) sem nenhum side effect (sprint #514;
/// flags de filtro adicionadas no #515; `?detail=full` adicionado no #527
/// pra UI confirmar visualmente quem vai ser afetado — `in_range`/
/// `out_of_range` viram `[{compact,summary,dtstart,dtend,description,
/// location}]` em vez de `[String]`; `unparseable` continua `[String]` por
/// definição — RECURRENCE-ID corrompido não tem campos parseáveis;
/// `?detail=summary` ou ausente preserva 100% shape do #514).
/// Diferente dos POST `?dry=true` (#510/#511/#512/#513) que precisam de
/// WRITE+ porque são apenas POST com short-circuit, este é GET puro,
/// READ-only — útil pra UI que só quer "discovery": "se eu rodar touch-all
/// agora, quem é afetado?". Retorna sempre `{event_id, master, total_overrides,
/// in_range, out_of_range, unparseable}` cobrindo TODAS dimensões num só
/// payload: `master` sempre true (o master sempre seria tocado num touch-all),
/// `in_range` lista compacts que cairiam dentro de [after, before) (filtros
/// half-open opcionais — mesma semantics do #509), `out_of_range` lista os
/// fora, `unparseable` lista RECURRENCE-IDs que `parse_one_exdate` rejeita
/// (compact corrompido). Sem `after`/`before`, todos vão pra `in_range`
/// (caso degenerado ≡ touch-all sem master). 400 se `after >= before` ou
/// master sem UID. Read-only, NÃO requer WRITE+ (paralelo aos GETs do #496
/// e #503/#504). Ortogonal aos POST `?dry=true` que retornam shape específico
/// por endpoint — preview agrega.
async fn touch_preview(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<TouchPreviewQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;

    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }

    let only_parseable      = q.only_parseable.unwrap_or(false);
    let include_unparseable = q.include_unparseable.unwrap_or(true);
    if only_parseable && q.include_unparseable == Some(true) {
        return Err(CalendarError::BadRequest(
            "only_parseable=true conflicts with include_unparseable=true".into()
        ));
    }

    let full = match q.detail.as_deref() {
        None | Some("") | Some("summary") => false,
        Some("full")                      => true,
        Some(other) => return Err(CalendarError::BadRequest(
            format!("invalid detail `{other}` — expected `summary` or `full`")
        )),
    };

    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let uid = extract_uid(&ev.ical_raw).ok_or_else(|| CalendarError::BadRequest(
        "master event has no UID — cannot locate overrides".into()
    ))?;

    let mut in_range:     Vec<serde_json::Value> = Vec::new();
    let mut out_of_range: Vec<serde_json::Value> = Vec::new();
    let mut unparseable:  Vec<String>            = Vec::new();
    let mut seen:         Vec<String>            = Vec::new();

    let listed = list_recurrence_id_overrides(&ev.ical_raw, &uid, full);
    for item in &listed {
        let compact = match item.get("compact").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None    => continue,
        };
        if seen.iter().any(|c| c == &compact) { continue; }
        seen.push(compact.clone());

        let parsed = match parse_one_exdate(&compact) {
            Some(t) => t,
            None    => { unparseable.push(compact); continue; }
        };
        let bucket_item = if full { item.clone() } else { serde_json::Value::String(compact.clone()) };
        if let Some(a) = q.after  { if parsed <  a { out_of_range.push(bucket_item); continue; } }
        if let Some(b) = q.before { if parsed >= b { out_of_range.push(bucket_item); continue; } }
        in_range.push(bucket_item);
    }

    // `only_parseable` exclui unparseable do count + payload (universo "tocável" puro).
    // `include_unparseable=false` esconde só do payload mas mantém no count
    // (UI sabe que existem N items corrompidos sem precisar listá-los).
    let total_overrides = if only_parseable {
        in_range.len() + out_of_range.len()
    } else {
        in_range.len() + out_of_range.len() + unparseable.len()
    };

    let mut payload = serde_json::json!({
        "event_id":        ev.id,
        "master":          true,
        "total_overrides": total_overrides,
        "in_range":        in_range,
        "out_of_range":    out_of_range,
    });
    if include_unparseable && !only_parseable {
        payload["unparseable"] = serde_json::json!(unparseable);
    }
    Ok(Json(payload))
}

#[derive(Debug, serde::Deserialize)]
struct ExdatesPreviewQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?include_non_utc=false` esconde a lista `non_utc` do payload (ainda
    /// contabiliza no `total_exdates`). Default true preserva shape baseline.
    /// Paralelo direto do `include_unparseable` do touch-preview #515 — a
    /// classe "EXDATE não-UTC" (tzid/date-only/unknown) é o equivalente
    /// semântico de "RECURRENCE-ID corrompido" do touch-preview, porque
    /// ambos representam items que NÃO se encaixam no range filter half-open.
    include_non_utc: Option<bool>,
    /// `?only_utc=true` filtra TUDO: nem `non_utc` aparece nem conta em
    /// `total_exdates` — payload reflete só o universo de EXDATEs com
    /// `parsed_utc=Some` (paralelo do `only_parseable` do #515). Conflita
    /// com `include_non_utc=true` explícito → 400. Default false preserva
    /// shape baseline.
    only_utc: Option<bool>,
    /// `?detail=full` (sprint #528, paralelo simétrico do #527 mas pra EXDATE)
    /// engrandece `in_range`/`out_of_range`/`non_utc` de `Vec<String>` (só
    /// `raw_value`s, default do #525) pra `Vec<{compact, rfc3339, kind,
    /// raw_value, tzid, params}>` reusando `parse_exdates_rich` que já
    /// produz exatamente essa shape — zero extensão de helper. Mesma shape
    /// do `/exdates?detail=full` do #511/#516 (consistência cross-endpoint:
    /// preview e list mostram items idênticos quando enriquecidos). Em
    /// `non_utc`, `compact`/`rfc3339` ficam `null` (não há timestamp UTC).
    /// Diferença vs touch-preview #527 cujo `unparseable` permanece String:
    /// aqui `non_utc` enrichece porque `parse_exdates_rich` extrai metadata
    /// rica mesmo sem `parsed_utc` (kind/tzid/params). Default
    /// `summary` (mesmo que ausente) preserva 100% shape do #525.
    detail: Option<String>,
    /// `?top_tzid=N` (sprint #532, follow-on do #530) trunca o
    /// `tzid_breakdown` em `/exdates-preview/stats` pra apenas as N TZIDs
    /// mais frequentes (sort by count desc, ties broken por insertion
    /// order do `Vec` association list — i.e. TZID que apareceu primeiro
    /// no walker). Adiciona `tzid_other_count: usize` agregando a soma das
    /// counts das TZIDs descartadas (não inclui no breakdown). Útil pra UI
    /// dashboard com ranking-truncado quando há cardinalidade alta de
    /// TZIDs (>20 distintas) — sem flag, payload pode ficar pesado e a
    /// curva é tipicamente long-tail (poucas TZIDs com muitas ocorrências
    /// + cauda de TZIDs únicos). `top_tzid=0` → 400 (não faz sentido —
    /// `include_non_utc=false` ou `only_utc=true` já omitem o breakdown
    /// inteiro). Sem flag (None) preserva 100% shape do #530 (breakdown
    /// completo, sem `tzid_other_count`). Portado pra `/exdates/stats` do
    /// #531 via sprint #533 fechando dualidade `top_tzid` cross-stats —
    /// pattern "stats endpoints evoluem EM PAR" aplica também a flags de
    /// presentation. Aplicado APÓS o full breakdown ser construído (re-sort
    /// + split é O(N log N) numa lista pequena, irrelevante).
    top_tzid: Option<usize>,
    /// `?sort_tzid=count_desc|count_asc|name_asc|name_desc` (sprint #534,
    /// presentation flag complementar ao `top_tzid` #532, paralelo simétrico
    /// do mesmo flag em `ExdatesStatsQuery` do #533). Emite o array
    /// adjacente `tzid_breakdown_order: [tzid1, tzid2, ...]` listando as
    /// chaves do `tzid_breakdown` na ordem solicitada — necessário porque
    /// `serde_json::Map` sem feature `preserve_order` serializa Object em
    /// ordem alfabética determinística, então a única forma de transmitir
    /// ordem custom em JSON Object é via array adjacente que a UI itera
    /// buscando counts por chave. Aplicado em DUAS fases compostas com
    /// `top_tzid`: (1) selecionar top-N por count desc; (2) ordenar o set
    /// retido. Variantes: `count_desc`/`count_asc` por count com ties
    /// broken por insertion order do walker; `name_asc`/`name_desc`
    /// alfabeticamente pelo TZID. Outros valores → 400. Sem flag (None)
    /// omite `tzid_breakdown_order` e preserva 100% shape do #532. Só faz
    /// sentido quando `tzid_breakdown` é incluído no payload (i.e.
    /// `include_non_utc=true` e `only_utc=false`) — caso contrário o array
    /// também não é emitido.
    #[serde(default)]
    sort_tzid: Option<String>,
    /// `?min_count=N` (sprint #535, dual filosófico ao `top_tzid` #532,
    /// paralelo simétrico do mesmo flag em `ExdatesStatsQuery` do #533+#535
    /// — pattern "stats endpoints evoluem EM PAR" agora cobre 4 dimensões:
    /// agregado, range, top_tzid (cardinalidade da cabeça), sort_tzid
    /// (apresentação), min_count (cardinalidade da cauda)). Filtra
    /// `tzid_breakdown` removendo TZIDs com count < N. Composição em 3
    /// fases (mesma ordem do #535 em ExdatesStatsQuery): (1) min_count
    /// filtra long-tail; (2) top_tzid trunca cabeça do universo filtrado;
    /// (3) sort_tzid ordena. Emite `tzid_filtered_count` somente quando
    /// `min_count.is_some()`. `min_count=0` → 400. Sem flag (None) preserva
    /// 100% shape do #534. Só faz sentido quando `tzid_breakdown` é
    /// incluído (mesma condição do `top_tzid`/`sort_tzid`).
    #[serde(default)]
    min_count: Option<usize>,
    /// `?include_kind_breakdown=true` (sprint #536, paralelo simétrico do
    /// mesmo flag em `ExdatesStatsQuery` — fechando dualidade
    /// cross-stats já estabelecida em #530+#531, #532+#533, #534, #535).
    /// Emite `tzid_breakdown_by_kind: {tz: {tzid: N, unknown: M}}`
    /// particionando o count de cada TZID retido em canônico vs
    /// malformado (mesma definição do #536 em `ExdatesStatsQuery`).
    /// Aplicado APÓS `min_count` → `top_tzid` → `sort_tzid` — só TZIDs
    /// retidos no `kept` final aparecem. Sem flag (None ou false) omite
    /// o objeto e preserva 100% shape do #535. Só faz sentido quando
    /// `tzid_breakdown` é incluído (mesma condição do `top_tzid`/
    /// `sort_tzid`/`min_count` — i.e. `include_non_utc=true` e
    /// `only_utc=false`).
    #[serde(default)]
    include_kind_breakdown: Option<bool>,
}

/// GET /api/v1/calendars/:cal_id/events/:id/exdates-preview?after=&before=
/// `[&include_non_utc=false][&only_utc=true][&detail=full]` — paralelo simétrico
/// do touch-preview (#514/#515/#527) mas pra EXDATE list (sprint #525/#528). Read-only,
/// NÃO requer WRITE+. Útil pra UI fazer "discovery" de quais EXDATEs cairiam
/// numa janela temporal antes de qualquer ação (e.g. preview antes de bulk
/// delete por range, audit "quais cancelamentos estão programados pra esta
/// semana"). Reusa `parse_exdates_rich` (mesma fonte do #511/#516/#522/#523/
/// #524) sem extensão — itera uma vez particionando em 3 listas:
/// - `in_range`: EXDATEs UTC parseáveis (`parsed_utc=Some`) cujo timestamp
///   cai dentro de [after, before) (filtros half-open opcionais — mesma
///   semantics do #517/#522);
/// - `out_of_range`: EXDATEs UTC parseáveis fora do intervalo;
/// - `non_utc`: EXDATEs com `parsed_utc=None` (kind=tzid|date-only|unknown
///   — não têm timestamp UTC pra comparar com bounds, classe semantic
///   equivalente ao `unparseable` do touch-preview).
///
/// Retorna `{event_id, total_exdates, in_range, out_of_range[, non_utc]}`.
/// Sem `after`/`before`, todos os UTC vão pra `in_range` (caso degenerado).
/// 400 se `after >= before` ou `only_utc=true && include_non_utc=true`.
///
/// Diferenças do `/exdates?detail=full&kind=...&with_*=...` do #516/#523:
/// list endpoint retorna items com metadata RICA (raw_value, tzid, params)
/// e suporta filtro categórico/qualitativo; preview retorna só `compact`s
/// agrupados em buckets temporais (paralelo direto do touch-preview #514).
/// Os 2 endpoints são complementares: preview pra dashboard "qual a janela",
/// list pra drill-down "me dê os detalhes destas EXDATEs". Read-only, sem
/// WRITE+, 404 se evento não existe.
async fn exdates_preview(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<ExdatesPreviewQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let only_utc        = q.only_utc.unwrap_or(false);
    let include_non_utc = q.include_non_utc.unwrap_or(true);
    if only_utc && q.include_non_utc == Some(true) {
        return Err(CalendarError::BadRequest(
            "only_utc=true conflicts with include_non_utc=true".into()
        ));
    }
    let full = match q.detail.as_deref() {
        None | Some("") | Some("summary") => false,
        Some("full")                      => true,
        Some(other) => return Err(CalendarError::BadRequest(
            format!("invalid detail `{other}` — expected `summary` or `full`")
        )),
    };
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let mut in_range:     Vec<serde_json::Value> = Vec::new();
    let mut out_of_range: Vec<serde_json::Value> = Vec::new();
    let mut non_utc:      Vec<serde_json::Value> = Vec::new();
    let mut seen:         Vec<String>            = Vec::new();

    for info in parse_exdates_rich(&ev.ical_raw) {
        let key = info.raw_value.clone();
        if seen.iter().any(|c| c == &key) { continue; }
        seen.push(key.clone());

        let (compact_v, rfc_v, parsed_opt) = match info.parsed_utc {
            Some(t) => {
                let utc = t.to_offset(time::UtcOffset::UTC);
                let c = format!(
                    "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
                    utc.year(), u8::from(utc.month()), utc.day(),
                    utc.hour(), utc.minute(), utc.second(),
                );
                let r = utc.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                (serde_json::Value::String(c), serde_json::Value::String(r), Some(t))
            }
            None => (serde_json::Value::Null, serde_json::Value::Null, None),
        };

        let bucket_item: serde_json::Value = if full {
            serde_json::json!({
                "compact":   compact_v,
                "rfc3339":   rfc_v,
                "kind":      info.kind,
                "raw_value": info.raw_value,
                "tzid":      info.tzid,
                "params":    info.params,
            })
        } else {
            serde_json::Value::String(key.clone())
        };

        let parsed = match parsed_opt {
            Some(t) => t,
            None    => { non_utc.push(bucket_item); continue; }
        };
        if let Some(a) = q.after  { if parsed <  a { out_of_range.push(bucket_item); continue; } }
        if let Some(b) = q.before { if parsed >= b { out_of_range.push(bucket_item); continue; } }
        in_range.push(bucket_item);
    }

    let total_exdates = if only_utc {
        in_range.len() + out_of_range.len()
    } else {
        in_range.len() + out_of_range.len() + non_utc.len()
    };

    let mut payload = serde_json::json!({
        "event_id":      ev.id,
        "total_exdates": total_exdates,
        "in_range":      in_range,
        "out_of_range":  out_of_range,
    });
    if include_non_utc && !only_utc {
        payload["non_utc"] = serde_json::json!(non_utc);
    }
    Ok(Json(payload))
}

/// GET /api/v1/calendars/:cal_id/events/:id/exdates-preview/stats?after=&before=
/// `[&include_non_utc=false][&only_utc=true]` — variant aggregate-only do
/// `/exdates-preview` do #525, paralelo da dualidade list/stats que cobriu
/// overrides em #519/#520/#521 e EXDATE list em #522/#523/#524 (sprint #526).
/// Mesma partição em 3 buckets (`in_range`/`out_of_range`/`non_utc`), mas
/// retorna só CONTAGENS — útil pra dashboard "quantos cancelamentos caem
/// nesta janela" sem puxar lista de `raw_value`s (pode ser N grande).
///
/// Reusa `parse_exdates_rich` + mesma lógica de partição do #525 (incluindo
/// dedup por `raw_value` — duas EXDATE lines idênticas contam como 1, mesma
/// semantics do preview), mas em vez de empurrar pra `Vec<String>` só
/// incrementa contadores. Mantém os mesmos flags `?include_non_utc=` e
/// `?only_utc=` com a MESMA semantics do #525:
/// - `include_non_utc=false`: omite `non_utc_count` do payload mas mantém em
///   `total_exdates`;
/// - `only_utc=true`: exclui non_utc de TUDO (payload e `total_exdates`);
/// - conflito `only_utc=true && include_non_utc=true` explícito → 400.
///
/// Retorna `{event_id, total_exdates, in_range_count, out_of_range_count
/// [, non_utc_count, non_utc_by_kind]}`. Read-only, NÃO requer WRITE+,
/// 404 se evento não existe. Validações idênticas ao #525
/// (`after >= before` → 400).
///
/// `non_utc_by_kind: {tzid, date_only, unknown}` (sprint #529, paralelo
/// do `by_kind` do #522 mas particionando o bucket "non-UTC" do preview)
/// agrega contagens dos 3 únicos `kind`s que produzem `parsed_utc=None`
/// — `tzid` (TZID-based), `date-only` (date sem time), `unknown`
/// (malformado). Soma `tzid + date_only + unknown == non_utc_count`
/// (invariant testável). Útil pra dashboard "quais EXDATEs preciso
/// migrar pra UTC vs quais estão corrompidas" sem puxar lista do #528 e
/// agrupar client-side. Omitido junto com `non_utc_count` quando
/// `include_non_utc=false` ou `only_utc=true` (paralelo do #526). Nome
/// do campo `date_only` (snake_case) em vez do `kind` literal
/// `"date-only"` (com hífen) pra ficar JSON-key-friendly em consumidores
/// (JS `obj.date_only` vs `obj["date-only"]`).
///
/// `tzid_breakdown: {"Europe/Berlin": N, "America/Sao_Paulo": M, ...}`
/// (sprint #530, drill-down adicional do bucket `tzid` do
/// `non_utc_by_kind`) agrega ocorrências de cada TZID DISTINTO entre os
/// EXDATEs com `kind="tzid"` (não inclui `date-only` nem `unknown` —
/// estes não têm TZID acionável, embora `unknown` ocasionalmente carregue
/// TZID malformado, mantemos disjunto pra invariant da soma:
/// `sum(tzid_breakdown.values()) == non_utc_by_kind.tzid`). Particiona o
/// count opaco do `tzid` em entradas por timezone — útil pra audit "quais
/// timezones aparecem nas EXDATEs não-UTC do calendário" + migration
/// planning "Europe/Berlin tem 47 EXDATEs, vai precisar de N
/// conversions". Implementado via `Vec<(String, usize)>` association
/// list (não `HashMap` — cardinalidade típica é baixa, N TZIDs distintos
/// por evento, lookup linear `iter().position()` mais barato que hash em
/// pequena escala) acumulando só quando `kind="tzid"` e `info.tzid` é
/// `Some`. Serializado como objeto JSON (chaves dinâmicas TZID, valores
/// counts). Omitido junto com `non_utc_by_kind` quando
/// `include_non_utc=false` ou `only_utc=true`.
async fn exdates_preview_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(q):     Query<ExdatesPreviewQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let only_utc        = q.only_utc.unwrap_or(false);
    let include_non_utc = q.include_non_utc.unwrap_or(true);
    if only_utc && q.include_non_utc == Some(true) {
        return Err(CalendarError::BadRequest(
            "only_utc=true conflicts with include_non_utc=true".into()
        ));
    }
    if q.top_tzid == Some(0) {
        return Err(CalendarError::BadRequest(
            "top_tzid must be >= 1 (use include_non_utc=false to omit breakdown entirely)".into()
        ));
    }
    if q.min_count == Some(0) {
        return Err(CalendarError::BadRequest(
            "min_count must be >= 1 (omit flag for full breakdown)".into()
        ));
    }
    let sort_mode: Option<&str> = match q.sort_tzid.as_deref() {
        None | Some("") => None,
        Some(s @ ("count_desc" | "count_asc" | "name_asc" | "name_desc")) => Some(s),
        Some(other) => return Err(CalendarError::BadRequest(
            format!("sort_tzid must be 'count_desc', 'count_asc', 'name_asc' or 'name_desc', got '{other}'")
        )),
    };
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    if ev.calendar_id != cal_id { return Err(CalendarError::EventNotFound(id)); }

    let mut seen:         Vec<String> = Vec::new();
    let mut in_range_n:     usize = 0;
    let mut out_of_range_n: usize = 0;
    let mut non_utc_n:      usize = 0;
    let mut k_tzid:         usize = 0;
    let mut k_date_only:    usize = 0;
    let mut k_unknown:      usize = 0;
    let include_kind_breakdown = q.include_kind_breakdown.unwrap_or(false);
    let mut tzid_breakdown: Vec<(String, usize)> = Vec::new();
    let mut tzid_by_kind: Vec<(String, usize, usize)> = Vec::new();

    for info in parse_exdates_rich(&ev.ical_raw) {
        let key = info.raw_value.clone();
        if seen.iter().any(|c| c == &key) { continue; }
        seen.push(key);
        let parsed = match info.parsed_utc {
            Some(t) => t,
            None    => {
                non_utc_n += 1;
                match info.kind {
                    "tzid" => {
                        k_tzid += 1;
                        if let Some(tz) = info.tzid.as_deref() {
                            match tzid_breakdown.iter().position(|(k, _)| k == tz) {
                                Some(i) => tzid_breakdown[i].1 += 1,
                                None    => tzid_breakdown.push((tz.to_string(), 1)),
                            }
                            if include_kind_breakdown {
                                let canonical = is_canonical_local_datetime(&info.raw_value);
                                match tzid_by_kind.iter().position(|(k, _, _)| k == tz) {
                                    Some(i) => {
                                        if canonical { tzid_by_kind[i].1 += 1; }
                                        else         { tzid_by_kind[i].2 += 1; }
                                    }
                                    None => {
                                        let (c, u) = if canonical { (1, 0) } else { (0, 1) };
                                        tzid_by_kind.push((tz.to_string(), c, u));
                                    }
                                }
                            }
                        }
                    }
                    "date-only" => k_date_only += 1,
                    _           => k_unknown   += 1,
                }
                continue;
            }
        };
        if let Some(a) = q.after  { if parsed <  a { out_of_range_n += 1; continue; } }
        if let Some(b) = q.before { if parsed >= b { out_of_range_n += 1; continue; } }
        in_range_n += 1;
    }

    let total_exdates = if only_utc {
        in_range_n + out_of_range_n
    } else {
        in_range_n + out_of_range_n + non_utc_n
    };

    let mut payload = serde_json::json!({
        "event_id":           ev.id,
        "total_exdates":      total_exdates,
        "in_range_count":     in_range_n,
        "out_of_range_count": out_of_range_n,
    });
    if include_non_utc && !only_utc {
        payload["non_utc_count"]   = serde_json::json!(non_utc_n);
        payload["non_utc_by_kind"] = serde_json::json!({
            "tzid":      k_tzid,
            "date_only": k_date_only,
            "unknown":   k_unknown,
        });
        let (filtered, filtered_count): (Vec<(String, usize)>, usize) = match q.min_count {
            Some(m) => {
                let mut keep = Vec::with_capacity(tzid_breakdown.len());
                let mut excluded = 0usize;
                for (tz, c) in &tzid_breakdown {
                    if *c >= m { keep.push((tz.clone(), *c)); } else { excluded += 1; }
                }
                (keep, excluded)
            }
            None => (tzid_breakdown.clone(), 0),
        };
        let (mut kept, other_count): (Vec<(String, usize)>, usize) = match q.top_tzid {
            Some(n) if filtered.len() > n => {
                let mut sorted = filtered.clone();
                sorted.sort_by(|a, b| b.1.cmp(&a.1));
                let other: usize = sorted.iter().skip(n).map(|(_, c)| *c).sum();
                sorted.truncate(n);
                (sorted, other)
            }
            _ => (filtered, 0),
        };
        match sort_mode {
            Some("count_desc") => kept.sort_by(|a, b| b.1.cmp(&a.1)),
            Some("count_asc")  => kept.sort_by(|a, b| a.1.cmp(&b.1)),
            Some("name_asc")   => kept.sort_by(|a, b| a.0.cmp(&b.0)),
            Some("name_desc")  => kept.sort_by(|a, b| b.0.cmp(&a.0)),
            _ => {}
        }
        let mut breakdown_obj = serde_json::Map::new();
        for (tz, n) in &kept {
            breakdown_obj.insert(tz.clone(), serde_json::json!(n));
        }
        payload["tzid_breakdown"] = serde_json::Value::Object(breakdown_obj);
        if q.top_tzid.is_some() {
            payload["tzid_other_count"] = serde_json::json!(other_count);
        }
        if q.min_count.is_some() {
            payload["tzid_filtered_count"] = serde_json::json!(filtered_count);
        }
        if sort_mode.is_some() {
            let order: Vec<serde_json::Value> = kept.iter()
                .map(|(tz, _)| serde_json::Value::String(tz.clone()))
                .collect();
            payload["tzid_breakdown_order"] = serde_json::Value::Array(order);
        }
        if include_kind_breakdown {
            let mut by_kind_obj = serde_json::Map::new();
            for (tz, _) in &kept {
                let (c, u) = tzid_by_kind.iter()
                    .find(|(k, _, _)| k == tz)
                    .map(|(_, c, u)| (*c, *u))
                    .unwrap_or((0, 0));
                by_kind_obj.insert(tz.clone(), serde_json::json!({
                    "tzid":    c,
                    "unknown": u,
                }));
            }
            payload["tzid_breakdown_by_kind"] = serde_json::Value::Object(by_kind_obj);
        }
    }
    Ok(Json(payload))
}

/// Reescreve o bloco VEVENT cujo UID==`uid_master` e RECURRENCE-ID==
/// `target_compact`. Pra cada propriedade alvo (SUMMARY/DESCRIPTION/
/// LOCATION/DTSTART/DTEND): se `Some` substitui linha existente ou
/// adiciona antes de END:VEVENT; se `None` preserva. DTSTAMP sempre
/// refrescado pra `dtstamp_now`. Outras linhas (UID, RECURRENCE-ID,
/// RRULE residual etc.) preservadas. Outros blocos VEVENT inalterados.
fn patch_recurrence_id_override_block(
    raw: &str,
    uid_master:     &str,
    target_compact: &str,
    new_summary:     Option<&str>,
    new_description: Option<&str>,
    new_location:    Option<&str>,
    new_dtstart:     Option<&str>,
    new_dtend:       Option<&str>,
    dtstamp_now:    &str,
) -> String {
    let eol = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(raw.len() + 256);
    let mut buf: Vec<String> = Vec::new();
    let mut in_event = false;
    let mut found_uid = false;
    let mut found_recid = false;

    for src_line in raw.split_inclusive('\n') {
        let trimmed = src_line.trim_start();
        let upper14: String = trimmed.chars().take(14).collect::<String>().to_ascii_uppercase();

        if upper14.starts_with("BEGIN:VEVENT") {
            in_event = true;
            found_uid = false;
            found_recid = false;
            buf.clear();
            buf.push(src_line.to_string());
            continue;
        }

        if !in_event {
            out.push_str(src_line);
            continue;
        }

        if upper14.starts_with("END:VEVENT") {
            if found_uid && found_recid {
                let mut had_summary = false;
                let mut had_description = false;
                let mut had_location = false;
                let mut had_dtstart = false;
                let mut had_dtend   = false;
                for line in &buf {
                    let head: String = line.trim_start().chars().take(16)
                        .collect::<String>().to_ascii_uppercase();
                    if head.starts_with("DTSTAMP:") {
                        out.push_str(&format!("DTSTAMP:{dtstamp_now}"));
                        out.push_str(eol);
                    } else if head.starts_with("SUMMARY:") {
                        if let Some(v) = new_summary {
                            out.push_str(&format!("SUMMARY:{}", escape_ics_text(v)));
                            out.push_str(eol);
                        } else {
                            out.push_str(line);
                        }
                        had_summary = true;
                    } else if head.starts_with("DESCRIPTION:") {
                        if let Some(v) = new_description {
                            out.push_str(&format!("DESCRIPTION:{}", escape_ics_text(v)));
                            out.push_str(eol);
                        } else {
                            out.push_str(line);
                        }
                        had_description = true;
                    } else if head.starts_with("LOCATION:") {
                        if let Some(v) = new_location {
                            out.push_str(&format!("LOCATION:{}", escape_ics_text(v)));
                            out.push_str(eol);
                        } else {
                            out.push_str(line);
                        }
                        had_location = true;
                    } else if head.starts_with("DTSTART:") {
                        if let Some(v) = new_dtstart {
                            out.push_str(&format!("DTSTART:{v}"));
                            out.push_str(eol);
                        } else {
                            out.push_str(line);
                        }
                        had_dtstart = true;
                    } else if head.starts_with("DTEND:") {
                        if let Some(v) = new_dtend {
                            out.push_str(&format!("DTEND:{v}"));
                            out.push_str(eol);
                        } else {
                            out.push_str(line);
                        }
                        had_dtend = true;
                    } else {
                        out.push_str(line);
                    }
                }
                if !had_summary {
                    if let Some(v) = new_summary {
                        out.push_str(&format!("SUMMARY:{}", escape_ics_text(v)));
                        out.push_str(eol);
                    }
                }
                if !had_description {
                    if let Some(v) = new_description {
                        out.push_str(&format!("DESCRIPTION:{}", escape_ics_text(v)));
                        out.push_str(eol);
                    }
                }
                if !had_location {
                    if let Some(v) = new_location {
                        out.push_str(&format!("LOCATION:{}", escape_ics_text(v)));
                        out.push_str(eol);
                    }
                }
                if !had_dtstart {
                    if let Some(v) = new_dtstart {
                        out.push_str(&format!("DTSTART:{v}"));
                        out.push_str(eol);
                    }
                }
                if !had_dtend {
                    if let Some(v) = new_dtend {
                        out.push_str(&format!("DTEND:{v}"));
                        out.push_str(eol);
                    }
                }
                out.push_str(src_line);
            } else {
                for line in &buf { out.push_str(line); }
                out.push_str(src_line);
            }
            in_event = false;
            buf.clear();
            continue;
        }

        let upper4: String = trimmed.chars().take(4).collect::<String>().to_ascii_uppercase();
        if upper4.starts_with("UID:") {
            let v = trimmed["UID:".len()..].trim();
            if v == uid_master { found_uid = true; }
        } else if upper14.starts_with("RECURRENCE-ID:") {
            let v = trimmed["RECURRENCE-ID:".len()..].trim();
            if v == target_compact { found_recid = true; }
        }
        buf.push(src_line.to_string());
    }

    if !buf.is_empty() {
        for line in &buf { out.push_str(line); }
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeSetAttendeesQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?op=add|remove` — operação a aplicar por evento. Obrigatório.
    op:     Option<String>,
    /// `?email=` — endereço do attendee a adicionar ou remover. Obrigatório.
    email:  Option<String>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/set-attendees?after=&before=&op=add|remove&email=
/// Adiciona ou remove um attendee em massa nos eventos cujo `dtstart` ∈
/// `[after, before)` (sprint #563, variante multi-valued da família
/// events-by-range/* — primeiro endpoint que manipula uma propriedade 1:N
/// do ical_raw em vez de uma coluna SQL simples). Attendees ficam no
/// `ical_raw` como linhas `ATTENDEE[;params]:mailto:{email}` dentro do
/// VEVENT — não há tabela `event_attendees` separada neste serviço.
///
/// `?op=add`: insere `ATTENDEE;RSVP=TRUE:mailto:{email}` antes de
/// END:VEVENT via `inject_exdate_line` adaptado. Idempotente: se o email
/// já está presente como ATTENDEE no evento → pula (não adiciona duplicata).
/// `?op=remove`: remove a linha ATTENDEE cujo mailto: valor confere com
/// o email (case-insensitive). Idempotente: se o email não está presente
/// → pula.
///
/// Em ambos os casos, eventos afetados são salvos via `EventRepo::update`
/// (incrementa SEQUENCE, regenera ETag — sinal de mudança pra clientes
/// CalDAV). Trade-off cross-channel: `ical_raw` é a fonte de attendees
/// (coluna `attendees` não existe no schema); `EventRepo::update` re-parseia
/// e salva o raw inteiro — coluna autoritativa pra futura API GET /attendees.
/// iTIP outbound NÃO disparado (explicitamente, paralelo do #552/#554) —
/// UI que quiser notificar attendees adicionados usa `resend-itip` (#562)
/// após este endpoint. Retorna `{calendar_id, op, email, events_scanned,
/// events_updated}`. WRITE+ via `assert_can_write`. `after >= before` → 400.
/// `email` e `op` obrigatórios → 400 se ausentes. Email vazio → 400.
async fn events_by_range_set_attendees(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeSetAttendeesQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let op = match q.op.as_deref() {
        Some("add")    => "add",
        Some("remove") => "remove",
        Some(other)    => return Err(CalendarError::BadRequest(
            format!("op must be 'add' or 'remove', got '{other}'")
        )),
        None => return Err(CalendarError::BadRequest("op is required".into())),
    };
    let email = match q.email.as_deref().filter(|s| !s.trim().is_empty()) {
        Some(e) => e.trim().to_ascii_lowercase(),
        None    => return Err(CalendarError::BadRequest("email is required".into())),
    };

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let events = EventRepo::new(pool)
        .list(
            ctx.tenant_id,
            cal_id,
            &EventQuery { from: q.after, to: q.before, limit: None },
        )
        .await?;

    let mut events_scanned: u64 = 0;
    let mut events_updated: u64 = 0;

    for ev in events {
        let dtstart = match ev.dtstart {
            Some(ds) => ds,
            None => continue,
        };
        if let Some(a) = q.after  { if dtstart <  a { continue; } }
        if let Some(b) = q.before { if dtstart >= b { continue; } }

        events_scanned += 1;

        // Check current attendees from ical_raw.
        let attendees = crate::domain::itip::parse_attendees(&ev.ical_raw);
        let already_present = attendees.iter().any(|a| a.email.to_ascii_lowercase() == email);

        let new_raw = if op == "add" {
            if already_present { continue; } // idempotent skip
            let line = format!("ATTENDEE;RSVP=TRUE:mailto:{email}");
            inject_exdate_line(&ev.ical_raw, &line) // same injection point: before END:VEVENT
        } else {
            // op == "remove"
            if !already_present { continue; } // idempotent skip
            remove_attendee_line(&ev.ical_raw, &email)
        };

        EventRepo::new(pool)
            .update(ctx.tenant_id, ev.id, &new_raw)
            .await?;
        events_updated += 1;
    }

    Ok(Json(serde_json::json!({
        "calendar_id":    cal_id,
        "op":             op,
        "email":          email,
        "events_scanned": events_scanned,
        "events_updated": events_updated,
    })))
}

/// Remove a linha `ATTENDEE[;params]:mailto:{target_email}` do primeiro VEVENT.
/// Case-insensitive no email. Mantém EOL original. Remove APENAS a primeira
/// linha matching — em teoria cada email aparece uma vez por VEVENT (RFC 5545
/// não proíbe duplicatas mas `add` é idempotente, então duplicatas não surgem
/// pelo nosso stack).
fn remove_attendee_line(raw: &str, target_email: &str) -> String {
    let target_lower = target_email.to_ascii_lowercase();
    let mut out = String::with_capacity(raw.len());
    let mut removed = false;
    for src_line in raw.split_inclusive('\n') {
        let trimmed = src_line.trim_start();
        let upper9: String = trimmed.chars().take(9).collect::<String>().to_ascii_uppercase();
        if !removed && upper9.starts_with("ATTENDEE") {
            // Line is "ATTENDEE[;params]:mailto:email" or "ATTENDEE:mailto:email"
            if let Some(colon_pos) = trimmed.find(':') {
                let rest = &trimmed[colon_pos + 1..];
                let mailto_lower = rest.trim_end_matches('\r').trim().to_ascii_lowercase();
                let email_part = mailto_lower
                    .strip_prefix("mailto:")
                    .unwrap_or(&mailto_lower);
                if email_part == target_lower {
                    removed = true;
                    continue; // drop this line
                }
            }
        }
        out.push_str(src_line);
    }
    out
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeResendItipQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    /// `?kind=request|cancel` — método iMIP a disparar. `request` (default)
    /// envia REQUEST a todos os attendees (re-convite). `cancel` envia CANCEL
    /// (útil após set-status CANCELLED em massa via #547).
    #[serde(default)]
    kind: Option<String>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/resend-itip?after=&before=&kind=request|cancel
/// Re-envia iMIP envelope (via NATS JetStream → expresso-imip-dispatch) para
/// todos os eventos cujo `dtstart` ∈ `[after, before)` no calendário (sprint
/// #562, itip-resend hook da família events-by-range/* — complemento dos
/// bulk-set #547 set-status/CANCEL, #552 set-organizer-email, #554
/// set-text-fields que mudam campos observáveis no invite mas NÃO disparam
/// iTIP automaticamente). Útil pra "re-enviar convites em massa" após bulk-set
/// sem editar os raws individualmente.
///
/// `?kind=request` (default): publica METHOD=REQUEST — re-convite a todos os
/// attendees (attendees já confirmados recebem o convite atualizado;
/// semantics de SEQUENCE: cada `EventRepo::update` já incrementa SEQUENCE,
/// então um request pós bulk-set carrega o sequence correto do DB).
/// `?kind=cancel`: publica METHOD=CANCEL — útil após set-status=CANCELLED
/// em massa (RFC 5546 §3.2.5: organizer envia CANCEL pra todos os attendees
/// quando evento é cancelado). Qualquer outro valor → 400.
///
/// Eventos sem attendees, sem dtstart/dtend ou sem organizer_email são
/// silenciosamente pulados por `publish_imip` (comportamento existente).
/// Retorna `{calendar_id, kind, dispatched, skipped}` — `dispatched` conta
/// eventos pra os quais o envelope foi enfileirado no JetStream; `skipped`
/// conta eventos sem dtstart no range OU sem JetStream configurado. Se NATS
/// não está conectado → todos viram skipped, campo `note` explica. WRITE+
/// via `assert_can_write`. `after >= before` → 400.
async fn events_by_range_resend_itip(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeResendItipQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let method: &'static str = match q.kind.as_deref() {
        None | Some("") | Some("request") => "REQUEST",
        Some("cancel")                    => "CANCEL",
        Some(other) => return Err(CalendarError::BadRequest(
            format!("kind must be 'request' or 'cancel', got '{other}'")
        )),
    };

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let events = EventRepo::new(pool)
        .list(
            ctx.tenant_id,
            cal_id,
            &EventQuery { from: q.after, to: q.before, limit: None },
        )
        .await?;

    let mut dispatched: u64 = 0;
    let mut skipped:    u64 = 0;

    for ev in events {
        let dtstart = match ev.dtstart {
            Some(ds) => ds,
            None => { skipped += 1; continue; }
        };
        if let Some(a) = q.after  { if dtstart <  a { skipped += 1; continue; } }
        if let Some(b) = q.before { if dtstart >= b { skipped += 1; continue; } }

        if state.events().publish_imip(ev, method) {
            dispatched += 1;
        } else {
            skipped += 1;
        }
    }

    let mut resp = serde_json::json!({
        "calendar_id": cal_id,
        "kind":        method.to_ascii_lowercase(),
        "dispatched":  dispatched,
        "skipped":     skipped,
    });
    if dispatched == 0 && skipped > 0 {
        resp["note"] = serde_json::json!(
            "all events skipped — NATS JetStream may not be configured or events lack required fields"
        );
    }
    Ok(Json(resp))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeReindexFtsQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/reindex-fts?after=&before=
/// Re-indexa no Tantivy (via expresso-search) os eventos cujo `dtstart` ∈
/// `[after, before)` no calendário (sprint #561). Complemento dos bulk-set
/// endpoints #549/#551/#554 que deixam o índice FTS stale quando mudam
/// summary/description/location sem chamar search: aqui UI chama
/// explicitamente pós bulk-set pra sincronizar freshness vs latency de
/// reindex. Cada evento vira 1 `IndexDoc` (`kind="calendar"`, `document_id=
/// "calendar/{event_id}"`, `subject=summary`, `from_addr=organizer_email`,
/// `body=description`). Chama `POST {SEARCH__URL}/api/v1/index/bulk` em
/// lotes de até 500 docs (cap do bulk endpoint). Retorna `{calendar_id,
/// indexed, skipped}` — `skipped` conta eventos sem dtstart no range ou
/// quando SEARCH__URL não está configurado (no-op gracioso). WRITE+ via
/// `assert_can_write` (reindex muda dados externos ao DB, exige mesma ACL
/// que mutations). `after >= before` → 400. Eventos sem dtstart → pulados
/// silenciosamente. Síncrono: responde quando o(s) lote(s) terminam —
/// latência proporcional ao tamanho do range (trade-off: bulk sync > N
/// fire-and-forget individuais pra UI que precisa de confirmação de freshness).
async fn events_by_range_reindex_fts(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeReindexFtsQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let search_url = state.search_url().to_owned();
    if search_url.is_empty() {
        return Ok(Json(serde_json::json!({
            "calendar_id": cal_id,
            "indexed":     0,
            "skipped":     0,
            "note":        "SEARCH__URL not configured — no-op",
        })));
    }

    let events = EventRepo::new(pool)
        .list(
            ctx.tenant_id,
            cal_id,
            &EventQuery { from: q.after, to: q.before, limit: None },
        )
        .await?;

    let mut docs: Vec<serde_json::Value> = Vec::new();
    let mut skipped: u64 = 0;

    for ev in &events {
        let dtstart = match ev.dtstart {
            Some(ds) => ds,
            None => { skipped += 1; continue; }
        };
        if let Some(a) = q.after  { if dtstart <  a { skipped += 1; continue; } }
        if let Some(b) = q.before { if dtstart >= b { skipped += 1; continue; } }

        docs.push(serde_json::json!({
            "document_id": format!("calendar/{}", ev.id),
            "tenant_id":   ctx.tenant_id.to_string(),
            "subject":     ev.summary,
            "from_addr":   ev.organizer_email,
            "body":        ev.description,
            "kind":        "calendar",
        }));
    }

    let indexed_total = docs.len() as u64;
    let search_token = state.search_token().to_owned();
    let client = reqwest::Client::new();

    // Bulk in batches of 500 (search service cap).
    for chunk in docs.chunks(500) {
        let payload = serde_json::json!({ "documents": chunk });
        let mut req = client
            .post(format!("{search_url}/api/v1/index/bulk"))
            .json(&payload);
        if !search_token.is_empty() {
            req = req.bearer_auth(&search_token);
        }
        req.send().await.ok(); // best-effort; search errors don't fail the calendar op
    }

    Ok(Json(serde_json::json!({
        "calendar_id": cal_id,
        "indexed":     indexed_total,
        "skipped":     skipped,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsByRangeCleanupOrphansQuery {
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    before: Option<OffsetDateTime>,
    #[serde(default)]
    dry: Option<bool>,
}

/// PATCH /api/v1/calendars/:cal_id/events-by-range/cleanup-orphans?after=&before=&dry=
/// Remove EXDATEs e RECURRENCE-ID overrides órfãos de eventos recorrentes cujo
/// `dtstart` ∈ `[after, before)` (sprint #560). Após um `set-rrule` (#553) em
/// massa, EXDATEs e overrides que apontavam pra ocorrências que não existem mais
/// na nova RRULE ficam dormentes no `ical_raw` — nunca são expandidos nem
/// exibidos, mas infla o raw e pode confundir clientes CalDAV. Este endpoint
/// detecta e remove esses dangling anchors.
///
/// Lógica por evento:
/// 1. Se evento não tem RRULE → pula (sem recorrência = não há órfãos).
/// 2. Expande a RRULE atual numa janela de 2 anos a partir do `dtstart` do
///    master (cap 1000 ocorrências via `Rrule::expand`).
/// 3. Coleta EXDATEs via `parse_exdates` (UTC-only — formato MVP).
/// 4. Coleta RECURRENCE-IDs via `list_recurrence_id_overrides`.
/// 5. EXDATE é órfão se não coincide com nenhuma ocorrência expandida
///    (timestamp equality com tolerância zero — mesma semântica do expander).
/// 6. Override é órfão se RECURRENCE-ID não coincide com nenhuma ocorrência.
/// 7. Se `?dry=true` (default false): contabiliza sem persistir — útil pra
///    UI mostrar preview antes do commit.
/// 8. Se `?dry=false`: salva raw limpo via `EventRepo::update` por evento
///    afetado (cada update incrementa SEQUENCE e regenera ETag).
///
/// Retorna `{calendar_id, dry, events_scanned, events_cleaned,
/// orphan_exdates_removed, orphan_overrides_removed}`.
/// WRITE+ via `assert_can_write`. `after >= before` → 400. Eventos sem
/// `dtstart` ou sem RRULE parseável são contados em `events_scanned` mas
/// não em `events_cleaned`.
async fn events_by_range_cleanup_orphans(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(cal_id): Path<Uuid>,
    Query(q):     Query<EventsByRangeCleanupOrphansQuery>,
) -> Result<Json<serde_json::Value>> {
    if let (Some(a), Some(b)) = (q.after, q.before) {
        if a >= b {
            return Err(CalendarError::BadRequest("after must be < before".into()));
        }
    }
    let dry = q.dry.unwrap_or(false);
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;

    let events = EventRepo::new(pool)
        .list(
            ctx.tenant_id,
            cal_id,
            &EventQuery { from: q.after, to: q.before, limit: None },
        )
        .await?;

    // Window for RRULE expansion: 2 years from now, capped by Rrule::expand's 1000-iter guard.
    let win_from = time::OffsetDateTime::UNIX_EPOCH;
    let win_to   = time::OffsetDateTime::now_utc() + time::Duration::days(365 * 2);

    let mut events_scanned:          u64 = 0;
    let mut events_cleaned:          u64 = 0;
    let mut orphan_exdates_removed:  u64 = 0;
    let mut orphan_overrides_removed: u64 = 0;

    for ev in events {
        // Only master events with dtstart in the requested range.
        let dtstart = match ev.dtstart {
            Some(ds) => ds,
            None     => continue,
        };
        if let Some(a) = q.after  { if dtstart <  a { continue; } }
        if let Some(b) = q.before { if dtstart >= b { continue; } }

        events_scanned += 1;

        let rrule_str = match ev.rrule.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(s) => s,
            None    => continue, // no recurrence → no orphans possible
        };
        let rrule = match crate::domain::rrule::Rrule::parse(rrule_str) {
            Some(r) => r,
            None    => continue, // unsupported FREQ — can't expand; skip safely
        };

        let duration = ev.dtend
            .map(|e| e - dtstart)
            .unwrap_or(time::Duration::ZERO);
        let occurrences = rrule.expand(dtstart, duration, win_from, win_to);

        // Set of occurrence starts for O(1) lookup.
        let occ_set: std::collections::HashSet<i128> = occurrences
            .iter()
            .map(|(s, _)| s.unix_timestamp_nanos())
            .collect();

        // ── EXDATE orphans ─────────────────────────────────────────────────
        let exdates = parse_exdates(&ev.ical_raw);
        let orphan_exdates: Vec<OffsetDateTime> = exdates
            .into_iter()
            .filter(|t| !occ_set.contains(&t.unix_timestamp_nanos()))
            .collect();

        // ── Override orphans ───────────────────────────────────────────────
        let uid = extract_uid(&ev.ical_raw).unwrap_or_default();
        let overrides = list_recurrence_id_overrides(&ev.ical_raw, &uid, false);
        let orphan_overrides: Vec<String> = overrides
            .iter()
            .filter_map(|item| item.get("compact").and_then(|v| v.as_str()).map(|s| s.to_owned()))
            .filter(|compact| {
                match parse_one_exdate(compact) {
                    Some(t) => !occ_set.contains(&t.unix_timestamp_nanos()),
                    None    => false, // non-UTC recurrence-id: can't compare → keep
                }
            })
            .collect();

        if orphan_exdates.is_empty() && orphan_overrides.is_empty() {
            continue;
        }

        orphan_exdates_removed  += orphan_exdates.len() as u64;
        orphan_overrides_removed += orphan_overrides.len() as u64;
        events_cleaned           += 1;

        if !dry {
            // Apply removals in-memory then persist once per event.
            let mut new_raw = ev.ical_raw.clone();
            for t in &orphan_exdates {
                new_raw = remove_exdate_value(&new_raw, *t);
            }
            for compact in &orphan_overrides {
                new_raw = remove_recurrence_id_override_block(&new_raw, &uid, compact);
            }
            EventRepo::new(pool)
                .update(ctx.tenant_id, ev.id, &new_raw)
                .await?;
        }
    }

    Ok(Json(serde_json::json!({
        "calendar_id":             cal_id,
        "dry":                     dry,
        "events_scanned":          events_scanned,
        "events_cleaned":          events_cleaned,
        "orphan_exdates_removed":  orphan_exdates_removed,
        "orphan_overrides_removed": orphan_overrides_removed,
    })))
}

/// GET /api/v1/calendars/events/search?q=&limit=&offset= — full-text search cross-calendar.
///
/// Delega para o expresso-search service via `GET /api/v1/search?q=&tenant_id=&limit=&offset=`.
/// Retorna `{hits: [{document_id, subject, score, ...}], count}`.
/// 503 se SEARCH__URL não configurado. Parâmetros: `q` (obrigatório), `limit`
/// (default 20, máx 200), `offset` (default 0). Sprint #588.
async fn events_search(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(params): Query<EventsSearchParams>,
) -> Result<Json<serde_json::Value>> {
    use serde_json::json;

    let search_url = state.search_url().to_owned();
    if search_url.is_empty() {
        return Err(CalendarError::BadRequest(
            "SEARCH__URL not configured — search unavailable".into(),
        ));
    }

    if params.q.trim().is_empty() {
        return Err(CalendarError::BadRequest("q is required".into()));
    }

    let limit  = params.limit.unwrap_or(20).min(200);
    let offset = params.offset.unwrap_or(0);

    let url = format!(
        "{search_url}/api/v1/search?q={q}&tenant_id={tenant}&limit={limit}&offset={offset}",
        q      = urlencoding::encode(params.q.trim()),
        tenant = ctx.tenant_id,
    );

    let client = reqwest::Client::new();
    let mut req = client.get(&url);
    let token = state.search_token().to_owned();
    if !token.is_empty() {
        req = req.bearer_auth(&token);
    }

    let resp = req.send().await.map_err(|e| {
        CalendarError::BadRequest(format!("search service unreachable: {e}"))
    })?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(CalendarError::BadRequest(format!(
            "search service returned {status}: {body}"
        )));
    }

    let result: serde_json::Value = resp.json().await.map_err(|e| {
        CalendarError::BadRequest(format!("failed to parse search response: {e}"))
    })?;

    Ok(Json(json!({
        "q":      params.q.trim(),
        "limit":  limit,
        "offset": offset,
        "result": result,
    })))
}

#[derive(Debug, serde::Deserialize)]
struct EventsSearchParams {
    q:      String,
    limit:  Option<u32>,
    offset: Option<u32>,
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

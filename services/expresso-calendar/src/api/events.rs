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
        .route(
            "/api/v1/calendars/:cal_id/events/:id/instances",
            get(events_instances),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/cancel-instance",
            post(cancel_event_instance),
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

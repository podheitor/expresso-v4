//! Event REST endpoints (JSON out, text/calendar in for POST/PUT).

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete as delete_route, get, post},
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
        .route(
            "/api/v1/calendars/:cal_id/events/:id/exdates",
            get(list_exdates).delete(clear_exdates),
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
            "/api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id",
            get(get_one_override).delete(delete_override).patch(patch_override),
        )
        .route(
            "/api/v1/calendars/:cal_id/events/:id/overrides/:recurrence_id/cancel",
            post(migrate_override_to_cancel),
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
/// importado tem EXDATEs em formato não-MVP. Não requer WRITE
/// (read-only). 400 em valor de detail desconhecido. 404 se evento não
/// existe.
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
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;

    let items: Vec<serde_json::Value> = if full {
        parse_exdates_rich(&ev.ical_raw).into_iter().map(|info| {
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
}

/// GET /api/v1/calendars/:cal_id/events/:id/overrides — lista os
/// RECURRENCE-ID overrides existentes no VCALENDAR (sprint #496, paralelo
/// ao EXDATE list #491). Retorna `{event_id, count, overrides:[{compact,
/// rfc3339, summary?, dtstart?, dtend?}]}` por default. Com `?detail=full`
/// (sprint #503) adiciona `description?` + `location?` em cada item pra
/// paridade com get-one (#500) — útil pra UI que precisa exibir lista
/// completa sem N+1 GETs por override. Reusa `extract_uid` do master e
/// walk por blocos VEVENT pareando UID + RECURRENCE-ID. Read-only,
/// não exige WRITE+. 404 se evento não existe. 400 se detail desconhecido.
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
    let pool = state.db_or_unavailable()?;
    let ev = EventRepo::new(pool).get(ctx.tenant_id, id).await?;
    let uid = extract_uid(&ev.ical_raw).unwrap_or_default();
    let items = list_recurrence_id_overrides(&ev.ical_raw, &uid, full);
    Ok(Json(serde_json::json!({
        "event_id":  ev.id,
        "count":     items.len(),
        "overrides": items,
    })))
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

    let mut out = Vec::new();
    let mut in_event = false;
    let mut found_uid = false;
    let mut cur_recid:       Option<String> = None;
    let mut cur_summary:     Option<String> = None;
    let mut cur_dtstart:     Option<String> = None;
    let mut cur_dtend:       Option<String> = None;
    let mut cur_description: Option<String> = None;
    let mut cur_location:    Option<String> = None;

    for line in raw.lines() {
        let trimmed = line.trim_start();
        let upper16: String = trimmed.chars().take(16).collect::<String>().to_ascii_uppercase();
        if upper16.starts_with("BEGIN:VEVENT") {
            in_event = true;
            found_uid = false;
            cur_recid = None; cur_summary = None; cur_dtstart = None; cur_dtend = None;
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
                        "compact":  rec,
                        "rfc3339":  rfc,
                        "summary":  cur_summary.take(),
                        "dtstart":  cur_dtstart.take(),
                        "dtend":    cur_dtend.take(),
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

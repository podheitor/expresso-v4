//! Calendar collection REST endpoints (JSON).

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Calendar, CalendarRepo, NewCalendar, UpdateCalendar};
use crate::error::Result;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/calendars/stats/by-tenant", get(stats_by_tenant))
        .route(
            "/api/v1/calendars/stats/event-density",
            get(stats_event_density),
        )
        .route("/api/v1/calendars", post(create).get(list))
        .route(
            "/api/v1/calendars/:id",
            get(get_one).delete(delete).patch(update),
        )
        .route("/api/v1/calendars/:id/ctag", get(ctag_one))
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewCalendar>,
) -> Result<(StatusCode, Json<Calendar>)> {
    let pool = state.db_or_unavailable()?;
    let cal = CalendarRepo::new(pool)
        .create(ctx.tenant_id, ctx.user_id, &body)
        .await?;
    Ok((StatusCode::CREATED, Json(cal)))
}

async fn list(state: State<AppState>, ctx: RequestCtx, req_headers: HeaderMap) -> Result<Response> {
    let r = list_inner(state, ctx, req_headers).await;
    crate::metrics::record_result(crate::metrics::OP_CALENDAR_LIST, &r);
    r
}

async fn list_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    req_headers: HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let (total, max_updated): (i64, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(updated_at) FROM calendars WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) =
                    OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822)
                {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let cals = CalendarRepo::new(pool)
        .list_accessible(ctx.tenant_id, ctx.user_id)
        .await?;
    let mut resp = (
        [(
            header::HeaderName::from_static("x-total-count"),
            total.to_string(),
        )],
        Json(cals),
    )
        .into_response();
    if let Some(ts) = max_updated {
        let lm = ts
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap_or_default();
        resp.headers_mut()
            .insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    req_headers: axum::http::HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let cal = CalendarRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}-{}\"", cal.updated_at.unix_timestamp(), cal.id);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    let lm = cal
        .updated_at
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) =
                OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822)
            {
                if cal.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(cal).into_response();
    resp.headers_mut()
        .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut()
        .insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateCalendar>,
) -> Result<Json<Calendar>> {
    let pool = state.db_or_unavailable()?;
    let cal = CalendarRepo::new(pool)
        .update(ctx.tenant_id, id, &body)
        .await?;
    Ok(Json(cal))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    CalendarRepo::new(pool).delete(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ctag_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let ctag = CalendarRepo::new(pool).ctag(ctx.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "id": id, "ctag": ctag })))
}

/// GET /api/v1/calendars/stats/by-tenant?limit=N — contagem de calendars+events por tenant_id.
///
/// Endpoint ops cross-tenant (sem RequestCtx/RLS). Retorna
/// `{rows:[{tenant_id,calendar_count,event_count}]}` ordenado por `calendar_count DESC`.
/// `limit` default 20 max 200. Sprint #680.
#[derive(Debug, Deserialize)]
struct StatsByTenantQuery {
    limit: Option<i64>,
}

async fn stats_by_tenant(
    State(state): State<AppState>,
    Query(q): Query<StatsByTenantQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    let rows: Vec<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT c.tenant_id, \
                COUNT(DISTINCT c.id)::BIGINT AS calendar_count, \
                COUNT(e.id)::BIGINT          AS event_count \
           FROM calendars c \
           LEFT JOIN calendar_events e ON e.calendar_id = c.id AND e.tenant_id = c.tenant_id \
          GROUP BY c.tenant_id \
          ORDER BY calendar_count DESC, c.tenant_id ASC \
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(tid, cc, ec)| {
            serde_json::json!({
                "tenant_id":      tid,
                "calendar_count": cc,
                "event_count":    ec,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

/// GET /api/v1/calendars/stats/event-density?bucket=day|week|month — eventos por bucket temporal.
///
/// Cross-tenant: total de eventos em `calendar_events` agrupados por DATE_TRUNC(bucket, dtstart).
/// `bucket` aceita "day" (default), "week", "month". Retorna `{bucket,rows:[{period,count}]}` ASC.
/// Sprint #709.
#[derive(Debug, Deserialize)]
struct EventDensityQuery {
    bucket: Option<String>,
}

async fn stats_event_density(
    State(state): State<AppState>,
    Query(q): Query<EventDensityQuery>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let bucket = match q.bucket.as_deref().unwrap_or("day") {
        "week" => "week",
        "month" => "month",
        _ => "day",
    };
    let fmt = match bucket {
        "week" => "IYYY-\"W\"IW",
        "month" => "YYYY-MM",
        _ => "YYYY-MM-DD",
    };
    let sql = format!(
        "SELECT to_char(date_trunc('{bucket}', dtstart), '{fmt}') AS period, \
                COUNT(*)::BIGINT AS count \
           FROM calendar_events \
          WHERE dtstart IS NOT NULL \
          GROUP BY period \
          ORDER BY period ASC"
    );

    let rows: Vec<(String, i64)> = sqlx::query_as(&sql).fetch_all(pool).await?;

    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(period, count)| serde_json::json!({"period": period, "count": count}))
        .collect();
    Ok(Json(serde_json::json!({"bucket": bucket, "rows": out})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_by_tenant_query_default_limit() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.limit.is_none());
        assert_eq!(q.limit.unwrap_or(20).clamp(1, 200), 20);
    }

    #[test]
    fn stats_by_tenant_query_clamped_max() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":999}"#).unwrap();
        assert_eq!(q.limit.unwrap_or(20).clamp(1, 200), 200);
    }

    #[test]
    fn event_density_query_bucket_optional() {
        let q: EventDensityQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.bucket.is_none());
    }

    #[test]
    fn event_density_query_bucket_set() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"month"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("month"));
    }

    #[test]
    fn stats_by_tenant_query_in_range_limit() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":50}"#).unwrap();
        assert_eq!(q.limit.unwrap_or(20).clamp(1, 200), 50);
    }

    #[test]
    fn event_density_query_bucket_week() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"week"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("week"));
    }

    #[test]
    fn event_density_query_bucket_absent_is_none() {
        let q: EventDensityQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.bucket.is_none());
    }

    #[test]
    fn event_density_query_bucket_month_set() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"month"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("month"));
    }

    #[test]
    fn event_density_query_bucket_year_set() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"year"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("year"));
    }

    #[test]
    fn event_density_query_bucket_none_when_absent() {
        let q: EventDensityQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.bucket.is_none());
    }

    #[test]
    fn event_density_query_bucket_day_preserved() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"day"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("day"));
    }

    #[test]
    fn event_density_query_bucket_month_preserved() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"month"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("month"));
    }

    #[test]
    fn event_density_query_missing_bucket_is_none() {
        let q: EventDensityQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.bucket.is_none());
    }

    #[test]
    fn event_density_query_bucket_week_preserved() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"week"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("week"));
    }

    #[test]
    fn stats_by_tenant_query_limit_one_preserved() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":1}"#).unwrap();
        assert_eq!(q.limit.unwrap_or(20).clamp(1, 200), 1);
    }

    #[test]
    fn event_density_query_bucket_week_is_some() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"week"}"#).unwrap();
        assert!(q.bucket.is_some());
    }

    #[test]
    fn stats_by_tenant_query_limit_preserved_when_set() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":42}"#).unwrap();
        assert_eq!(q.limit, Some(42));
    }

    #[test]
    fn stats_by_tenant_query_limit_200_at_boundary() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":200}"#).unwrap();
        assert_eq!(q.limit.unwrap_or(20).clamp(1, 200), 200);
    }

    #[test]
    fn event_density_query_bucket_null_is_none() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":null}"#).unwrap();
        assert!(q.bucket.is_none());
    }

    #[test]
    fn stats_by_tenant_query_limit_none_by_default() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.limit.is_none());
    }

    #[test]
    fn stats_by_tenant_query_limit_fifty_preserved() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":50}"#).unwrap();
        assert_eq!(q.limit, Some(50));
    }

    #[test]
    fn event_density_query_bucket_day_is_some() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":"day"}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some("day"));
    }

    #[test]
    fn event_density_query_bucket_null_json_is_none() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":null}"#).unwrap();
        assert!(q.bucket.is_none());
    }

    #[test]
    fn stats_by_tenant_query_limit_clamped_to_min() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":0}"#).unwrap();
        assert_eq!(q.limit.unwrap_or(20).clamp(1, 200), 1);
    }

    #[test]
    fn event_density_query_bucket_empty_string_is_some() {
        let q: EventDensityQuery = serde_json::from_str(r#"{"bucket":""}"#).unwrap();
        assert_eq!(q.bucket.as_deref(), Some(""));
    }

    #[test]
    fn stats_by_tenant_query_limit_negative_clamped_to_min() {
        let q: StatsByTenantQuery = serde_json::from_str(r#"{"limit":-5}"#).unwrap();
        assert_eq!(q.limit.unwrap_or(20).clamp(1, 200), 1);
    }
}

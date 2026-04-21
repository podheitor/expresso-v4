//! Scheduling endpoints — free/busy lookup (RFC 6638 subset).
//!
//! GET /api/v1/scheduling/freebusy?attendees=a@ex.com,b@ex.com&from=…&to=…

use axum::{
    extract::{Query, State},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::api::context::RequestCtx;
use crate::domain::freebusy::{BusyInterval, FreeBusyRepo};
use crate::error::{CalendarError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/scheduling/freebusy", get(freebusy))
}

/// Query shape: comma-separated `attendees`, rfc3339 `from`/`to`.
#[derive(Debug, Deserialize)]
struct FreeBusyParams {
    attendees: String,
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to:   OffsetDateTime,
}

#[derive(Debug, Serialize)]
struct FreeBusyResp {
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to:   OffsetDateTime,
    attendees: std::collections::BTreeMap<String, Vec<BusyInterval>>,
}

async fn freebusy(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(p): Query<FreeBusyParams>,
) -> Result<impl IntoResponse> {
    if p.to <= p.from {
        return Err(CalendarError::BadRequest("`to` must be strictly after `from`".into()));
    }
    // Cap window at 370 days → bounds scan cost; covers "next year" UI needs.
    let span = p.to - p.from;
    if span > time::Duration::days(370) {
        return Err(CalendarError::BadRequest("range exceeds 370 days".into()));
    }

    let attendees: Vec<String> = p.attendees
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    if attendees.is_empty() {
        return Err(CalendarError::BadRequest("`attendees` required".into()));
    }
    if attendees.len() > 50 {
        return Err(CalendarError::BadRequest("max 50 attendees per query".into()));
    }

    let pool = state.db_or_unavailable()?;
    let map = FreeBusyRepo::new(pool)
        .lookup(ctx.tenant_id, &attendees, p.from, p.to)
        .await?;

    Ok(Json(FreeBusyResp { from: p.from, to: p.to, attendees: map }))
}

//! Internal calendar-write endpoints for the ActiveSync bridge.
//!
//! ActiveSync runs in the mail service but must not write `calendar_events`
//! directly — that would bypass this service's domain logic (iCal parse, etag,
//! ctag bump, scheduling). Instead the EAS Sync handler translates a device's
//! Calendar item to a minimal VEVENT and calls these LAN-trusted `/internal/*`
//! endpoints, which apply the write through `EventRepo`. Same trust model as the
//! auth service's `/internal/*` (reached over the internal network, no end-user
//! auth; the caller has already authenticated the device).
//!
//! POST   /internal/calendar/events   — upsert a VEVENT by UID
//! DELETE /internal/calendar/events/:id — delete an event by id

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::EventRepo;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/internal/calendar/events", post(upsert_event))
        .route(
            "/internal/calendar/events/:id",
            axum::routing::delete(delete_event),
        )
}

#[derive(Debug, Deserialize)]
pub struct UpsertEventBody {
    pub tenant_id: Uuid,
    pub calendar_id: Uuid,
    /// Full VCALENDAR/VEVENT body (the EAS bridge builds this from the device's
    /// Calendar item). Parsed + validated by `EventRepo::replace_by_uid`.
    pub ical_raw: String,
}

#[derive(Debug, Serialize)]
pub struct EventRef {
    pub id: Uuid,
    pub uid: String,
}

/// Upsert (create-or-replace by UID) an event. Mirrors what a CalDAV PUT does.
async fn upsert_event(
    State(state): State<AppState>,
    Json(body): Json<UpsertEventBody>,
) -> Result<Json<EventRef>, Response> {
    let pool = state
        .db_or_unavailable()
        .map_err(IntoResponse::into_response)?;
    let ev = EventRepo::new(pool)
        .replace_by_uid(body.tenant_id, body.calendar_id, &body.ical_raw)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
    Ok(Json(EventRef {
        id: ev.id,
        uid: ev.uid,
    }))
}

/// Delete an event by id, scoped to the tenant passed as a query param.
#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    pub tenant_id: Uuid,
}

async fn delete_event(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    axum::extract::Query(q): axum::extract::Query<DeleteQuery>,
) -> Result<StatusCode, Response> {
    let pool = state
        .db_or_unavailable()
        .map_err(IntoResponse::into_response)?;
    EventRepo::new(pool)
        .delete(q.tenant_id, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_body_deser() {
        let t = Uuid::new_v4();
        let c = Uuid::new_v4();
        let json = format!(
            r#"{{"tenant_id":"{t}","calendar_id":"{c}","ical_raw":"BEGIN:VCALENDAR\nEND:VCALENDAR"}}"#
        );
        let b: UpsertEventBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.tenant_id, t);
        assert_eq!(b.calendar_id, c);
        assert!(b.ical_raw.contains("VCALENDAR"));
    }

    #[test]
    fn event_ref_serializes() {
        let r = EventRef {
            id: Uuid::nil(),
            uid: "evt-1@x".into(),
        };
        let j = serde_json::to_string(&r).unwrap();
        assert!(j.contains("evt-1@x"));
    }
}

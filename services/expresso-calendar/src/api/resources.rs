//! Bookable-resource (room/equipment) registry + double-booking detection.
//!
//! Routes:
//!   GET    /api/v1/resources                 → list the tenant's resources
//!   POST   /api/v1/resources                 → register a resource
//!   GET    /api/v1/resources/:id             → fetch one
//!   DELETE /api/v1/resources/:id             → unregister
//!   GET    /api/v1/resources/:id/conflicts?from=&to=
//!                                            → double-bookings of this resource
//!
//! A resource's email matches the `ATTENDEE;CUTYPE=ROOM|RESOURCE` it appears as
//! in events; `EventRepo` indexes those bookings, and conflicts are the time
//! overlaps among events sharing the resource (stored dtstart/dtend, no RRULE).

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{NewResource, ResourceRepo};
use crate::error::{CalendarError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/resources", get(list).post(create))
        .route("/api/v1/resources/:id", get(get_one).delete(delete))
        .route("/api/v1/resources/:id/conflicts", get(conflicts))
}

async fn list(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let resources = ResourceRepo::new(pool).list(ctx.tenant_id).await?;
    Ok(Json(serde_json::json!({ "resources": resources })))
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewResource>,
) -> Result<(StatusCode, Json<serde_json::Value>)> {
    let pool = state.db_or_unavailable()?;
    let resource = ResourceRepo::new(pool).create(ctx.tenant_id, body).await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!(resource))))
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let resource = ResourceRepo::new(pool).get(ctx.tenant_id, id).await?;
    Ok(Json(serde_json::json!(resource)))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    ResourceRepo::new(pool).delete(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Time window for conflict detection. Both bounds required (RFC3339).
#[derive(Debug, Deserialize)]
struct ConflictsQuery {
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to: OffsetDateTime,
}

async fn conflicts(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Query(q): Query<ConflictsQuery>,
) -> Result<Json<serde_json::Value>> {
    if q.from >= q.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = ResourceRepo::new(pool);
    // 404 if the resource isn't registered in this tenant.
    let resource = repo.get(ctx.tenant_id, id).await?;
    let conflicts = repo
        .conflicts(ctx.tenant_id, &resource.email, q.from, q.to)
        .await?;
    Ok(Json(serde_json::json!({
        "resource_id":    id,
        "resource_email": resource.email,
        "conflicts":      conflicts,
    })))
}

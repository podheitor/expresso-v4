//! Read markers — unread tracking.
//!
//! PUT /api/v1/channels/:id/read   → mark the channel read up to now (member-only)
//! GET /api/v1/unread              → channel ids unread for the caller + count
//!
//! `unread` is a top-level resource (not `/channels/unread`) to avoid colliding
//! with the `GET /api/v1/channels/:id` detail route in the merged router.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use serde::Serialize;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{ChannelRepo, ReadMarkerRepo};
use crate::error::{ChatError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/channels/:id/read", put(mark_read))
        .route("/api/v1/unread", get(unread))
}

#[derive(Debug, Serialize)]
struct UnreadResponse {
    /// Channel ids with activity newer than the caller's read marker.
    channels: Vec<Uuid>,
    /// Convenience count = channels.len(), for an unread badge.
    count: usize,
}

/// Mark a channel read for the caller. Members only.
async fn mark_read(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    if !ChannelRepo::new(pool)
        .is_member(ctx.tenant_id, channel_id, ctx.user_id)
        .await?
    {
        return Err(ChatError::NotMember);
    }
    ReadMarkerRepo::new(pool)
        .mark_read(ctx.tenant_id, channel_id, ctx.user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// List the caller's unread channels (and a count) across the tenant.
async fn unread(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<UnreadResponse>> {
    let pool = state.db_or_unavailable()?;
    let channels = ReadMarkerRepo::new(pool)
        .unread_channels(ctx.tenant_id, ctx.user_id)
        .await?;
    let count = channels.len();
    Ok(Json(UnreadResponse { channels, count }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_response_count_matches_channels() {
        let ids = vec![Uuid::new_v4(), Uuid::new_v4()];
        let resp = UnreadResponse {
            channels: ids.clone(),
            count: ids.len(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"count\":2"), "got: {json}");
        assert!(json.contains("\"channels\""), "got: {json}");
    }

    #[test]
    fn unread_response_empty_serializes_zero() {
        let resp = UnreadResponse {
            channels: vec![],
            count: 0,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"count\":0"), "got: {json}");
    }
}

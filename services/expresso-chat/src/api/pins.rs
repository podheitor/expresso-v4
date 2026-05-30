//! Pinned messages — a channel's pin set is the Matrix `m.room.pinned_events`
//! state event (a list of event ids).
//!
//! GET    /api/v1/channels/:id/pins  → list pinned event ids
//! POST   /api/v1/channels/:id/pins  → pin   {event_id}
//! DELETE /api/v1/channels/:id/pins  → unpin {event_id}
//!
//! Pin/unpin is a read-modify-write of the state event; the HS enforces the
//! power level required to send state. No DB: pins live entirely in Matrix.
//! A pin change broadcasts over the SSE bus so other members update live.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::ChannelRepo;
use crate::error::{ChatError, Result};
use crate::events::ChatEvent;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/channels/:id/pins",
        get(list).post(pin).delete(unpin),
    )
}

#[derive(Debug, Deserialize)]
pub struct PinBody {
    pub event_id: String,
}

#[derive(Debug, Serialize)]
pub struct PinList {
    pub pinned: Vec<String>,
}

fn validate(body: &PinBody) -> Result<()> {
    if body.event_id.trim().is_empty() {
        return Err(ChatError::BadRequest("event_id required".into()));
    }
    Ok(())
}

/// Resolve the channel's Matrix room id, gating on membership.
async fn room_for_member(
    state: &AppState,
    ctx: &RequestCtx,
    channel: Uuid,
) -> Result<(crate::matrix::MatrixClient, String, String)> {
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, channel, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, channel)
        .await
        .map_err(|_| ChatError::ChannelNotFound(channel))?;
    let acting_as = matrix.mxid_for(ctx.user_id);
    Ok((matrix.clone(), ch.matrix_room_id, acting_as))
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
) -> Result<Json<PinList>> {
    let (matrix, room, acting_as) = room_for_member(&state, &ctx, channel_id).await?;
    let pinned = matrix.get_pinned_events(&acting_as, &room).await?;
    Ok(Json(PinList { pinned }))
}

async fn pin(
    state: State<AppState>,
    ctx: RequestCtx,
    channel_id: Path<Uuid>,
    body: Json<PinBody>,
) -> Result<(StatusCode, Json<Value>)> {
    let r = pin_inner(state, ctx, channel_id, body).await;
    crate::metrics::record_result(crate::metrics::OP_PIN, &r);
    r
}

async fn pin_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
    Json(body): Json<PinBody>,
) -> Result<(StatusCode, Json<Value>)> {
    validate(&body)?;
    let (matrix, room, acting_as) = room_for_member(&state, &ctx, channel_id).await?;
    let mut pinned = matrix.get_pinned_events(&acting_as, &room).await?;
    if pinned.contains(&body.event_id) {
        // Already pinned — idempotent, nothing to write or broadcast.
        return Ok((StatusCode::OK, Json(json!({ "pinned": pinned }))));
    }
    pinned.push(body.event_id.clone());
    matrix.set_pinned_events(&acting_as, &room, &pinned).await?;
    state.bus().publish(ChatEvent::pin(
        ctx.tenant_id,
        channel_id,
        ctx.user_id,
        body.event_id,
        true,
    ));
    Ok((StatusCode::OK, Json(json!({ "pinned": pinned }))))
}

async fn unpin(
    state: State<AppState>,
    ctx: RequestCtx,
    channel_id: Path<Uuid>,
    body: Json<PinBody>,
) -> Result<(StatusCode, Json<Value>)> {
    let r = unpin_inner(state, ctx, channel_id, body).await;
    crate::metrics::record_result(crate::metrics::OP_PIN, &r);
    r
}

async fn unpin_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
    Json(body): Json<PinBody>,
) -> Result<(StatusCode, Json<Value>)> {
    validate(&body)?;
    let (matrix, room, acting_as) = room_for_member(&state, &ctx, channel_id).await?;
    let mut pinned = matrix.get_pinned_events(&acting_as, &room).await?;
    let before = pinned.len();
    pinned.retain(|e| e != &body.event_id);
    if pinned.len() == before {
        // Wasn't pinned — idempotent, nothing to write or broadcast.
        return Ok((StatusCode::OK, Json(json!({ "pinned": pinned }))));
    }
    matrix.set_pinned_events(&acting_as, &room, &pinned).await?;
    state.bus().publish(ChatEvent::pin(
        ctx.tenant_id,
        channel_id,
        ctx.user_id,
        body.event_id,
        false,
    ));
    Ok((StatusCode::OK, Json(json!({ "pinned": pinned }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_body_deserializes() {
        let b: PinBody = serde_json::from_str(r#"{"event_id":"$evt:hs"}"#).unwrap();
        assert_eq!(b.event_id, "$evt:hs");
    }

    #[test]
    fn pin_body_missing_field_fails_deser() {
        let r: std::result::Result<PinBody, _> = serde_json::from_str(r#"{}"#);
        assert!(r.is_err());
    }

    #[test]
    fn validate_accepts_event_id() {
        assert!(validate(&PinBody {
            event_id: "$evt:hs".into()
        })
        .is_ok());
    }

    #[test]
    fn validate_rejects_empty_event_id() {
        let err = format!(
            "{:?}",
            validate(&PinBody {
                event_id: "  ".into()
            })
            .unwrap_err()
        );
        assert!(err.contains("event_id required"), "got: {err}");
    }

    #[test]
    fn pin_list_serializes_pinned_array() {
        let json = serde_json::to_string(&PinList {
            pinned: vec!["$a:hs".into(), "$b:hs".into()],
        })
        .unwrap();
        assert!(json.contains("\"pinned\""), "got: {json}");
        assert!(json.contains("$a:hs"), "got: {json}");
    }
}

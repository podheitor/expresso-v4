//! SSE endpoint streaming realtime events for one channel.
//!
//! GET /api/v1/channels/:id/messages/stream — verifies the caller is a member
//! of the channel, then keeps the connection open and emits one SSE `event:`
//! per `ChatEvent` published to the in-process bus for that tenant + channel.
//! Browsers consume it via `EventSource` (auto-reconnect on drop). A 15s
//! keep-alive comment survives idle NAT/proxy timeouts.
//!
//! Durable history is fetched separately (`GET …/messages`); this stream is
//! live delivery only. On reconnect a client re-lists to fill any gap.

use std::{convert::Infallible, time::Duration};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use futures::stream::{Stream, StreamExt};
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::ChannelRepo;
use crate::error::{ChatError, Result};
use crate::events::ChatEvent;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/channels/:id/messages/stream", get(stream))
        .route("/api/v1/channels/:id/typing", post(typing))
}

/// Open the per-channel SSE stream. Membership is checked once here (an async
/// DB call); the per-event filter inside the stream is a cheap sync comparison.
async fn stream(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
) -> Result<Sse<impl Stream<Item = std::result::Result<SseEvent, Infallible>>>> {
    let pool = state.db_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo
        .is_member(ctx.tenant_id, channel_id, ctx.user_id)
        .await?
    {
        return Err(ChatError::NotMember);
    }

    let rx = state.bus().subscribe();
    let tenant_id = ctx.tenant_id;

    let s = BroadcastStream::new(rx).filter_map(move |msg| async move {
        match msg {
            Ok(ev) if ev.tenant_id == tenant_id && ev.channel_id == channel_id => {
                Some(Ok::<_, Infallible>(sse_event(&ev)))
            }
            // Lagged receiver or an event for another tenant/channel: skip.
            _ => None,
        }
    });

    Ok(Sse::new(s).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

/// Render a `ChatEvent` as an SSE frame: the event name is the kind
/// (`message`/`typing`) so clients can `addEventListener` per kind.
fn sse_event(ev: &ChatEvent) -> SseEvent {
    let data = serde_json::to_string(ev).unwrap_or_else(|_| "{}".into());
    SseEvent::default().event(ev.kind.as_str()).data(data)
}

/// POST /api/v1/channels/:id/typing — publish an ephemeral "user is typing"
/// signal to the channel's live subscribers. Never stored; members-only.
async fn typing(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
) -> Result<(StatusCode, Json<Value>)> {
    let pool = state.db_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo
        .is_member(ctx.tenant_id, channel_id, ctx.user_id)
        .await?
    {
        return Err(ChatError::NotMember);
    }
    let delivered = state
        .bus()
        .publish(ChatEvent::typing(ctx.tenant_id, channel_id, ctx.user_id));
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "delivered": delivered })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_event_uses_kind_as_event_name() {
        let t = Uuid::new_v4();
        let c = Uuid::new_v4();
        let u = Uuid::new_v4();
        // We can't read SseEvent fields back, but we can assert the serialized
        // ChatEvent payload that feeds it is well-formed for each kind.
        let msg = ChatEvent::message(t, c, u, "$e:hs".into(), "hi".into());
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"message\""));
        assert!(json.contains("\"body\":\"hi\""));

        let typ = ChatEvent::typing(t, c, u);
        let json = serde_json::to_string(&typ).unwrap();
        assert!(json.contains("\"kind\":\"typing\""));
    }
}

//! SSE endpoint streaming per-tenant calendar events.
//!
//! GET /api/v1/events/stream — keeps connection open; emits one `event:` per
//! publication on the process-local `EventBus`. Clients auto-reconnect via
//! EventSource. Heartbeat every 15s to survive idle NAT/proxy timeouts.

use std::{convert::Infallible, time::Duration};

use axum::{
    extract::State,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
};
use futures::stream::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use crate::{api::context::RequestCtx, state::AppState};

pub async fn stream(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.events().subscribe();
    let tenant_id = ctx.tenant_id;

    let s = BroadcastStream::new(rx)
        .filter_map(move |msg| async move {
            match msg {
                Ok(ev) if ev.tenant_id() == tenant_id => {
                    let json = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
                    Some(Ok::<_, Infallible>(SseEvent::default().event("calendar").data(json)))
                }
                _ => None, // lagged / other tenants
            }
        });

    Sse::new(s).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

pub fn routes() -> axum::Router<AppState> {
    use axum::routing::get;
    axum::Router::new().route("/api/v1/events/stream", get(stream))
}

//! Axum HTTP router for expresso-calendar

pub mod context;
mod calendars;
mod events;
mod health;
mod scheduling;
mod sharing;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    // JSON API: recebe CORS + compressão + tracing.
    let api = Router::new()
        .merge(health::routes())
        .merge(calendars::routes())
        .merge(events::routes())
        .merge(scheduling::routes())
        .merge(sharing::routes())
        .layer(CorsLayer::permissive());

    // CalDAV: ≠ passa por CorsLayer (senão OPTIONS é sequestrado
    // e response perde headers `DAV:`/`Allow:` exigidos pelo protocolo).
    Router::new()
        .merge(api)
        .merge(crate::caldav::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .with_state(state)
}

//! Axum HTTP router for expresso-calendar

pub mod context;
mod calendars;
mod events;
mod health;
mod scheduling;
mod stream;
mod sharing;
mod users;
mod wellknown;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    // JSON API: recebe CORS + compressão + tracing.
    let api = Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .merge(calendars::routes())
        .merge(events::routes())
        .merge(scheduling::routes())
        .merge(stream::routes())
        .merge(sharing::routes())
        .merge(users::routes())
        .merge(wellknown::routes())
        .layer(CorsLayer::permissive());

    // CalDAV: ≠ passa por CorsLayer (senão OPTIONS é sequestrado
    // e response perde headers `DAV:`/`Allow:` exigidos pelo protocolo).
    Router::new()
        .merge(api)
        .merge(crate::caldav::routes())
        .layer(axum::middleware::from_fn(expresso_observability::http_counter_mw))
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .with_state(state)
}

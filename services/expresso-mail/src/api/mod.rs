//! Axum HTTP router for webmail REST API

pub mod health;
pub mod folders;
pub mod messages;
pub mod compose;

use axum::Router;
use tower_http::{
    cors::{CorsLayer, Any},
    trace::TraceLayer,
    compression::CompressionLayer,
};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .nest("/api/v1", api_routes(state.clone()))
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)  // tighten in prod via env
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}

fn api_routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .merge(folders::routes())
        .merge(messages::routes())
        .merge(compose::routes())
}

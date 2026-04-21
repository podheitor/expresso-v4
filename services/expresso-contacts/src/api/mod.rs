//! Axum HTTP router for expresso-contacts

pub mod context;
mod addressbooks;
mod contacts;
mod health;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(addressbooks::routes())
        .merge(contacts::routes())
        .merge(crate::carddav::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

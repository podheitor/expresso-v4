pub mod context;
mod files;
mod health;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(files::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

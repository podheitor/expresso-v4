pub mod context;
mod files;
mod health;
mod shares;
mod wopi;
mod uploads;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .merge(files::routes())
        .merge(shares::routes())
        .merge(wopi::routes())
        .merge(uploads::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

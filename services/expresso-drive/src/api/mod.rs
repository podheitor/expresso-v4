pub mod context;
mod activity;
mod comments;
mod files;
mod health;
mod reactions;
mod shares;
mod wopi;
mod wopi_metrics;
mod uploads;

pub use wopi_metrics::init as init_wopi_metrics;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .merge(activity::routes())
        .merge(comments::routes())
        .merge(reactions::routes())
        .merge(files::routes())
        .merge(shares::routes())
        .merge(wopi::routes())
        .merge(uploads::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

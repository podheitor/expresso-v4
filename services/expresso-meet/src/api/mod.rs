pub mod context;
mod meetings;
mod health;

use std::sync::Arc;

use axum::{Extension, Router};
use expresso_auth_client::OidcValidator;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState, oidc: Option<Arc<OidcValidator>>) -> Router {
    let router = Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .merge(meetings::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state);
    match oidc {
        Some(v) => router.layer(Extension(v)),
        None    => router,
    }
}

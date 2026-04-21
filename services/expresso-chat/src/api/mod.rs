pub mod context;
mod channels;
mod health;
mod messages;

use std::sync::Arc;

use axum::{Extension, Router};
use expresso_auth_client::OidcValidator;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState, oidc: Option<Arc<OidcValidator>>) -> Router {
    let router = Router::new()
        .merge(health::routes())
        .merge(channels::routes())
        .merge(messages::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state);
    match oidc {
        Some(v) => router.layer(Extension(v)),
        None    => router,
    }
}

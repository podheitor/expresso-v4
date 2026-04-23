//! Axum HTTP router for expresso-contacts

pub mod context;
mod addressbooks;
mod contacts;
mod gal;
mod health;
mod wellknown;
mod sharing;
mod users;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .merge(addressbooks::routes())
        .merge(contacts::routes())
        .merge(gal::routes())
        .merge(sharing::routes())
        .merge(users::routes())
        .merge(wellknown::routes())
        .layer(CorsLayer::permissive());

    // CardDAV ≠ passa por CorsLayer (senão OPTIONS perde `DAV:`/`Allow:`).
    Router::new()
        .merge(api)
        .merge(crate::carddav::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .with_state(state)
}

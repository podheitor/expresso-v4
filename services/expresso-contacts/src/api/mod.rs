//! Axum HTTP router for expresso-contacts

mod activity;
mod addressbooks;
mod contacts;
pub mod context;
mod gal;
mod groups;
mod health;
mod internal;
mod search_index;
mod sharing;
mod users;
mod wellknown;

use axum::Router;
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .merge(addressbooks::routes())
        .merge(contacts::routes())
        .merge(activity::routes())
        .merge(internal::routes())
        .merge(gal::routes())
        .merge(groups::routes())
        .merge(sharing::routes())
        .merge(users::routes())
        .merge(wellknown::routes())
        .layer(CorsLayer::permissive());

    // CardDAV ≠ passa por CorsLayer (senão OPTIONS perde `DAV:`/`Allow:`).
    Router::new()
        .merge(api)
        .merge(crate::carddav::routes())
        .layer(axum::middleware::from_fn(
            expresso_observability::http_counter_mw,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .with_state(state)
}

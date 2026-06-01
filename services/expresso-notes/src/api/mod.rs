//! Axum HTTP router for expresso-notes.

pub mod context;
mod health;
mod notebooks;
mod notes;
mod sharing;

use std::sync::Arc;

use axum::{Extension, Router};
use expresso_auth_client::{MultiRealmValidator, OidcValidator, PatResolver, TenantResolver};
use tower_http::{compression::CompressionLayer, cors::CorsLayer, trace::TraceLayer};

use crate::state::AppState;

pub fn router(
    state: AppState,
    oidc: Option<Arc<OidcValidator>>,
    multi: Option<Arc<MultiRealmValidator>>,
    resolver: Option<Arc<TenantResolver>>,
    pat: Option<Arc<PatResolver>>,
) -> Router {
    let mut router = Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .merge(notes::routes())
        .merge(notebooks::routes())
        .merge(sharing::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state);
    if let Some(v) = oidc {
        router = router.layer(Extension(v));
    }
    if let Some(m) = multi {
        router = router.layer(Extension(m));
    }
    if let Some(r) = resolver {
        router = router.layer(Extension(r));
    }
    if let Some(p) = pat {
        router = router.layer(Extension(p));
    }
    router
}

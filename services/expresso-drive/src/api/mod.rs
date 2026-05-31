mod acl;
mod activity;
mod comments;
pub mod context;
mod files;
mod health;
mod reactions;
mod settings;
mod shares;
mod tags;
mod uploads;
mod wopi;
mod wopi_metrics;

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
        .merge(acl::routes())
        .merge(files::routes())
        .merge(settings::routes())
        .merge(shares::routes())
        .merge(wopi::routes())
        .merge(tags::routes())
        .merge(uploads::routes())
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    /// Mounting the full router must not panic. axum rejects overlapping
    /// method routes at construction, so this catches the duplicate-route
    /// regression that previously made the service unbootable (two parallel
    /// tag/version modules registering the same paths).
    #[test]
    fn router_mounts_without_overlap() {
        let state = AppState::new(None, std::path::PathBuf::from("/tmp"));
        let _ = router(state);
    }
}

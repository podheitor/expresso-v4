//! Axum HTTP router for webmail REST API

pub mod aliases;
pub mod attachments;
pub mod compose;
pub mod context;
pub mod drafts;
pub mod flag_presets;
pub mod folders;
pub mod health;
pub mod messages;
pub mod quota;
pub mod sieve;
pub mod signatures;
pub mod snooze;
pub mod threads;
pub mod vacation;

use axum::{
    extract::Request,
    middleware::{self, Next},
    response::Response,
    Router,
};
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::state::AppState;

/// Record HTTP_REQUESTS_TOTAL{service, method, status} for every request.
async fn http_metrics_middleware(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let resp = next.run(req).await;
    let status = resp.status().as_u16().to_string();
    expresso_observability::HTTP_REQUESTS_TOTAL
        .with_label_values(&["expresso-mail", &method, &status])
        .inc();
    resp
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(expresso_observability::metrics_router())
        .nest("/api/v1", api_routes(state.clone()))
        .layer(middleware::from_fn(http_metrics_middleware))
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(
            CorsLayer::new()
                .allow_origin(Any) // tighten in prod via env
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}

fn api_routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .merge(folders::routes())
        .merge(aliases::routes())
        .merge(messages::routes())
        .merge(compose::routes())
        .merge(attachments::routes())
        .merge(quota::routes())
        .merge(vacation::routes())
        .merge(sieve::routes())
        .merge(drafts::routes())
        .merge(flag_presets::routes())
        .merge(snooze::routes())
        .merge(signatures::routes())
        .merge(threads::routes())
}

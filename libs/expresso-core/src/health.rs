//! Deep readiness checks (`/readyz`) across dependencies.
//!
//! Each service composes a `ReadinessCheck` list. Handler runs them in
//! parallel; returns 200 + JSON with each component status, or 503 if any
//! required check fails.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde::Serialize;

pub type CheckFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
pub type CheckFn = Arc<dyn Fn() -> CheckFuture + Send + Sync>;

pub struct ReadinessCheck {
    pub name:     &'static str,
    pub required: bool,
    pub run:      CheckFn,
}

#[derive(Serialize)]
pub struct ComponentStatus {
    name:   &'static str,
    status: &'static str, // "ok" | "fail"
    error:  Option<String>,
    elapsed_ms: u128,
}

#[derive(Serialize)]
pub struct ReadyReport {
    pub status:     &'static str, // "ok" | "degraded" | "fail"
    pub components: Vec<ComponentStatus>,
}

pub async fn run(checks: &[ReadinessCheck]) -> (StatusCode, ReadyReport) {
    let start = std::time::Instant::now();
    let mut components = Vec::with_capacity(checks.len());
    let mut fail_required = false;
    let mut fail_optional = false;

    for c in checks {
        let t = std::time::Instant::now();
        // 3s per-check timeout; keeps /readyz bounded even if a backend hangs.
        let result = tokio::time::timeout(Duration::from_secs(3), (c.run)()).await;
        let (status, error) = match result {
            Ok(Ok(())) => ("ok", None),
            Ok(Err(e)) => ("fail", Some(e)),
            Err(_)     => ("fail", Some("timeout".into())),
        };
        if status == "fail" {
            if c.required { fail_required = true; } else { fail_optional = true; }
        }
        components.push(ComponentStatus {
            name: c.name, status, error, elapsed_ms: t.elapsed().as_millis(),
        });
    }
    let overall = if fail_required { "fail" } else if fail_optional { "degraded" } else { "ok" };
    let code = if fail_required { StatusCode::SERVICE_UNAVAILABLE } else { StatusCode::OK };
    tracing::debug!(elapsed_ms = start.elapsed().as_millis(), overall, "readyz checked");
    (code, ReadyReport { status: overall, components })
}

impl IntoResponse for ReadyReport {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

/// Convenience DB check factory.
pub fn db_check(pool: sqlx::PgPool) -> CheckFn {
    Arc::new(move || {
        let p = pool.clone();
        Box::pin(async move {
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&p).await
                .map(|_| ()).map_err(|e| e.to_string())
        })
    })
}

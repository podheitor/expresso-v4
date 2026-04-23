//! Per-tenant in-process rate limiting.
//!
//! Sliding-window token bucket keyed by tenant UUID. Enforced by axum
//! middleware: extracts tenant from `X-Expresso-Tenant` header (set by
//! upstream auth/gateway) or falls back to client IP when absent.
//!
//! Limits configurable via env:
//!   - `EXPRESSO_RATELIMIT_RPS`   — steady-state refill (default: 50)
//!   - `EXPRESSO_RATELIMIT_BURST` — bucket size (default: 200)
//!
//! When exceeded: returns 429 + `Retry-After` header (seconds).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::Request,
    http::{HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};

#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub rps:   u32,   // tokens/sec
    pub burst: u32,   // bucket capacity
}

impl RateLimitConfig {
    pub fn from_env() -> Self {
        fn parse(k: &str, default: u32) -> u32 {
            std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        Self {
            rps:   parse("EXPRESSO_RATELIMIT_RPS", 50),
            burst: parse("EXPRESSO_RATELIMIT_BURST", 200),
        }
    }
}

#[derive(Debug)]
struct Bucket {
    tokens:    f64,
    last_fill: Instant,
}

#[derive(Debug)]
pub struct RateLimiter {
    cfg:     RateLimitConfig,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub fn new(cfg: RateLimitConfig) -> Arc<Self> {
        Arc::new(Self { cfg, buckets: Mutex::new(HashMap::new()) })
    }

    /// Returns Ok(()) if allowed; Err(retry_after_secs) if denied.
    pub fn check(&self, key: &str) -> Result<(), u64> {
        let now = Instant::now();
        let mut map = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        let b = map.entry(key.to_string()).or_insert(Bucket {
            tokens:    self.cfg.burst as f64,
            last_fill: now,
        });
        let elapsed = now.duration_since(b.last_fill).as_secs_f64();
        let refill  = elapsed * self.cfg.rps as f64;
        b.tokens    = (b.tokens + refill).min(self.cfg.burst as f64);
        b.last_fill = now;

        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            // Seconds until one token refills.
            let need = 1.0 - b.tokens;
            let secs = (need / self.cfg.rps as f64).ceil().max(1.0) as u64;
            Err(secs)
        }
    }

    /// GC old buckets (>10 min idle). Call periodically to bound memory.
    pub fn gc(&self) {
        let cutoff = Instant::now() - Duration::from_secs(600);
        let mut map = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        map.retain(|_, b| b.last_fill > cutoff);
    }
}

/// Axum middleware. Expects the Limiter in request extensions.
pub async fn layer(
    req: Request,
    next: Next,
) -> Response {
    // Skip rate limiting on observability/health endpoints.
    let path = req.uri().path();
    if matches!(path, "/health" | "/healthz" | "/readyz" | "/ready" | "/metrics") {
        return next.run(req).await;
    }
    let limiter = req.extensions().get::<Arc<RateLimiter>>().cloned();
    if let Some(limiter) = limiter {
        let key = req.headers().get("x-expresso-tenant")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| req.headers().get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(|s| s.trim().to_string()))
            .unwrap_or_else(|| "_anon".to_string());

        if let Err(retry) = limiter.check(&key) {
            let mut resp = Response::new(axum::body::Body::from(
                format!(r#"{{"error":"rate_limited","retry_after":{retry}}}"#),
            ));
            *resp.status_mut() = StatusCode::TOO_MANY_REQUESTS;
            resp.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_str(&retry.to_string()).unwrap_or_else(|_| HeaderValue::from_static("1")),
            );
            return resp;
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burst_then_denied() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 3 });
        assert!(rl.check("k").is_ok());
        assert!(rl.check("k").is_ok());
        assert!(rl.check("k").is_ok());
        assert!(rl.check("k").is_err());
    }

    #[test]
    fn refill_over_time() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 100, burst: 1 });
        assert!(rl.check("k").is_ok());
        assert!(rl.check("k").is_err());
        std::thread::sleep(Duration::from_millis(20));
        assert!(rl.check("k").is_ok());
    }

    #[test]
    fn per_key_isolated() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 1 });
        assert!(rl.check("a").is_ok());
        assert!(rl.check("b").is_ok());
        assert!(rl.check("a").is_err());
    }
}

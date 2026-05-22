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

    #[test]
    fn gc_does_not_panic_and_preserves_active_bucket() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 100, burst: 5 });
        rl.check("tenant-active").unwrap();
        // gc() prunes buckets idle for >10 min; this bucket was just touched,
        // so it survives. The key assertion: gc() is idempotent and doesn't
        // corrupt the bucket state.
        rl.gc();
        assert!(rl.check("tenant-active").is_ok());
    }

    #[test]
    fn retry_after_is_positive() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 1 });
        rl.check("t").unwrap();
        let retry = rl.check("t").unwrap_err();
        assert!(retry >= 1, "retry_after must be ≥ 1 s, got {retry}");
    }

    #[test]
    fn burst_allows_exactly_burst_requests() {
        let burst = 5u32;
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst });
        for i in 0..burst {
            assert!(rl.check("k").is_ok(), "request {i} should pass");
        }
        assert!(rl.check("k").is_err(), "request burst+1 must be denied");
    }

    #[test]
    fn new_key_starts_with_full_bucket() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 3 });
        // First check on a new key must always succeed (starts at burst capacity).
        assert!(rl.check("brand-new-key").is_ok());
    }

    #[test]
    fn gc_removes_idle_buckets_simulated() {
        // We can't wait 10 min in a unit test, but we can verify gc() runs
        // without panicking on a fresh limiter with some active buckets.
        let rl = RateLimiter::new(RateLimitConfig { rps: 10, burst: 5 });
        rl.check("tenant-a").unwrap();
        rl.check("tenant-b").unwrap();
        // gc() only removes buckets idle > 10 min; these were just touched,
        // so they must survive. The assertion: gc() is safe + idempotent.
        rl.gc();
        rl.gc(); // idempotent
        assert!(rl.check("tenant-a").is_ok());
    }

    #[test]
    fn multiple_independent_keys_all_start_at_burst() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 2 });
        for key in ["alpha", "beta", "gamma"] {
            assert!(rl.check(key).is_ok(), "{key} first check must pass");
            assert!(rl.check(key).is_ok(), "{key} second check must pass");
            assert!(rl.check(key).is_err(), "{key} burst+1 must be denied");
        }
    }

    #[test]
    fn burst_one_allows_only_one_request() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 1 });
        assert!(rl.check("u").is_ok());
        assert!(rl.check("u").is_err());
    }

    #[test]
    fn different_keys_are_independent() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 1 });
        assert!(rl.check("key_a").is_ok());
        assert!(rl.check("key_b").is_ok());
    }

    #[test]
    fn rate_limit_config_rps_preserved() {
        let cfg = RateLimitConfig { rps: 5, burst: 10 };
        assert_eq!(cfg.rps, 5);
        assert_eq!(cfg.burst, 10);
    }

    #[test]
    fn rate_limit_config_burst_preserved() {
        let cfg = RateLimitConfig { rps: 10, burst: 50 };
        assert_eq!(cfg.burst, 50);
    }

    #[test]
    fn rate_limit_config_rps_is_five() {
        let cfg = RateLimitConfig { rps: 5, burst: 10 };
        assert_eq!(cfg.rps, 5);
    }

    #[test]
    fn rate_limit_config_burst_ten_preserved() {
        let cfg = RateLimitConfig { rps: 1, burst: 10 };
        assert_eq!(cfg.burst, 10);
    }

    #[test]
    fn rate_limit_config_zero_rps_is_valid() {
        let cfg = RateLimitConfig { rps: 0, burst: 0 };
        assert_eq!(cfg.rps, 0);
        assert_eq!(cfg.burst, 0);
    }

    #[test]
    fn rate_limit_config_burst_greater_than_rps_is_valid() {
        let cfg = RateLimitConfig { rps: 10, burst: 100 };
        assert!(cfg.burst > cfg.rps);
    }

    #[test]
    fn check_returns_err_with_retry_at_least_one_on_exhausted_bucket() {
        let rl = RateLimiter::new(RateLimitConfig { rps: 1, burst: 2 });
        rl.check("t").unwrap();
        rl.check("t").unwrap();
        let retry = rl.check("t").unwrap_err();
        assert!(retry >= 1);
    }
}

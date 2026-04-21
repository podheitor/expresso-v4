//! Simple per-IP sliding-window rate limiter — protects /auth/login
//! from credential-stuffing / DoS.
//!
//! Memory: HashMap<IpAddr, VecDeque<Instant>> pruned on every check.
//! ≠ distributed (single-instance). For multi-instance deploy behind a
//! reverse proxy or use Redis-backed limiter.

use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, State},
    http::{header::HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug)]
pub struct RateLimiter {
    window:     Duration,
    max:        usize,
    buckets:    Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(window: Duration, max: usize) -> Self {
        Self { window, max, buckets: Mutex::new(HashMap::new()) }
    }

    /// Returns true if request under limit, false if exceeded.
    pub fn check(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.buckets.lock().unwrap();
        let q = map.entry(ip).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) > self.window) {
            q.pop_front();
        }
        if q.len() >= self.max { return false; }
        q.push_back(now);
        true
    }
}

/// Axum middleware — rejects 429 when per-IP rate exceeded.
/// Wire via `axum::middleware::from_fn_with_state(limiter, rate_limit_mw)`.
pub async fn rate_limit_mw(
    State(limiter): State<std::sync::Arc<RateLimiter>>,
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    req: axum::extract::Request,
    next: Next,
) -> Response
{
    // Prefer X-Forwarded-For (first hop) when behind a proxy.
    let ip = headers.get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
        .unwrap_or(addr.ip());

    if limiter.check(ip) {
        next.run(req).await
    } else {
        tracing::warn!(
            target: "audit",
            event = "auth.login.rate_limited",
            ip = %ip,
            "rate limit exceeded"
        );
        (StatusCode::TOO_MANY_REQUESTS,
         Json(json!({"error":"rate_limited","message":"too many requests"})))
         .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn allows_up_to_max_then_blocks() {
        let rl = RateLimiter::new(Duration::from_secs(60), 3);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(rl.check(ip));
        assert!(rl.check(ip));
        assert!(rl.check(ip));
        assert!(!rl.check(ip));
    }

    #[test]
    fn different_ips_independent_buckets() {
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        let a = IpAddr::V4(Ipv4Addr::new(10,0,0,1));
        let b = IpAddr::V4(Ipv4Addr::new(10,0,0,2));
        assert!(rl.check(a));
        assert!(!rl.check(a));
        assert!(rl.check(b)); // b unaffected
    }

    #[test]
    fn window_eviction_allows_new_requests() {
        let rl = RateLimiter::new(Duration::from_millis(50), 1);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(rl.check(ip));
        assert!(!rl.check(ip));
        std::thread::sleep(Duration::from_millis(80));
        assert!(rl.check(ip));
    }
}

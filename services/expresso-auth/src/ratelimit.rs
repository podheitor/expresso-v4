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
    window:          Duration,
    max:             usize,
    /// Quando true, o middleware confia no primeiro hop de
    /// `X-Forwarded-For` ao decidir a chave do bucket. Caso contrário usa
    /// ConnectInfo (peer addr). Ligar APENAS quando o serviço estiver
    /// atrás de um proxy reverso que sobrescreve o header — caso contrário
    /// um atacante incrementa o XFF a cada request e ganha um bucket novo,
    /// anulando o limite.
    trust_forwarded: bool,
    buckets:         Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(window: Duration, max: usize) -> Self {
        Self::with_trust_proxy(window, max, false)
    }

    pub fn with_trust_proxy(window: Duration, max: usize, trust_forwarded: bool) -> Self {
        Self { window, max, trust_forwarded, buckets: Mutex::new(HashMap::new()) }
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
    // XFF só conta quando o operador opt-in via `trust_forwarded` —
    // sem isso, o atacante poderia gerar IPs falsos no header e fugir
    // do limite. Caminho default: peer addr da conexão.
    let ip = if limiter.trust_forwarded {
        headers.get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
            .unwrap_or(addr.ip())
    } else {
        addr.ip()
    };

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

    #[test]
    fn trust_proxy_flag_default_off() {
        // Garantir que o construtor padrão NÃO confia no XFF — operador
        // tem que opt-in explicitamente. Defesa contra deploy direto na
        // internet sem proxy reverso.
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        assert!(!rl.trust_forwarded);
        let rl2 = RateLimiter::with_trust_proxy(Duration::from_secs(60), 1, true);
        assert!(rl2.trust_forwarded);
    }

    #[test]
    fn window_boundary_exact() {
        // Requests at exactly max should still block; one above limit should fail.
        let rl = RateLimiter::new(Duration::from_secs(60), 5);
        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        for _ in 0..5 {
            assert!(rl.check(ip), "request within limit should pass");
        }
        assert!(!rl.check(ip), "request at limit+1 should be blocked");
    }

    #[test]
    fn ipv6_tracked_separately_from_ipv4() {
        use std::net::Ipv6Addr;
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        let v4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert!(rl.check(v4));
        assert!(!rl.check(v4));
        // IPv6 loopback is a different key — must not be affected by IPv4 exhaustion.
        assert!(rl.check(v6));
    }

    #[test]
    fn high_request_volume_does_not_panic() {
        let rl = RateLimiter::new(Duration::from_secs(60), 10);
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        for _ in 0..100 {
            let _ = rl.check(ip);
        }
    }

    #[test]
    fn two_distinct_ips_tracked_independently() {
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        let a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert!(rl.check(a));
        assert!(!rl.check(a));
        assert!(rl.check(b));
    }

    #[test]
    fn limit_zero_blocks_immediately() {
        let rl = RateLimiter::new(Duration::from_secs(60), 0);
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1));
        assert!(!rl.check(ip));
    }

    #[test]
    fn limit_one_allows_first_denies_second() {
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        assert!(rl.check(ip));
        assert!(!rl.check(ip));
    }

    #[test]
    fn different_ips_are_independent() {
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        let ip_a = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip_b = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert!(rl.check(ip_a));
        assert!(rl.check(ip_b));
    }

    #[test]
    fn limit_large_allows_many_requests() {
        let rl = RateLimiter::new(Duration::from_secs(60), 50);
        let ip = IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1));
        for _ in 0..50 {
            assert!(rl.check(ip), "should allow requests within limit");
        }
        assert!(!rl.check(ip), "51st request should be blocked");
    }

    #[test]
    fn limit_two_separate_ips_independent_limits() {
        let rl = RateLimiter::new(Duration::from_secs(60), 2);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 10));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 11));
        assert!(rl.check(ip1));
        assert!(rl.check(ip1));
        assert!(!rl.check(ip1));
        assert!(rl.check(ip2));
    }

    #[test]
    fn limit_one_allows_exactly_one_request() {
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1));
        assert!(rl.check(ip));
        assert!(!rl.check(ip));
    }

    #[test]
    fn two_different_ips_are_independent() {
        let rl = RateLimiter::new(Duration::from_secs(60), 1);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));
        assert!(rl.check(ip1));
        assert!(rl.check(ip2));
    }

    #[test]
    fn with_trust_proxy_true_sets_flag() {
        let rl = RateLimiter::with_trust_proxy(Duration::from_secs(60), 5, true);
        assert!(rl.trust_forwarded);
    }

    #[test]
    fn with_trust_proxy_false_keeps_flag_off() {
        let rl = RateLimiter::with_trust_proxy(Duration::from_secs(60), 10, false);
        assert!(!rl.trust_forwarded);
    }
}

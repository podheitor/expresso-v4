//! Per-username brute-force throttle pra `cmd_login`.
//!
//! IMAP LOGIN bate direto no DB (legacy `users.password_hash` via pgcrypto
//! `crypt()`) — não passa pelo Keycloak, então o `KcBasicAuthenticator`
//! do sprint #105 não cobre esse caminho. Sem freio, atacante com
//! username conhecido manda LOGIN num loop apertado e cada tentativa
//! custa um bcrypt no Postgres.
//!
//! Lockout per-username (lowercased) — não inclui senha na chave, senão
//! atacante rotacionando senha bypassa o counter.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct FailureTracker {
    window_start: Instant,
    failures:     u32,
    locked_until: Option<Instant>,
}

#[derive(Debug)]
pub struct LoginLockout {
    /// Falhas consecutivas (na janela `failure_window`) antes do
    /// lockout disparar. Default 10 — alto pra usuários reais não
    /// caírem por typo, baixo pra brute-force.
    max_failures:     u32,
    /// Janela de contagem das falhas. Default 60s.
    failure_window:   Duration,
    /// Duração do lockout depois de atingir `max_failures`. Default 5min.
    lockout_duration: Duration,
    failures:         Mutex<HashMap<String, FailureTracker>>,
}

impl Default for LoginLockout {
    fn default() -> Self {
        Self::new(10, Duration::from_secs(60), Duration::from_secs(5 * 60))
    }
}

impl LoginLockout {
    pub fn new(max_failures: u32, failure_window: Duration, lockout_duration: Duration) -> Self {
        Self {
            max_failures, failure_window, lockout_duration,
            failures: Mutex::new(HashMap::new()),
        }
    }

    pub fn is_locked_out(&self, user: &str) -> bool {
        let key = user.to_ascii_lowercase();
        let Ok(guard) = self.failures.lock() else { return false; };
        let now = Instant::now();
        guard.get(&key)
            .and_then(|t| t.locked_until)
            .is_some_and(|until| until > now)
    }

    pub fn record_failure(&self, user: &str) {
        let key = user.to_ascii_lowercase();
        let Ok(mut guard) = self.failures.lock() else { return; };
        let now = Instant::now();
        let entry = guard.entry(key).or_insert(FailureTracker {
            window_start: now,
            failures:     0,
            locked_until: None,
        });
        if now.duration_since(entry.window_start) > self.failure_window {
            entry.window_start = now;
            entry.failures     = 0;
            entry.locked_until = None;
        }
        entry.failures += 1;
        if entry.failures >= self.max_failures {
            entry.locked_until = Some(now + self.lockout_duration);
        }
    }

    pub fn clear_failures(&self, user: &str) {
        let key = user.to_ascii_lowercase();
        if let Ok(mut guard) = self.failures.lock() {
            guard.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockout_triggers_after_max_failures() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        assert!(!l.is_locked_out("alice"));
        l.record_failure("alice");
        l.record_failure("alice");
        assert!(!l.is_locked_out("alice"));
        l.record_failure("alice"); // hits max
        assert!(l.is_locked_out("alice"));
        // Bob unaffected — lockout é per-username.
        assert!(!l.is_locked_out("bob"));
    }

    #[test]
    fn lockout_key_case_insensitive() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("Alice@Example.Com");
        l.record_failure("alice@example.com");
        // Mesmo bucket → 2 falhas.
        assert!(l.is_locked_out("ALICE@EXAMPLE.COM"));
    }

    #[test]
    fn lockout_expires_after_duration() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_millis(50));
        l.record_failure("alice");
        l.record_failure("alice");
        assert!(l.is_locked_out("alice"));
        std::thread::sleep(Duration::from_millis(80));
        assert!(!l.is_locked_out("alice"));
    }

    #[test]
    fn success_clears_failures() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("alice");
        l.record_failure("alice");
        l.clear_failures("alice");
        l.record_failure("alice"); // counter reseta — só conta como 1
        assert!(!l.is_locked_out("alice"));
    }

    #[test]
    fn window_expiry_resets_counter() {
        let l = LoginLockout::new(3, Duration::from_millis(40), Duration::from_secs(60));
        l.record_failure("alice");
        l.record_failure("alice");
        std::thread::sleep(Duration::from_millis(60));
        // Janela expirou — próxima falha começa um counter novo.
        l.record_failure("alice");
        l.record_failure("alice");
        assert!(!l.is_locked_out("alice"));
    }

    #[test]
    fn clear_failures_noop_on_unknown_user() {
        let l = LoginLockout::default();
        // Should not panic even if user never had failures.
        l.clear_failures("nobody@example.com");
        assert!(!l.is_locked_out("nobody@example.com"));
    }

    #[test]
    fn not_locked_before_any_failures() {
        let l = LoginLockout::default();
        assert!(!l.is_locked_out("brand-new-user@example.com"));
    }

    #[test]
    fn lockout_per_user_isolation() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("alice@example.com");
        l.record_failure("alice@example.com");
        assert!(l.is_locked_out("alice@example.com"));
        // bob is unaffected
        assert!(!l.is_locked_out("bob@example.com"));
    }

    #[test]
    fn one_failure_below_threshold_not_locked() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("user@example.com");
        assert!(!l.is_locked_out("user@example.com"));
    }

    #[test]
    fn threshold_failures_triggers_lockout() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("x@y.com");
        l.record_failure("x@y.com");
        l.record_failure("x@y.com");
        assert!(l.is_locked_out("x@y.com"));
    }

    #[test]
    fn fresh_lockout_is_not_locked() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        assert!(!l.is_locked_out("new@user.com"));
    }

    #[test]
    fn two_failures_below_threshold_not_locked() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("u@example.com");
        l.record_failure("u@example.com");
        assert!(!l.is_locked_out("u@example.com"));
    }

    #[test]
    fn different_users_are_independent() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("a@x.com");
        l.record_failure("a@x.com");
        assert!(l.is_locked_out("a@x.com"));
        assert!(!l.is_locked_out("b@x.com"));
    }

    #[test]
    fn no_failures_not_locked_out() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        assert!(!l.is_locked_out("new@x.com"));
    }

    #[test]
    fn exactly_threshold_failures_triggers_lockout() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("c@x.com");
        l.record_failure("c@x.com");
        assert!(l.is_locked_out("c@x.com"));
    }

    #[test]
    fn default_lockout_max_failures_is_ten() {
        let l = LoginLockout::default();
        // Default threshold is 10 — nine failures must not lock out.
        for _ in 0..9 {
            l.record_failure("d@x.com");
        }
        assert!(!l.is_locked_out("d@x.com"));
    }

    #[test]
    fn clear_failures_allows_immediate_login_attempt() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("e@x.com");
        l.record_failure("e@x.com");
        assert!(l.is_locked_out("e@x.com"));
        l.clear_failures("e@x.com");
        assert!(!l.is_locked_out("e@x.com"));
    }

    #[test]
    fn record_failure_below_threshold_twice_stays_unlocked() {
        let l = LoginLockout::new(5, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("user@x.com");
        l.record_failure("user@x.com");
        assert!(!l.is_locked_out("user@x.com"));
    }

    #[test]
    fn fresh_user_is_not_locked_out() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        assert!(!l.is_locked_out("brand_new@x.com"));
    }

    #[test]
    fn record_failure_three_times_at_threshold_three_triggers_lockout() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("u@x.com");
        l.record_failure("u@x.com");
        l.record_failure("u@x.com");
        assert!(l.is_locked_out("u@x.com"));
    }

    #[test]
    fn lockout_two_users_independent_lockout() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("alice@x.com");
        l.record_failure("alice@x.com");
        assert!(l.is_locked_out("alice@x.com"));
        assert!(!l.is_locked_out("bob@x.com"));
    }

    #[test]
    fn clear_failures_after_lockout_then_single_failure_not_locked() {
        let l = LoginLockout::new(2, Duration::from_secs(60), Duration::from_secs(60));
        l.record_failure("z@x.com");
        l.record_failure("z@x.com");
        assert!(l.is_locked_out("z@x.com"));
        l.clear_failures("z@x.com");
        l.record_failure("z@x.com");
        assert!(!l.is_locked_out("z@x.com"));
    }

    #[test]
    fn user_not_locked_after_clear_failures_with_no_prior_failures() {
        let l = LoginLockout::new(3, Duration::from_secs(60), Duration::from_secs(60));
        l.clear_failures("new@x.com");
        assert!(!l.is_locked_out("new@x.com"));
    }
}

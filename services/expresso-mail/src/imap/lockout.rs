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
}

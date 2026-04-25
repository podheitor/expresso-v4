//! Prometheus metrics for the IMAP server.
//!
//! Three counters expose enough surface to alert on outages and brute-force
//! attempts without inflating cardinality:
//! - `mail_imap_commands_total{command, outcome}` — per-command outcomes
//!   bucketed as ok / no / bad / other.
//! - `mail_imap_sessions_total{result}` — accepted, closed, error.
//! - `mail_imap_logins_total{outcome}` — success / failure.

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

pub static IMAP_COMMANDS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_imap_commands_total",
            "IMAP command counts per name and outcome",
        ),
        &["command", "outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

pub static IMAP_SESSIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_imap_sessions_total",
            "IMAP TCP session lifecycle outcomes",
        ),
        &["result"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

pub static IMAP_LOGINS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_imap_logins_total",
            "IMAP LOGIN attempts per outcome",
        ),
        &["outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

/// Pre-populate label series so Prometheus `rate()` / `increase()` work
/// from the first scrape, even before any client connects. Idempotent.
pub fn init() {
    Lazy::force(&IMAP_COMMANDS_TOTAL);
    Lazy::force(&IMAP_SESSIONS_TOTAL);
    Lazy::force(&IMAP_LOGINS_TOTAL);

    for cmd in [
        "CAPABILITY", "LOGIN", "LIST", "SELECT", "EXAMINE", "FETCH",
        "STORE", "EXPUNGE", "CLOSE", "LOGOUT", "NOOP", "IDLE", "STATUS",
        "APPEND", "COPY", "OTHER",
    ] {
        for outcome in ["ok", "no", "bad"] {
            IMAP_COMMANDS_TOTAL.with_label_values(&[cmd, outcome]).inc_by(0);
        }
    }
    for r in ["accepted", "closed", "error", "parse_error"] {
        IMAP_SESSIONS_TOTAL.with_label_values(&[r]).inc_by(0);
    }
    for o in ["success", "failure"] {
        IMAP_LOGINS_TOTAL.with_label_values(&[o]).inc_by(0);
    }
}

/// Bucket arbitrary command names into the fixed label set kept by
/// `init` — anything outside the canonical list collapses to "OTHER" so
/// cardinality stays bounded under malformed traffic.
pub fn command_label(name: &str) -> &'static str {
    match name.to_ascii_uppercase().as_str() {
        "CAPABILITY" => "CAPABILITY",
        "LOGIN"      => "LOGIN",
        "LIST"       => "LIST",
        "SELECT"     => "SELECT",
        "EXAMINE"    => "EXAMINE",
        "FETCH"      => "FETCH",
        "STORE"      => "STORE",
        "EXPUNGE"    => "EXPUNGE",
        "CLOSE"      => "CLOSE",
        "LOGOUT"     => "LOGOUT",
        "NOOP"       => "NOOP",
        "IDLE"       => "IDLE",
        "STATUS"     => "STATUS",
        "APPEND"     => "APPEND",
        "COPY"       => "COPY",
        _            => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_commands_map_directly() {
        assert_eq!(command_label("LOGIN"),  "LOGIN");
        assert_eq!(command_label("login"),  "LOGIN");
        assert_eq!(command_label("Fetch"),  "FETCH");
        assert_eq!(command_label("STATUS"), "STATUS");
        assert_eq!(command_label("IDLE"),   "IDLE");
        assert_eq!(command_label("APPEND"), "APPEND");
        assert_eq!(command_label("COPY"),   "COPY");
    }

    #[test]
    fn unknown_collapses_to_other() {
        assert_eq!(command_label("APPEND"),   "OTHER");
        assert_eq!(command_label("garbage"),  "OTHER");
        assert_eq!(command_label(""),         "OTHER");
    }
}

//! Prometheus metrics for the POP3 server (RFC 1939).
//!
//! Mirrors the IMAP metric surface so dashboards can alert on POP3 the same
//! way: per-command outcomes, session lifecycle, and login attempts.
//! - `mail_pop3_commands_total{command, outcome}` — ok / err per command.
//! - `mail_pop3_sessions_total{result}` — accepted / closed / error.
//! - `mail_pop3_logins_total{outcome}` — success / failure / locked_out.

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

pub static POP3_COMMANDS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_pop3_commands_total",
            "POP3 command counts per name and outcome",
        ),
        &["command", "outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

pub static POP3_SESSIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_pop3_sessions_total",
            "POP3 TCP session lifecycle outcomes",
        ),
        &["result"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

pub static POP3_LOGINS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new("mail_pop3_logins_total", "POP3 login attempts per outcome"),
        &["outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

/// Pre-populate label series so Prometheus `rate()` / `increase()` work
/// from the first scrape, even before any client connects. Idempotent.
pub fn init() {
    Lazy::force(&POP3_COMMANDS_TOTAL);
    Lazy::force(&POP3_SESSIONS_TOTAL);
    Lazy::force(&POP3_LOGINS_TOTAL);

    for cmd in [
        "USER", "PASS", "STAT", "LIST", "UIDL", "RETR", "DELE", "TOP", "NOOP", "RSET", "QUIT",
        "CAPA",
    ] {
        for outcome in ["ok", "err"] {
            POP3_COMMANDS_TOTAL
                .with_label_values(&[cmd, outcome])
                .inc_by(0);
        }
    }
    for result in ["accepted", "closed", "error"] {
        POP3_SESSIONS_TOTAL.with_label_values(&[result]).inc_by(0);
    }
    for outcome in ["success", "failure", "locked_out"] {
        POP3_LOGINS_TOTAL.with_label_values(&[outcome]).inc_by(0);
    }
}

//! Prometheus metrics for the SMTP/LMTP servers.
//!
//! - `mail_smtp_commands_total{command,listener,outcome}` — per-command outcomes
//!   bucketed as ok / reject / error / other.
//! - `mail_smtp_sessions_total{listener,result}` — accepted / closed / error.

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

pub static SMTP_COMMANDS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_smtp_commands_total",
            "SMTP/LMTP command counts per name, listener and outcome",
        ),
        &["command", "listener", "outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

pub static SMTP_SESSIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_smtp_sessions_total",
            "SMTP/LMTP TCP session lifecycle outcomes",
        ),
        &["listener", "result"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

/// Pre-populate label series so Prometheus `rate()` works from first scrape.
pub fn init() {
    Lazy::force(&SMTP_COMMANDS_TOTAL);
    Lazy::force(&SMTP_SESSIONS_TOTAL);

    for listener in ["smtp25", "smtp587", "smtps465", "lmtp"] {
        for cmd in [
            "EHLO", "HELO", "LHLO", "MAIL", "RCPT", "DATA",
            "RSET", "NOOP", "QUIT", "VRFY", "STARTTLS", "AUTH", "OTHER",
        ] {
            for outcome in ["ok", "reject", "error"] {
                SMTP_COMMANDS_TOTAL.with_label_values(&[cmd, listener, outcome]).inc_by(0);
            }
        }
        for result in ["accepted", "closed", "error"] {
            SMTP_SESSIONS_TOTAL.with_label_values(&[listener, result]).inc_by(0);
        }
    }
}

/// Normalise raw command strings into the bounded label set.
pub fn command_label(cmd: &str) -> &'static str {
    match cmd.to_ascii_uppercase().as_str() {
        "EHLO"     => "EHLO",
        "HELO"     => "HELO",
        "LHLO"     => "LHLO",
        "MAIL"     => "MAIL",
        "RCPT"     => "RCPT",
        "DATA"     => "DATA",
        "RSET"     => "RSET",
        "NOOP"     => "NOOP",
        "QUIT"     => "QUIT",
        "VRFY"     => "VRFY",
        "STARTTLS" => "STARTTLS",
        "AUTH"     => "AUTH",
        _          => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_commands_map() {
        assert_eq!(command_label("EHLO"), "EHLO");
        assert_eq!(command_label("ehlo"), "EHLO");
        assert_eq!(command_label("MAIL FROM:"), "OTHER"); // full line, not stripped
        assert_eq!(command_label("AUTH PLAIN"), "OTHER"); // full line
        assert_eq!(command_label("XFOO"), "OTHER");
    }

    #[test]
    fn all_known_commands_map() {
        for cmd in ["HELO", "LHLO", "MAIL", "RCPT", "DATA", "RSET", "NOOP", "QUIT", "VRFY", "STARTTLS", "AUTH"] {
            assert_ne!(command_label(cmd), "OTHER", "expected {cmd} to map to itself");
        }
    }

    #[test]
    fn command_label_case_insensitive() {
        assert_eq!(command_label("quit"), "QUIT");
        assert_eq!(command_label("Starttls"), "STARTTLS");
        assert_eq!(command_label("noop"), "NOOP");
    }

    #[test]
    fn empty_and_whitespace_map_to_other() {
        assert_eq!(command_label(""), "OTHER");
        assert_eq!(command_label("   "), "OTHER");
    }

    #[test]
    fn lhlo_maps_directly() {
        assert_eq!(command_label("LHLO"), "LHLO");
        assert_eq!(command_label("lhlo"), "LHLO");
    }

    #[test]
    fn starttls_and_auth_map() {
        assert_eq!(command_label("STARTTLS"), "STARTTLS");
        assert_eq!(command_label("AUTH"), "AUTH");
    }
}

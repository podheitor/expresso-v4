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
        prometheus::Opts::new("mail_imap_logins_total", "IMAP LOGIN attempts per outcome"),
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
        "CAPABILITY",
        "LOGIN",
        "AUTHENTICATE",
        "LIST",
        "SELECT",
        "EXAMINE",
        "FETCH",
        "STORE",
        "EXPUNGE",
        "CLOSE",
        "LOGOUT",
        "NOOP",
        "IDLE",
        "STATUS",
        "APPEND",
        "COPY",
        "MOVE",
        "SEARCH",
        "SUBSCRIBE",
        "UNSUBSCRIBE",
        "LSUB",
        "CREATE",
        "DELETE",
        "RENAME",
        "UNSELECT",
        "CHECK",
        "ENABLE",
        "SORT",
        "THREAD",
        "OTHER",
    ] {
        for outcome in ["ok", "no", "bad"] {
            IMAP_COMMANDS_TOTAL
                .with_label_values(&[cmd, outcome])
                .inc_by(0);
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
        "LOGIN" => "LOGIN",
        "AUTHENTICATE" => "AUTHENTICATE",
        "LIST" => "LIST",
        "SELECT" => "SELECT",
        "EXAMINE" => "EXAMINE",
        "FETCH" => "FETCH",
        "STORE" => "STORE",
        "EXPUNGE" => "EXPUNGE",
        "CLOSE" => "CLOSE",
        "LOGOUT" => "LOGOUT",
        "NOOP" => "NOOP",
        "IDLE" => "IDLE",
        "STATUS" => "STATUS",
        "APPEND" => "APPEND",
        "COPY" => "COPY",
        "MOVE" => "MOVE",
        "SEARCH" => "SEARCH",
        "SUBSCRIBE" => "SUBSCRIBE",
        "UNSUBSCRIBE" => "UNSUBSCRIBE",
        "LSUB" => "LSUB",
        "CREATE" => "CREATE",
        "DELETE" => "DELETE",
        "RENAME" => "RENAME",
        "UNSELECT" => "UNSELECT",
        "CHECK" => "CHECK",
        "ENABLE" => "ENABLE",
        "SORT" => "SORT",
        "THREAD" => "THREAD",
        _ => "OTHER",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_commands_map_directly() {
        assert_eq!(command_label("LOGIN"), "LOGIN");
        assert_eq!(command_label("login"), "LOGIN");
        assert_eq!(command_label("AUTHENTICATE"), "AUTHENTICATE");
        assert_eq!(command_label("Fetch"), "FETCH");
        assert_eq!(command_label("STATUS"), "STATUS");
        assert_eq!(command_label("IDLE"), "IDLE");
        assert_eq!(command_label("APPEND"), "APPEND");
        assert_eq!(command_label("COPY"), "COPY");
        assert_eq!(command_label("SEARCH"), "SEARCH");
        assert_eq!(command_label("SUBSCRIBE"), "SUBSCRIBE");
        assert_eq!(command_label("UNSUBSCRIBE"), "UNSUBSCRIBE");
        assert_eq!(command_label("LSUB"), "LSUB");
        assert_eq!(command_label("CREATE"), "CREATE");
        assert_eq!(command_label("DELETE"), "DELETE");
        assert_eq!(command_label("RENAME"), "RENAME");
        assert_eq!(command_label("UNSELECT"), "UNSELECT");
        assert_eq!(command_label("MOVE"), "MOVE");
    }

    #[test]
    fn unknown_collapses_to_other() {
        assert_eq!(command_label("garbage"), "OTHER");
        assert_eq!(command_label("XFOO"), "OTHER");
        assert_eq!(command_label(""), "OTHER");
    }

    #[test]
    fn sort_thread_check_enable_map() {
        assert_eq!(command_label("SORT"), "SORT");
        assert_eq!(command_label("THREAD"), "THREAD");
        assert_eq!(command_label("CHECK"), "CHECK");
        assert_eq!(command_label("ENABLE"), "ENABLE");
    }

    #[test]
    fn noop_logout_close_map() {
        assert_eq!(command_label("NOOP"), "NOOP");
        assert_eq!(command_label("LOGOUT"), "LOGOUT");
        assert_eq!(command_label("CLOSE"), "CLOSE");
    }

    #[test]
    fn mixed_case_normalised() {
        assert_eq!(command_label("FeTcH"), "FETCH");
        assert_eq!(command_label("sElEcT"), "SELECT");
    }

    #[test]
    fn store_expunge_uid_map() {
        assert_eq!(command_label("STORE"), "STORE");
        assert_eq!(command_label("EXPUNGE"), "EXPUNGE");
        assert_eq!(command_label("UID"), "OTHER");
    }

    #[test]
    fn fetch_and_search_map() {
        assert_eq!(command_label("FETCH"), "FETCH");
        assert_eq!(command_label("SEARCH"), "SEARCH");
    }

    #[test]
    fn numeric_command_maps_to_other() {
        assert_eq!(command_label("A001"), "OTHER");
    }

    #[test]
    fn fetch_command_maps_correctly() {
        assert_eq!(command_label("FETCH"), "FETCH");
        assert_eq!(command_label("fetch"), "FETCH");
    }

    #[test]
    fn authenticate_command_maps_correctly() {
        assert_eq!(command_label("AUTHENTICATE"), "AUTHENTICATE");
    }

    #[test]
    fn logout_command_maps_correctly() {
        assert_eq!(command_label("LOGOUT"), "LOGOUT");
        assert_eq!(command_label("logout"), "LOGOUT");
    }

    #[test]
    fn fetch_command_case_insensitive_maps_correctly() {
        assert_eq!(command_label("FETCH"), "FETCH");
        assert_eq!(command_label("fetch"), "FETCH");
    }

    #[test]
    fn select_command_maps_correctly() {
        assert_eq!(command_label("SELECT"), "SELECT");
        assert_eq!(command_label("select"), "SELECT");
    }

    #[test]
    fn append_command_maps_correctly() {
        assert_eq!(command_label("APPEND"), "APPEND");
        assert_eq!(command_label("append"), "APPEND");
    }

    #[test]
    fn unknown_command_maps_to_other() {
        assert_eq!(command_label("XXXXXXXX"), "OTHER");
    }

    #[test]
    fn unknown_empty_command_maps_to_other() {
        assert_eq!(command_label(""), "OTHER");
    }

    #[test]
    fn list_command_maps_correctly() {
        assert_eq!(command_label("LIST"), "LIST");
        assert_eq!(command_label("list"), "LIST");
    }

    #[test]
    fn lsub_command_maps_correctly() {
        assert_eq!(command_label("LSUB"), "LSUB");
        assert_eq!(command_label("lsub"), "LSUB");
    }

    #[test]
    fn status_command_maps_correctly() {
        assert_eq!(command_label("STATUS"), "STATUS");
        assert_eq!(command_label("status"), "STATUS");
    }

    #[test]
    fn subscribe_command_maps_correctly() {
        assert_eq!(command_label("SUBSCRIBE"), "SUBSCRIBE");
        assert_eq!(command_label("subscribe"), "SUBSCRIBE");
    }

    #[test]
    fn unsubscribe_command_maps_correctly() {
        assert_eq!(command_label("UNSUBSCRIBE"), "UNSUBSCRIBE");
        assert_eq!(command_label("unsubscribe"), "UNSUBSCRIBE");
    }

    #[test]
    fn capability_command_maps_correctly() {
        assert_eq!(command_label("CAPABILITY"), "CAPABILITY");
        assert_eq!(command_label("capability"), "CAPABILITY");
    }

    #[test]
    fn numeric_tag_and_unknown_both_map_to_other() {
        assert_eq!(command_label("A001"), "OTHER");
        assert_eq!(command_label("B999"), "OTHER");
    }

    #[test]
    fn examine_command_maps_correctly() {
        assert_eq!(command_label("EXAMINE"), "EXAMINE");
        assert_eq!(command_label("examine"), "EXAMINE");
    }

    #[test]
    fn copy_command_maps_correctly() {
        assert_eq!(command_label("COPY"), "COPY");
        assert_eq!(command_label("copy"), "COPY");
    }

    #[test]
    fn thread_command_is_case_insensitive() {
        assert_eq!(command_label("THREAD"), "THREAD");
        assert_eq!(command_label("thread"), "THREAD");
    }
}

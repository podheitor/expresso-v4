//! POP3 command parsing (RFC 1939 §5–§7). Commands are CRLF-delimited ASCII:
//! a keyword followed by at most one or two space-separated arguments. The
//! keyword is case-insensitive; arguments (message numbers) are decimal.

/// A parsed POP3 command. Unknown keywords map to `Unknown` so the session can
/// reply `-ERR` without crashing. Argument parsing is intentionally lenient:
/// malformed numbers surface as `None` and the session rejects them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pop3Command {
    User(String),
    Pass(String),
    Stat,
    List(Option<u32>),
    Uidl(Option<u32>),
    Retr(u32),
    Dele(u32),
    Top(u32, u32),
    Noop,
    Rset,
    Quit,
    Capa,
    /// A recognized keyword with a missing/invalid required argument.
    Invalid(&'static str),
    Unknown,
}

impl Pop3Command {
    /// Keyword used for metrics labels, regardless of argument validity.
    pub fn name(&self) -> &'static str {
        match self {
            Pop3Command::User(_) => "USER",
            Pop3Command::Pass(_) => "PASS",
            Pop3Command::Stat => "STAT",
            Pop3Command::List(_) => "LIST",
            Pop3Command::Uidl(_) => "UIDL",
            Pop3Command::Retr(_) => "RETR",
            Pop3Command::Dele(_) => "DELE",
            Pop3Command::Top(_, _) => "TOP",
            Pop3Command::Noop => "NOOP",
            Pop3Command::Rset => "RSET",
            Pop3Command::Quit => "QUIT",
            Pop3Command::Capa => "CAPA",
            Pop3Command::Invalid(kw) => kw,
            Pop3Command::Unknown => "UNKNOWN",
        }
    }
}

/// Parse one POP3 command line (without the trailing CRLF).
pub fn parse(line: &str) -> Pop3Command {
    let mut parts = line.split_whitespace();
    let Some(keyword) = parts.next() else {
        return Pop3Command::Unknown;
    };
    let arg1 = parts.next();
    let arg2 = parts.next();
    let num1 = arg1.and_then(|s| s.parse::<u32>().ok());

    match keyword.to_ascii_uppercase().as_str() {
        "USER" => match arg1 {
            Some(u) if !u.is_empty() => Pop3Command::User(u.to_owned()),
            _ => Pop3Command::Invalid("USER"),
        },
        // PASS may legitimately contain spaces; take everything after the keyword.
        "PASS" => match line.trim_end().split_once(char::is_whitespace) {
            Some((_, rest)) if !rest.is_empty() => Pop3Command::Pass(rest.to_owned()),
            _ => Pop3Command::Invalid("PASS"),
        },
        "STAT" => Pop3Command::Stat,
        "LIST" => Pop3Command::List(num1),
        "UIDL" => Pop3Command::Uidl(num1),
        "RETR" => num1.map_or(Pop3Command::Invalid("RETR"), Pop3Command::Retr),
        "DELE" => num1.map_or(Pop3Command::Invalid("DELE"), Pop3Command::Dele),
        "TOP" => match (num1, arg2.and_then(|s| s.parse::<u32>().ok())) {
            (Some(n), Some(lines)) => Pop3Command::Top(n, lines),
            _ => Pop3Command::Invalid("TOP"),
        },
        "NOOP" => Pop3Command::Noop,
        "RSET" => Pop3Command::Rset,
        "QUIT" => Pop3Command::Quit,
        "CAPA" => Pop3Command::Capa,
        _ => Pop3Command::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_user() {
        assert_eq!(
            parse("USER alice@x.com"),
            Pop3Command::User("alice@x.com".into())
        );
    }

    #[test]
    fn parse_user_case_insensitive() {
        assert_eq!(parse("user bob"), Pop3Command::User("bob".into()));
    }

    #[test]
    fn parse_user_missing_arg() {
        assert_eq!(parse("USER"), Pop3Command::Invalid("USER"));
    }

    #[test]
    fn parse_pass_with_spaces() {
        assert_eq!(
            parse("PASS my secret pw"),
            Pop3Command::Pass("my secret pw".into())
        );
    }

    #[test]
    fn parse_pass_missing() {
        assert_eq!(parse("PASS"), Pop3Command::Invalid("PASS"));
    }

    #[test]
    fn parse_stat() {
        assert_eq!(parse("STAT"), Pop3Command::Stat);
    }

    #[test]
    fn parse_list_no_arg() {
        assert_eq!(parse("LIST"), Pop3Command::List(None));
    }

    #[test]
    fn parse_list_with_arg() {
        assert_eq!(parse("LIST 3"), Pop3Command::List(Some(3)));
    }

    #[test]
    fn parse_uidl_with_arg() {
        assert_eq!(parse("UIDL 7"), Pop3Command::Uidl(Some(7)));
    }

    #[test]
    fn parse_retr() {
        assert_eq!(parse("RETR 1"), Pop3Command::Retr(1));
    }

    #[test]
    fn parse_retr_missing_arg() {
        assert_eq!(parse("RETR"), Pop3Command::Invalid("RETR"));
    }

    #[test]
    fn parse_retr_non_numeric() {
        assert_eq!(parse("RETR abc"), Pop3Command::Invalid("RETR"));
    }

    #[test]
    fn parse_dele() {
        assert_eq!(parse("DELE 2"), Pop3Command::Dele(2));
    }

    #[test]
    fn parse_top() {
        assert_eq!(parse("TOP 1 10"), Pop3Command::Top(1, 10));
    }

    #[test]
    fn parse_top_missing_lines() {
        assert_eq!(parse("TOP 1"), Pop3Command::Invalid("TOP"));
    }

    #[test]
    fn parse_top_zero_lines() {
        assert_eq!(parse("TOP 4 0"), Pop3Command::Top(4, 0));
    }

    #[test]
    fn parse_noop_rset_quit_capa() {
        assert_eq!(parse("NOOP"), Pop3Command::Noop);
        assert_eq!(parse("RSET"), Pop3Command::Rset);
        assert_eq!(parse("QUIT"), Pop3Command::Quit);
        assert_eq!(parse("CAPA"), Pop3Command::Capa);
    }

    #[test]
    fn parse_unknown() {
        assert_eq!(parse("FROBNICATE"), Pop3Command::Unknown);
    }

    #[test]
    fn parse_empty_line() {
        assert_eq!(parse(""), Pop3Command::Unknown);
    }

    #[test]
    fn parse_trims_trailing_cr() {
        assert_eq!(parse("STAT\r"), Pop3Command::Stat);
    }

    #[test]
    fn name_for_metrics() {
        assert_eq!(parse("RETR 1").name(), "RETR");
        assert_eq!(parse("FOO").name(), "UNKNOWN");
    }
}

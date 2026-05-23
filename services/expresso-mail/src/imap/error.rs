use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImapError {
    NotAuthenticated,
    AlreadyAuthenticated,
    NoMailboxSelected,
    MailboxNotFound(String),
    MailboxAlreadyExists(String),
    AuthFailed,
    LockedOut,
    UnsupportedCharset(String),
    UnsupportedMechanism(String),
    BadSequence(String),
    ParseError(String),
    InternalError(String),
    PermissionDenied,
    ReadOnly,
}

impl fmt::Display for ImapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImapError::NotAuthenticated => write!(f, "not authenticated"),
            ImapError::AlreadyAuthenticated => write!(f, "already authenticated"),
            ImapError::NoMailboxSelected => write!(f, "no mailbox selected"),
            ImapError::MailboxNotFound(name) => write!(f, "mailbox not found: {}", name),
            ImapError::MailboxAlreadyExists(name) => write!(f, "mailbox already exists: {}", name),
            ImapError::AuthFailed => write!(f, "authentication failed"),
            ImapError::LockedOut => write!(f, "account temporarily locked"),
            ImapError::UnsupportedCharset(cs) => write!(f, "unsupported charset: {}", cs),
            ImapError::UnsupportedMechanism(m) => write!(f, "unsupported mechanism: {}", m),
            ImapError::BadSequence(cmd) => write!(f, "bad command sequence: {}", cmd),
            ImapError::ParseError(msg) => write!(f, "parse error: {}", msg),
            ImapError::InternalError(msg) => write!(f, "internal error: {}", msg),
            ImapError::PermissionDenied => write!(f, "permission denied"),
            ImapError::ReadOnly => write!(f, "mailbox is read-only"),
        }
    }
}

impl ImapError {
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            ImapError::NotAuthenticated
                | ImapError::AlreadyAuthenticated
                | ImapError::NoMailboxSelected
                | ImapError::MailboxNotFound(_)
                | ImapError::MailboxAlreadyExists(_)
                | ImapError::AuthFailed
                | ImapError::UnsupportedCharset(_)
                | ImapError::UnsupportedMechanism(_)
                | ImapError::BadSequence(_)
                | ImapError::ParseError(_)
                | ImapError::PermissionDenied
                | ImapError::ReadOnly
        )
    }

    pub fn imap_response_tag(&self) -> &'static str {
        match self {
            ImapError::AuthFailed | ImapError::LockedOut | ImapError::NotAuthenticated
            | ImapError::AlreadyAuthenticated => "NO",
            ImapError::InternalError(_) => "NO",
            ImapError::ParseError(_) | ImapError::BadSequence(_) => "BAD",
            _ => "NO",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_not_authenticated() {
        assert_eq!(ImapError::NotAuthenticated.to_string(), "not authenticated");
    }

    #[test]
    fn display_already_authenticated() {
        assert_eq!(ImapError::AlreadyAuthenticated.to_string(), "already authenticated");
    }

    #[test]
    fn display_no_mailbox_selected() {
        assert_eq!(ImapError::NoMailboxSelected.to_string(), "no mailbox selected");
    }

    #[test]
    fn display_mailbox_not_found_contains_name() {
        let e = ImapError::MailboxNotFound("INBOX".into());
        assert!(e.to_string().contains("INBOX"));
    }

    #[test]
    fn display_mailbox_already_exists_contains_name() {
        let e = ImapError::MailboxAlreadyExists("Sent".into());
        assert!(e.to_string().contains("Sent"));
    }

    #[test]
    fn display_auth_failed() {
        assert!(ImapError::AuthFailed.to_string().contains("authentication failed"));
    }

    #[test]
    fn display_locked_out() {
        assert!(ImapError::LockedOut.to_string().contains("locked"));
    }

    #[test]
    fn display_unsupported_charset_contains_name() {
        let e = ImapError::UnsupportedCharset("ISO-2022-JP".into());
        assert!(e.to_string().contains("ISO-2022-JP"));
    }

    #[test]
    fn display_unsupported_mechanism_contains_name() {
        let e = ImapError::UnsupportedMechanism("GSSAPI".into());
        assert!(e.to_string().contains("GSSAPI"));
    }

    #[test]
    fn display_bad_sequence_contains_cmd() {
        let e = ImapError::BadSequence("DATA".into());
        assert!(e.to_string().contains("DATA"));
    }

    #[test]
    fn display_parse_error_contains_msg() {
        let e = ImapError::ParseError("unexpected token".into());
        assert!(e.to_string().contains("unexpected token"));
    }

    #[test]
    fn display_internal_error_contains_msg() {
        let e = ImapError::InternalError("db down".into());
        assert!(e.to_string().contains("db down"));
    }

    #[test]
    fn display_permission_denied() {
        assert_eq!(ImapError::PermissionDenied.to_string(), "permission denied");
    }

    #[test]
    fn display_read_only() {
        assert!(ImapError::ReadOnly.to_string().contains("read-only"));
    }

    #[test]
    fn not_authenticated_is_client_error() {
        assert!(ImapError::NotAuthenticated.is_client_error());
    }

    #[test]
    fn internal_error_is_not_client_error() {
        assert!(!ImapError::InternalError("x".into()).is_client_error());
    }

    #[test]
    fn locked_out_is_not_client_error() {
        assert!(!ImapError::LockedOut.is_client_error());
    }

    #[test]
    fn parse_error_is_client_error() {
        assert!(ImapError::ParseError("x".into()).is_client_error());
    }

    #[test]
    fn parse_error_tag_is_bad() {
        assert_eq!(ImapError::ParseError("x".into()).imap_response_tag(), "BAD");
    }

    #[test]
    fn bad_sequence_tag_is_bad() {
        assert_eq!(ImapError::BadSequence("x".into()).imap_response_tag(), "BAD");
    }

    #[test]
    fn auth_failed_tag_is_no() {
        assert_eq!(ImapError::AuthFailed.imap_response_tag(), "NO");
    }

    #[test]
    fn equality_same_variant() {
        assert_eq!(ImapError::NotAuthenticated, ImapError::NotAuthenticated);
    }

    #[test]
    fn inequality_different_variants() {
        assert_ne!(ImapError::NotAuthenticated, ImapError::AuthFailed);
    }

    #[test]
    fn read_only_is_client_error() {
        assert!(ImapError::ReadOnly.is_client_error());
    }

    #[test]
    fn permission_denied_is_client_error() {
        assert!(ImapError::PermissionDenied.is_client_error());
    }

    #[test]
    fn mailbox_already_exists_is_client_error() {
        assert!(ImapError::MailboxAlreadyExists("Drafts".into()).is_client_error());
    }
}

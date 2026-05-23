use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmtpError {
    MessageTooLarge { limit: usize, actual: usize },
    TooManyRecipients { limit: usize },
    InvalidAddress(String),
    AuthRequired,
    AuthFailed,
    TlsRequired,
    MailboxFull,
    RelayDenied(String),
    BadSequence(String),
    InternalError(String),
}

impl fmt::Display for SmtpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SmtpError::MessageTooLarge { limit, actual } =>
                write!(f, "message too large: {} > {}", actual, limit),
            SmtpError::TooManyRecipients { limit } =>
                write!(f, "too many recipients (max {})", limit),
            SmtpError::InvalidAddress(addr) =>
                write!(f, "invalid address: {}", addr),
            SmtpError::AuthRequired =>
                write!(f, "authentication required"),
            SmtpError::AuthFailed =>
                write!(f, "authentication failed"),
            SmtpError::TlsRequired =>
                write!(f, "TLS required"),
            SmtpError::MailboxFull =>
                write!(f, "mailbox full"),
            SmtpError::RelayDenied(domain) =>
                write!(f, "relay denied for {}", domain),
            SmtpError::BadSequence(cmd) =>
                write!(f, "bad sequence of commands: {}", cmd),
            SmtpError::InternalError(msg) =>
                write!(f, "internal error: {}", msg),
        }
    }
}

impl SmtpError {
    pub fn smtp_code(&self) -> u16 {
        match self {
            SmtpError::MessageTooLarge { .. } => 552,
            SmtpError::TooManyRecipients { .. } => 452,
            SmtpError::InvalidAddress(_) => 501,
            SmtpError::AuthRequired => 530,
            SmtpError::AuthFailed => 535,
            SmtpError::TlsRequired => 530,
            SmtpError::MailboxFull => 452,
            SmtpError::RelayDenied(_) => 550,
            SmtpError::BadSequence(_) => 503,
            SmtpError::InternalError(_) => 451,
        }
    }

    pub fn is_transient(&self) -> bool {
        matches!(self.smtp_code(), 400..=499)
    }

    pub fn is_permanent(&self) -> bool {
        matches!(self.smtp_code(), 500..=599)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_too_large_code_is_552() {
        let e = SmtpError::MessageTooLarge { limit: 10, actual: 20 };
        assert_eq!(e.smtp_code(), 552);
    }

    #[test]
    fn too_many_recipients_code_is_452() {
        let e = SmtpError::TooManyRecipients { limit: 100 };
        assert_eq!(e.smtp_code(), 452);
    }

    #[test]
    fn invalid_address_code_is_501() {
        let e = SmtpError::InvalidAddress("bad@".into());
        assert_eq!(e.smtp_code(), 501);
    }

    #[test]
    fn auth_required_code_is_530() {
        assert_eq!(SmtpError::AuthRequired.smtp_code(), 530);
    }

    #[test]
    fn auth_failed_code_is_535() {
        assert_eq!(SmtpError::AuthFailed.smtp_code(), 535);
    }

    #[test]
    fn relay_denied_code_is_550() {
        let e = SmtpError::RelayDenied("spam.net".into());
        assert_eq!(e.smtp_code(), 550);
    }

    #[test]
    fn bad_sequence_code_is_503() {
        let e = SmtpError::BadSequence("DATA".into());
        assert_eq!(e.smtp_code(), 503);
    }

    #[test]
    fn internal_error_code_is_451() {
        let e = SmtpError::InternalError("db".into());
        assert_eq!(e.smtp_code(), 451);
    }

    #[test]
    fn mailbox_full_code_is_452() {
        assert_eq!(SmtpError::MailboxFull.smtp_code(), 452);
    }

    #[test]
    fn tls_required_code_is_530() {
        assert_eq!(SmtpError::TlsRequired.smtp_code(), 530);
    }

    #[test]
    fn internal_error_is_transient() {
        let e = SmtpError::InternalError("x".into());
        assert!(e.is_transient());
        assert!(!e.is_permanent());
    }

    #[test]
    fn too_many_recipients_is_transient() {
        let e = SmtpError::TooManyRecipients { limit: 100 };
        assert!(e.is_transient());
    }

    #[test]
    fn auth_failed_is_permanent() {
        assert!(SmtpError::AuthFailed.is_permanent());
        assert!(!SmtpError::AuthFailed.is_transient());
    }

    #[test]
    fn relay_denied_is_permanent() {
        let e = SmtpError::RelayDenied("x.com".into());
        assert!(e.is_permanent());
    }

    #[test]
    fn display_message_too_large_contains_sizes() {
        let e = SmtpError::MessageTooLarge { limit: 1000, actual: 2000 };
        let s = e.to_string();
        assert!(s.contains("2000"));
        assert!(s.contains("1000"));
    }

    #[test]
    fn display_too_many_recipients_contains_limit() {
        let e = SmtpError::TooManyRecipients { limit: 50 };
        assert!(e.to_string().contains("50"));
    }

    #[test]
    fn display_invalid_address_contains_addr() {
        let e = SmtpError::InvalidAddress("bad@x".into());
        assert!(e.to_string().contains("bad@x"));
    }

    #[test]
    fn display_relay_denied_contains_domain() {
        let e = SmtpError::RelayDenied("evil.com".into());
        assert!(e.to_string().contains("evil.com"));
    }

    #[test]
    fn display_bad_sequence_contains_command() {
        let e = SmtpError::BadSequence("RCPT".into());
        assert!(e.to_string().contains("RCPT"));
    }

    #[test]
    fn display_internal_error_contains_message() {
        let e = SmtpError::InternalError("disk full".into());
        assert!(e.to_string().contains("disk full"));
    }

    #[test]
    fn equality_same_variant() {
        assert_eq!(SmtpError::AuthRequired, SmtpError::AuthRequired);
        assert_eq!(SmtpError::AuthFailed, SmtpError::AuthFailed);
    }

    #[test]
    fn inequality_different_variants() {
        assert_ne!(SmtpError::AuthRequired, SmtpError::AuthFailed);
    }

    #[test]
    fn tls_required_is_permanent() {
        assert!(SmtpError::TlsRequired.is_permanent());
    }

    #[test]
    fn mailbox_full_is_transient() {
        assert!(SmtpError::MailboxFull.is_transient());
    }

    #[test]
    fn invalid_address_is_permanent() {
        let e = SmtpError::InvalidAddress("bad@".into());
        assert!(e.is_permanent());
        assert!(!e.is_transient());
    }
}

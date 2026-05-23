use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ImapCapability {
    Imap4Rev1,
    StartTls,
    AuthPlain,
    Idle,
    UidPlus,
    Unselect,
    Move,
    LiteralPlus,
    Enable,
    Sort,
    ThreadOrderedSubject,
    StatusSize,
    Namespace,
    CondStore,
    QResync,
    SaslIr,
}

impl fmt::Display for ImapCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ImapCapability::Imap4Rev1            => "IMAP4rev1",
            ImapCapability::StartTls             => "STARTTLS",
            ImapCapability::AuthPlain            => "AUTH=PLAIN",
            ImapCapability::Idle                 => "IDLE",
            ImapCapability::UidPlus              => "UIDPLUS",
            ImapCapability::Unselect             => "UNSELECT",
            ImapCapability::Move                 => "MOVE",
            ImapCapability::LiteralPlus          => "LITERAL+",
            ImapCapability::Enable               => "ENABLE",
            ImapCapability::Sort                 => "SORT",
            ImapCapability::ThreadOrderedSubject => "THREAD=ORDEREDSUBJECT",
            ImapCapability::StatusSize           => "STATUS=SIZE",
            ImapCapability::Namespace            => "NAMESPACE",
            ImapCapability::CondStore            => "CONDSTORE",
            ImapCapability::QResync              => "QRESYNC",
            ImapCapability::SaslIr               => "SASL-IR",
        };
        write!(f, "{}", s)
    }
}

pub fn all_capabilities() -> Vec<ImapCapability> {
    vec![
        ImapCapability::Imap4Rev1,
        ImapCapability::AuthPlain,
        ImapCapability::Idle,
        ImapCapability::UidPlus,
        ImapCapability::Unselect,
        ImapCapability::Move,
        ImapCapability::LiteralPlus,
        ImapCapability::Enable,
        ImapCapability::Sort,
        ImapCapability::ThreadOrderedSubject,
        ImapCapability::StatusSize,
        ImapCapability::Namespace,
        ImapCapability::CondStore,
        ImapCapability::QResync,
    ]
}

pub fn capability_string(caps: &[ImapCapability]) -> String {
    let parts: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
    format!("CAPABILITY {}", parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imap4rev1_display() {
        assert_eq!(ImapCapability::Imap4Rev1.to_string(), "IMAP4rev1");
    }

    #[test]
    fn auth_plain_display() {
        assert_eq!(ImapCapability::AuthPlain.to_string(), "AUTH=PLAIN");
    }

    #[test]
    fn idle_display() {
        assert_eq!(ImapCapability::Idle.to_string(), "IDLE");
    }

    #[test]
    fn uidplus_display() {
        assert_eq!(ImapCapability::UidPlus.to_string(), "UIDPLUS");
    }

    #[test]
    fn unselect_display() {
        assert_eq!(ImapCapability::Unselect.to_string(), "UNSELECT");
    }

    #[test]
    fn move_display() {
        assert_eq!(ImapCapability::Move.to_string(), "MOVE");
    }

    #[test]
    fn literal_plus_display() {
        assert_eq!(ImapCapability::LiteralPlus.to_string(), "LITERAL+");
    }

    #[test]
    fn enable_display() {
        assert_eq!(ImapCapability::Enable.to_string(), "ENABLE");
    }

    #[test]
    fn sort_display() {
        assert_eq!(ImapCapability::Sort.to_string(), "SORT");
    }

    #[test]
    fn thread_ordered_subject_display() {
        assert_eq!(ImapCapability::ThreadOrderedSubject.to_string(), "THREAD=ORDEREDSUBJECT");
    }

    #[test]
    fn status_size_display() {
        assert_eq!(ImapCapability::StatusSize.to_string(), "STATUS=SIZE");
    }

    #[test]
    fn namespace_display() {
        assert_eq!(ImapCapability::Namespace.to_string(), "NAMESPACE");
    }

    #[test]
    fn condstore_display() {
        assert_eq!(ImapCapability::CondStore.to_string(), "CONDSTORE");
    }

    #[test]
    fn qresync_display() {
        assert_eq!(ImapCapability::QResync.to_string(), "QRESYNC");
    }

    #[test]
    fn sasl_ir_display() {
        assert_eq!(ImapCapability::SaslIr.to_string(), "SASL-IR");
    }

    #[test]
    fn all_capabilities_non_empty() {
        assert!(!all_capabilities().is_empty());
    }

    #[test]
    fn all_capabilities_contains_imap4rev1() {
        assert!(all_capabilities().contains(&ImapCapability::Imap4Rev1));
    }

    #[test]
    fn all_capabilities_contains_idle() {
        assert!(all_capabilities().contains(&ImapCapability::Idle));
    }

    #[test]
    fn capability_string_starts_with_capability() {
        let s = capability_string(&[ImapCapability::Imap4Rev1]);
        assert!(s.starts_with("CAPABILITY "));
    }

    #[test]
    fn capability_string_contains_all_names() {
        let caps = vec![ImapCapability::Idle, ImapCapability::UidPlus];
        let s = capability_string(&caps);
        assert!(s.contains("IDLE"));
        assert!(s.contains("UIDPLUS"));
    }

    #[test]
    fn capability_equality() {
        assert_eq!(ImapCapability::CondStore, ImapCapability::CondStore);
    }

    #[test]
    fn capability_inequality() {
        assert_ne!(ImapCapability::CondStore, ImapCapability::QResync);
    }

    #[test]
    fn starttls_display() {
        assert_eq!(ImapCapability::StartTls.to_string(), "STARTTLS");
    }

    #[test]
    fn capability_string_empty_list() {
        let s = capability_string(&[]);
        assert_eq!(s, "CAPABILITY ");
    }

    #[test]
    fn all_capabilities_does_not_contain_sasl_ir() {
        assert!(!all_capabilities().contains(&ImapCapability::SaslIr));
    }

    #[test]
    fn all_capabilities_count_is_fourteen() {
        assert_eq!(all_capabilities().len(), 14);
    }
}

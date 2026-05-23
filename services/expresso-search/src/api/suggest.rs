use serde::{Deserialize, Serialize};

pub const MAX_SUGGEST_QUERY_BYTES: usize = 256;
pub const MAX_SUGGEST_RESULTS: usize = 10;
pub const DEFAULT_SUGGEST_RESULTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestKind {
    Subject,
    Sender,
    Recipient,
    All,
}

impl std::fmt::Display for SuggestKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SuggestKind::Subject   => write!(f, "subject"),
            SuggestKind::Sender    => write!(f, "sender"),
            SuggestKind::Recipient => write!(f, "recipient"),
            SuggestKind::All       => write!(f, "all"),
        }
    }
}

impl SuggestKind {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "subject"   => Some(SuggestKind::Subject),
            "sender"    => Some(SuggestKind::Sender),
            "recipient" => Some(SuggestKind::Recipient),
            "all"       => Some(SuggestKind::All),
            _           => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestParams {
    pub q:         String,
    pub tenant_id: String,
    #[serde(default = "default_limit")]
    pub limit:     usize,
    pub kind:      Option<String>,
}

fn default_limit() -> usize {
    DEFAULT_SUGGEST_RESULTS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuggestResult {
    pub text:  String,
    pub score: f32,
    pub kind:  String,
}

pub fn is_valid_suggest_query(q: &str) -> bool {
    !q.is_empty() && q.len() <= MAX_SUGGEST_QUERY_BYTES
}

pub fn clamp_suggest_limit(limit: usize) -> usize {
    limit.min(MAX_SUGGEST_RESULTS).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_suggest_query_bytes_is_256() {
        assert_eq!(MAX_SUGGEST_QUERY_BYTES, 256);
    }

    #[test]
    fn max_suggest_results_is_10() {
        assert_eq!(MAX_SUGGEST_RESULTS, 10);
    }

    #[test]
    fn default_suggest_results_is_5() {
        assert_eq!(DEFAULT_SUGGEST_RESULTS, 5);
    }

    #[test]
    fn suggest_kind_display_subject() {
        assert_eq!(SuggestKind::Subject.to_string(), "subject");
    }

    #[test]
    fn suggest_kind_display_sender() {
        assert_eq!(SuggestKind::Sender.to_string(), "sender");
    }

    #[test]
    fn suggest_kind_display_recipient() {
        assert_eq!(SuggestKind::Recipient.to_string(), "recipient");
    }

    #[test]
    fn suggest_kind_display_all() {
        assert_eq!(SuggestKind::All.to_string(), "all");
    }

    #[test]
    fn from_str_subject() {
        assert_eq!(SuggestKind::from_str("subject"), Some(SuggestKind::Subject));
    }

    #[test]
    fn from_str_sender() {
        assert_eq!(SuggestKind::from_str("sender"), Some(SuggestKind::Sender));
    }

    #[test]
    fn from_str_recipient() {
        assert_eq!(SuggestKind::from_str("recipient"), Some(SuggestKind::Recipient));
    }

    #[test]
    fn from_str_all() {
        assert_eq!(SuggestKind::from_str("all"), Some(SuggestKind::All));
    }

    #[test]
    fn from_str_uppercase_subject() {
        assert_eq!(SuggestKind::from_str("SUBJECT"), Some(SuggestKind::Subject));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(SuggestKind::from_str("thread"), None);
    }

    #[test]
    fn valid_query_non_empty() {
        assert!(is_valid_suggest_query("hello"));
    }

    #[test]
    fn invalid_query_empty() {
        assert!(!is_valid_suggest_query(""));
    }

    #[test]
    fn invalid_query_too_long() {
        let long = "a".repeat(MAX_SUGGEST_QUERY_BYTES + 1);
        assert!(!is_valid_suggest_query(&long));
    }

    #[test]
    fn valid_query_at_max_bytes() {
        let exact = "a".repeat(MAX_SUGGEST_QUERY_BYTES);
        assert!(is_valid_suggest_query(&exact));
    }

    #[test]
    fn clamp_limit_above_max() {
        assert_eq!(clamp_suggest_limit(100), MAX_SUGGEST_RESULTS);
    }

    #[test]
    fn clamp_limit_below_min() {
        assert_eq!(clamp_suggest_limit(0), 1);
    }

    #[test]
    fn clamp_limit_within_range() {
        assert_eq!(clamp_suggest_limit(7), 7);
    }

    #[test]
    fn suggest_kind_equality() {
        assert_eq!(SuggestKind::All, SuggestKind::All);
        assert_ne!(SuggestKind::All, SuggestKind::Sender);
    }

    #[test]
    fn serde_roundtrip_suggest_kind() {
        let kind = SuggestKind::Sender;
        let json = serde_json::to_string(&kind).unwrap();
        let back: SuggestKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SuggestKind::Sender);
    }

    #[test]
    fn default_limit_fn_returns_5() {
        assert_eq!(default_limit(), 5);
    }

    #[test]
    fn clamp_limit_at_max() {
        assert_eq!(clamp_suggest_limit(MAX_SUGGEST_RESULTS), MAX_SUGGEST_RESULTS);
    }
}

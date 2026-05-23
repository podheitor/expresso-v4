//! Top senders analytics types and helpers — sprint #19857.

use serde::{Deserialize, Serialize};

/// A top-sender entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopSenderEntry {
    pub from_addr:   String,
    pub domain:      String,
    pub message_count: u64,
}

impl TopSenderEntry {
    /// Extract domain from an email address (part after '@').
    /// Returns the full address as fallback when '@' is absent.
    pub fn domain_from_addr(addr: &str) -> String {
        addr.rsplit_once('@')
            .map(|(_, d)| d.to_ascii_lowercase())
            .unwrap_or_else(|| addr.to_ascii_lowercase())
    }
}

/// Request parameters for top senders endpoint.
#[derive(Debug, Deserialize)]
pub struct TopSendersParams {
    pub tenant_id: String,
    pub top_n:     Option<usize>,
    pub since_days: Option<u32>,
}

pub const DEFAULT_TOP_N:      usize = 20;
pub const MAX_TOP_N:          usize = 100;
pub const DEFAULT_SINCE_DAYS: u32   = 30;
pub const MAX_SINCE_DAYS:     u32   = 365;

pub fn validate_top_senders_params(p: &TopSendersParams) -> Option<String> {
    if p.tenant_id.trim().is_empty() {
        return Some("tenant_id must not be empty".into());
    }
    if let Some(n) = p.top_n {
        if n == 0 {
            return Some("top_n must be >= 1".into());
        }
        if n > MAX_TOP_N {
            return Some(format!("top_n must be <= {}", MAX_TOP_N));
        }
    }
    if let Some(d) = p.since_days {
        if d == 0 {
            return Some("since_days must be >= 1".into());
        }
        if d > MAX_SINCE_DAYS {
            return Some(format!("since_days must be <= {}", MAX_SINCE_DAYS));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(tenant: &str, top_n: Option<usize>, since_days: Option<u32>) -> TopSendersParams {
        TopSendersParams { tenant_id: tenant.into(), top_n, since_days }
    }

    #[test]
    fn domain_from_addr_basic() {
        assert_eq!(TopSenderEntry::domain_from_addr("alice@example.com"), "example.com");
    }

    #[test]
    fn domain_from_addr_uppercase_normalized() {
        assert_eq!(TopSenderEntry::domain_from_addr("alice@EXAMPLE.COM"), "example.com");
    }

    #[test]
    fn domain_from_addr_no_at_returns_full_addr() {
        assert_eq!(TopSenderEntry::domain_from_addr("noatsign"), "noatsign");
    }

    #[test]
    fn domain_from_addr_multiple_at_uses_last() {
        let result = TopSenderEntry::domain_from_addr("a@b@c.com");
        assert_eq!(result, "c.com");
    }

    #[test]
    fn validate_empty_tenant_returns_error() {
        assert!(validate_top_senders_params(&make_params("", None, None)).is_some());
    }

    #[test]
    fn validate_valid_params_returns_none() {
        assert!(validate_top_senders_params(&make_params("t1", Some(10), Some(7))).is_none());
    }

    #[test]
    fn validate_top_n_zero_returns_error() {
        assert!(validate_top_senders_params(&make_params("t1", Some(0), None)).is_some());
    }

    #[test]
    fn validate_top_n_above_max_returns_error() {
        assert!(validate_top_senders_params(&make_params("t1", Some(MAX_TOP_N + 1), None)).is_some());
    }

    #[test]
    fn validate_top_n_at_max_is_ok() {
        assert!(validate_top_senders_params(&make_params("t1", Some(MAX_TOP_N), None)).is_none());
    }

    #[test]
    fn validate_since_days_zero_returns_error() {
        assert!(validate_top_senders_params(&make_params("t1", None, Some(0))).is_some());
    }

    #[test]
    fn validate_since_days_above_max_returns_error() {
        assert!(validate_top_senders_params(&make_params("t1", None, Some(MAX_SINCE_DAYS + 1))).is_some());
    }

    #[test]
    fn validate_since_days_at_max_is_ok() {
        assert!(validate_top_senders_params(&make_params("t1", None, Some(MAX_SINCE_DAYS))).is_none());
    }

    #[test]
    fn default_top_n_is_20() {
        assert_eq!(DEFAULT_TOP_N, 20);
    }

    #[test]
    fn max_top_n_is_100() {
        assert_eq!(MAX_TOP_N, 100);
    }

    #[test]
    fn default_since_days_is_30() {
        assert_eq!(DEFAULT_SINCE_DAYS, 30);
    }

    #[test]
    fn max_since_days_is_365() {
        assert_eq!(MAX_SINCE_DAYS, 365);
    }

    #[test]
    fn top_sender_entry_fields_accessible() {
        let e = TopSenderEntry {
            from_addr: "alice@example.com".into(),
            domain: "example.com".into(),
            message_count: 99,
        };
        assert_eq!(e.from_addr, "alice@example.com");
        assert_eq!(e.message_count, 99);
    }

    #[test]
    fn top_sender_entry_serde_roundtrip() {
        let e = TopSenderEntry {
            from_addr: "bob@acme.org".into(),
            domain: "acme.org".into(),
            message_count: 7,
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: TopSenderEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.message_count, 7);
    }

    #[test]
    fn validate_whitespace_tenant_returns_error() {
        assert!(validate_top_senders_params(&make_params("  ", None, None)).is_some());
    }

    #[test]
    fn default_top_n_less_than_max_top_n() {
        assert!(DEFAULT_TOP_N < MAX_TOP_N);
    }

    #[test]
    fn domain_from_addr_empty_string() {
        assert_eq!(TopSenderEntry::domain_from_addr(""), "");
    }

    #[test]
    fn validate_no_optionals_is_ok() {
        assert!(validate_top_senders_params(&make_params("t1", None, None)).is_none());
    }

    #[test]
    fn top_sender_entry_domain_field() {
        let e = TopSenderEntry {
            from_addr: "alice@corp.io".into(),
            domain: "corp.io".into(),
            message_count: 5,
        };
        assert_eq!(e.domain, "corp.io");
    }

    #[test]
    fn domain_from_addr_subomain_preserved() {
        let d = TopSenderEntry::domain_from_addr("user@mail.example.org");
        assert_eq!(d, "mail.example.org");
    }

    #[test]
    fn validate_top_n_one_is_ok() {
        assert!(validate_top_senders_params(&make_params("t1", Some(1), None)).is_none());
    }
}

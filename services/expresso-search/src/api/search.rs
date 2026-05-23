//! Search request / response types and parameter validation.

use serde::{Deserialize, Serialize};

pub const MAX_QUERY_BYTES:  usize = 1024;
pub const MAX_LIMIT:        usize = 200;
pub const DEFAULT_LIMIT:    usize = 20;
pub const FACET_TOP_N:      usize = 50;

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q:                String,
    pub tenant_id:        String,
    #[serde(default = "default_limit")]
    pub limit:            usize,
    #[serde(default)]
    pub offset:           usize,
    pub facet:            Option<String>,
    pub facet_granularity: Option<String>,
    pub after_secs:       Option<u64>,
    pub before_secs:      Option<u64>,
}

fn default_limit() -> usize { DEFAULT_LIMIT }

#[derive(Debug, Serialize)]
pub struct FacetEntry {
    pub value: String,
    pub count: u64,
}

#[derive(Debug, Serialize, Default)]
pub struct SearchFacets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind:         Option<Vec<FacetEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_addr:    Option<Vec<FacetEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain:       Option<Vec<FacetEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_at:  Option<Vec<FacetEntry>>,
}

pub fn validate_search_params(params: &mut SearchParams) -> Option<String> {
    if params.q.len() > MAX_QUERY_BYTES {
        return Some(format!(
            "query too large: {} bytes (max {})", params.q.len(), MAX_QUERY_BYTES
        ));
    }
    if params.limit == 0 {
        return Some("limit must be >= 1".into());
    }
    if params.limit > MAX_LIMIT {
        params.limit = MAX_LIMIT;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_params(q: &str, tenant: &str, limit: usize) -> SearchParams {
        SearchParams {
            q: q.into(),
            tenant_id: tenant.into(),
            limit,
            offset: 0,
            facet: None,
            facet_granularity: None,
            after_secs: None,
            before_secs: None,
        }
    }

    #[test]
    fn validate_empty_query_is_ok() {
        let mut p = make_params("", "tenant-1", 10);
        assert!(validate_search_params(&mut p).is_none());
    }

    #[test]
    fn validate_normal_query_is_ok() {
        let mut p = make_params("meeting", "tenant-1", 10);
        assert!(validate_search_params(&mut p).is_none());
    }

    #[test]
    fn validate_query_too_large_returns_error() {
        let q = "x".repeat(MAX_QUERY_BYTES + 1);
        let mut p = make_params(&q, "tenant-1", 10);
        assert!(validate_search_params(&mut p).is_some());
    }

    #[test]
    fn validate_query_exactly_at_limit_is_ok() {
        let q = "x".repeat(MAX_QUERY_BYTES);
        let mut p = make_params(&q, "tenant-1", 10);
        assert!(validate_search_params(&mut p).is_none());
    }

    #[test]
    fn validate_limit_zero_returns_error() {
        let mut p = make_params("hello", "t1", 0);
        assert!(validate_search_params(&mut p).is_some());
    }

    #[test]
    fn validate_limit_above_max_is_clamped() {
        let mut p = make_params("hello", "t1", MAX_LIMIT + 100);
        assert!(validate_search_params(&mut p).is_none());
        assert_eq!(p.limit, MAX_LIMIT);
    }

    #[test]
    fn validate_limit_at_max_is_not_clamped() {
        let mut p = make_params("hello", "t1", MAX_LIMIT);
        assert!(validate_search_params(&mut p).is_none());
        assert_eq!(p.limit, MAX_LIMIT);
    }

    #[test]
    fn default_limit_value() {
        assert_eq!(DEFAULT_LIMIT, 20);
    }

    #[test]
    fn max_limit_value() {
        assert_eq!(MAX_LIMIT, 200);
    }

    #[test]
    fn max_query_bytes_value() {
        assert_eq!(MAX_QUERY_BYTES, 1024);
    }

    #[test]
    fn facet_top_n_is_positive() {
        assert!(FACET_TOP_N > 0);
    }

    #[test]
    fn default_limit_less_than_max_limit() {
        assert!(DEFAULT_LIMIT < MAX_LIMIT);
    }

    #[test]
    fn facet_entry_fields_accessible() {
        let e = FacetEntry { value: "mail".into(), count: 42 };
        assert_eq!(e.value, "mail");
        assert_eq!(e.count, 42);
    }

    #[test]
    fn facet_entry_count_zero() {
        let e = FacetEntry { value: "drive".into(), count: 0 };
        assert_eq!(e.count, 0);
    }

    #[test]
    fn search_facets_default_all_none() {
        let f: SearchFacets = Default::default();
        assert!(f.kind.is_none());
        assert!(f.from_addr.is_none());
        assert!(f.domain.is_none());
        assert!(f.received_at.is_none());
    }

    #[test]
    fn search_params_offset_preserved() {
        let p = SearchParams {
            q: "q".into(), tenant_id: "t".into(), limit: 10, offset: 5,
            facet: None, facet_granularity: None, after_secs: None, before_secs: None,
        };
        assert_eq!(p.offset, 5);
    }

    #[test]
    fn search_params_after_before_optional() {
        let p = make_params("q", "t", 10);
        assert!(p.after_secs.is_none());
        assert!(p.before_secs.is_none());
    }

    #[test]
    fn validate_error_message_mentions_bytes_for_large_query() {
        let q = "x".repeat(MAX_QUERY_BYTES + 1);
        let mut p = make_params(&q, "t", 10);
        let msg = validate_search_params(&mut p).unwrap();
        assert!(msg.contains("bytes"));
    }

    #[test]
    fn validate_error_message_mentions_limit_for_zero_limit() {
        let mut p = make_params("q", "t", 0);
        let msg = validate_search_params(&mut p).unwrap();
        assert!(msg.contains("limit"));
    }

    #[test]
    fn facet_entry_serde_roundtrip() {
        let e = FacetEntry { value: "chat".into(), count: 7 };
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("chat"));
        assert!(json.contains("7"));
    }

    #[test]
    fn search_params_facet_field_optional() {
        let p = make_params("q", "t", 10);
        assert!(p.facet.is_none());
    }

    #[test]
    fn search_params_granularity_field_optional() {
        let p = make_params("q", "t", 10);
        assert!(p.facet_granularity.is_none());
    }

    #[test]
    fn validate_limit_one_is_ok() {
        let mut p = make_params("q", "t", 1);
        assert!(validate_search_params(&mut p).is_none());
    }

    #[test]
    fn facet_top_n_less_than_max_limit() {
        assert!(FACET_TOP_N < MAX_LIMIT);
    }

    #[test]
    fn search_params_sort_field_optional() {
        let p = make_params("q", "t", 10);
        assert!(p.facet.is_none());
        assert!(p.offset == 0);
    }
}

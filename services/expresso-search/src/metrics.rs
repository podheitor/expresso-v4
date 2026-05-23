//! Prometheus metric name constants for expresso-search.

pub const METRIC_SEARCH_REQUESTS:   &str = "expresso_search_requests_total";
pub const METRIC_SEARCH_ERRORS:     &str = "expresso_search_errors_total";
pub const METRIC_INDEX_OPERATIONS:  &str = "expresso_search_index_operations_total";
pub const METRIC_SEARCH_LATENCY_MS: &str = "expresso_search_latency_ms";
pub const METRIC_INDEX_DOC_COUNT:   &str = "expresso_search_index_doc_count";

pub const LABEL_OPERATION: &str = "operation";
pub const LABEL_STATUS:    &str = "status";

pub const OP_SEARCH:       &str = "search";
pub const OP_INDEX:        &str = "index";
pub const OP_DELETE:       &str = "delete";
pub const OP_BULK_INDEX:   &str = "bulk_index";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_requests_metric_has_expresso_prefix() {
        assert!(METRIC_SEARCH_REQUESTS.starts_with("expresso_"));
    }

    #[test]
    fn search_errors_metric_has_expresso_prefix() {
        assert!(METRIC_SEARCH_ERRORS.starts_with("expresso_"));
    }

    #[test]
    fn index_operations_metric_has_expresso_prefix() {
        assert!(METRIC_INDEX_OPERATIONS.starts_with("expresso_"));
    }

    #[test]
    fn latency_metric_has_expresso_prefix() {
        assert!(METRIC_SEARCH_LATENCY_MS.starts_with("expresso_"));
    }

    #[test]
    fn doc_count_metric_has_expresso_prefix() {
        assert!(METRIC_INDEX_DOC_COUNT.starts_with("expresso_"));
    }

    #[test]
    fn counter_metrics_end_with_total() {
        assert!(METRIC_SEARCH_REQUESTS.ends_with("_total"));
        assert!(METRIC_SEARCH_ERRORS.ends_with("_total"));
        assert!(METRIC_INDEX_OPERATIONS.ends_with("_total"));
    }

    #[test]
    fn label_operation_value() {
        assert_eq!(LABEL_OPERATION, "operation");
    }

    #[test]
    fn label_status_value() {
        assert_eq!(LABEL_STATUS, "status");
    }

    #[test]
    fn op_search_value() {
        assert_eq!(OP_SEARCH, "search");
    }

    #[test]
    fn op_index_value() {
        assert_eq!(OP_INDEX, "index");
    }

    #[test]
    fn op_delete_value() {
        assert_eq!(OP_DELETE, "delete");
    }

    #[test]
    fn op_bulk_index_value() {
        assert_eq!(OP_BULK_INDEX, "bulk_index");
    }

    #[test]
    fn all_ops_are_distinct() {
        let ops = [OP_SEARCH, OP_INDEX, OP_DELETE, OP_BULK_INDEX];
        let mut sorted = ops.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4);
    }

    #[test]
    fn all_metric_names_nonempty() {
        for m in [
            METRIC_SEARCH_REQUESTS, METRIC_SEARCH_ERRORS,
            METRIC_INDEX_OPERATIONS, METRIC_SEARCH_LATENCY_MS,
            METRIC_INDEX_DOC_COUNT,
        ] {
            assert!(!m.is_empty());
        }
    }

    #[test]
    fn all_metric_names_are_lowercase_snake_case() {
        for m in [
            METRIC_SEARCH_REQUESTS, METRIC_SEARCH_ERRORS,
            METRIC_INDEX_OPERATIONS, METRIC_SEARCH_LATENCY_MS,
            METRIC_INDEX_DOC_COUNT,
        ] {
            assert!(!m.contains('-'), "metric contains hyphen: {m}");
            assert_eq!(m, m.to_ascii_lowercase(), "metric not lowercase: {m}");
        }
    }

    #[test]
    fn search_requests_contains_search() {
        assert!(METRIC_SEARCH_REQUESTS.contains("search"));
    }

    #[test]
    fn latency_metric_contains_latency() {
        assert!(METRIC_SEARCH_LATENCY_MS.contains("latency"));
    }

    #[test]
    fn doc_count_metric_contains_doc_count() {
        assert!(METRIC_INDEX_DOC_COUNT.contains("doc_count"));
    }

    #[test]
    fn labels_are_distinct() {
        assert_ne!(LABEL_OPERATION, LABEL_STATUS);
    }

    #[test]
    fn metric_names_are_all_distinct() {
        let names = [
            METRIC_SEARCH_REQUESTS, METRIC_SEARCH_ERRORS,
            METRIC_INDEX_OPERATIONS, METRIC_SEARCH_LATENCY_MS,
            METRIC_INDEX_DOC_COUNT,
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 5);
    }

    #[test]
    fn ops_are_lowercase_snake_case() {
        for op in [OP_SEARCH, OP_INDEX, OP_DELETE, OP_BULK_INDEX] {
            assert!(!op.contains('-'), "op contains hyphen: {op}");
            assert_eq!(op, op.to_ascii_lowercase(), "op not lowercase: {op}");
        }
    }

    #[test]
    fn latency_metric_does_not_end_with_total() {
        assert!(!METRIC_SEARCH_LATENCY_MS.ends_with("_total"));
    }

    #[test]
    fn doc_count_metric_does_not_end_with_total() {
        assert!(!METRIC_INDEX_DOC_COUNT.ends_with("_total"));
    }

    #[test]
    fn op_bulk_index_contains_bulk() {
        assert!(OP_BULK_INDEX.contains("bulk"));
    }
}

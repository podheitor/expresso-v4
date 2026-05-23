//! Chat service Prometheus metrics labels and helpers.

/// Metric namespace for all chat service metrics.
pub const NAMESPACE: &str = "expresso_chat";

/// Label for message-send operations.
pub const OP_MESSAGE_SEND: &str = "message_send";
/// Label for message-list operations.
pub const OP_MESSAGE_LIST: &str = "message_list";
/// Label for channel-create operations.
pub const OP_CHANNEL_CREATE: &str = "channel_create";
/// Label for channel-join operations.
pub const OP_CHANNEL_JOIN: &str = "channel_join";
/// Label for channel-leave operations.
pub const OP_CHANNEL_LEAVE: &str = "channel_leave";
/// Label for Matrix backend errors.
pub const MATRIX_ERROR: &str = "matrix_error";

/// Returns the full Prometheus metric name for a given base name.
pub fn metric_name(base: &str) -> String {
    format!("{}_{}", NAMESPACE, base)
}

/// Returns true when the operation label is recognized.
pub fn is_known_op(op: &str) -> bool {
    matches!(
        op,
        OP_MESSAGE_SEND | OP_MESSAGE_LIST | OP_CHANNEL_CREATE | OP_CHANNEL_JOIN | OP_CHANNEL_LEAVE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_value() {
        assert_eq!(NAMESPACE, "expresso_chat");
    }

    #[test]
    fn op_message_send_value() {
        assert_eq!(OP_MESSAGE_SEND, "message_send");
    }

    #[test]
    fn op_message_list_value() {
        assert_eq!(OP_MESSAGE_LIST, "message_list");
    }

    #[test]
    fn op_channel_create_value() {
        assert_eq!(OP_CHANNEL_CREATE, "channel_create");
    }

    #[test]
    fn op_channel_join_value() {
        assert_eq!(OP_CHANNEL_JOIN, "channel_join");
    }

    #[test]
    fn op_channel_leave_value() {
        assert_eq!(OP_CHANNEL_LEAVE, "channel_leave");
    }

    #[test]
    fn matrix_error_label_value() {
        assert_eq!(MATRIX_ERROR, "matrix_error");
    }

    #[test]
    fn metric_name_prefixes_namespace() {
        assert_eq!(metric_name("requests_total"), "expresso_chat_requests_total");
    }

    #[test]
    fn metric_name_message_count() {
        assert_eq!(metric_name("message_count"), "expresso_chat_message_count");
    }

    #[test]
    fn is_known_op_message_send() {
        assert!(is_known_op(OP_MESSAGE_SEND));
    }

    #[test]
    fn is_known_op_message_list() {
        assert!(is_known_op(OP_MESSAGE_LIST));
    }

    #[test]
    fn is_known_op_channel_create() {
        assert!(is_known_op(OP_CHANNEL_CREATE));
    }

    #[test]
    fn is_known_op_channel_join() {
        assert!(is_known_op(OP_CHANNEL_JOIN));
    }

    #[test]
    fn is_known_op_channel_leave() {
        assert!(is_known_op(OP_CHANNEL_LEAVE));
    }

    #[test]
    fn is_known_op_rejects_unknown() {
        assert!(!is_known_op("delete_message"));
        assert!(!is_known_op(""));
    }

    #[test]
    fn all_op_labels_are_lowercase() {
        for op in [OP_MESSAGE_SEND, OP_MESSAGE_LIST, OP_CHANNEL_CREATE, OP_CHANNEL_JOIN, OP_CHANNEL_LEAVE] {
            assert_eq!(op, op.to_lowercase());
        }
    }

    #[test]
    fn all_op_labels_distinct() {
        let labels = [OP_MESSAGE_SEND, OP_MESSAGE_LIST, OP_CHANNEL_CREATE, OP_CHANNEL_JOIN, OP_CHANNEL_LEAVE];
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j { assert_ne!(a, b); }
            }
        }
    }

    #[test]
    fn metric_name_contains_base() {
        let base = "channel_count";
        assert!(metric_name(base).contains(base));
    }

    #[test]
    fn namespace_is_nonempty() {
        assert!(!NAMESPACE.is_empty());
    }

    #[test]
    fn metric_name_nonempty_for_nonempty_base() {
        assert!(!metric_name("count").is_empty());
    }

    #[test]
    fn is_known_op_case_sensitive() {
        assert!(!is_known_op("MESSAGE_SEND"));
    }

    #[test]
    fn matrix_error_label_is_nonempty() {
        assert!(!MATRIX_ERROR.is_empty());
    }

    #[test]
    fn namespace_contains_no_spaces() {
        assert!(!NAMESPACE.contains(' '));
    }

    #[test]
    fn metric_name_starts_with_namespace() {
        assert!(metric_name("foo").starts_with(NAMESPACE));
    }

    #[test]
    fn matrix_error_label_contains_no_spaces() {
        assert!(!MATRIX_ERROR.contains(' '));
    }

    #[test]
    fn namespace_does_not_contain_hyphen() {
        assert!(!NAMESPACE.contains('-'));
    }
}

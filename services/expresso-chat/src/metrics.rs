//! Chat service Prometheus metrics.
//!
//! Single counter `expresso_chat_ops_total{op, outcome}` covers handler
//! outcomes. `op` is one of: message_send, message_list, message_edit,
//! message_delete, channel_create, channel_join, channel_leave, reaction,
//! pin, attachment_upload. `outcome` is one of: ok, not_member, forbidden,
//! bad_request, not_found, matrix_error, unavailable, error.
//!
//! Cardinality is bounded: handlers always pass one of the canonical `op`
//! labels below, and `outcome_for_err` collapses every error to one label.

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

use crate::error::ChatError;

/// Metric namespace for all chat service metrics.
pub const NAMESPACE: &str = "expresso_chat";

/// Label for message-send operations.
pub const OP_MESSAGE_SEND: &str = "message_send";
/// Label for message-list operations.
pub const OP_MESSAGE_LIST: &str = "message_list";
/// Label for message-edit operations.
pub const OP_MESSAGE_EDIT: &str = "message_edit";
/// Label for message-delete (redaction) operations.
pub const OP_MESSAGE_DELETE: &str = "message_delete";
/// Label for channel-create operations.
pub const OP_CHANNEL_CREATE: &str = "channel_create";
/// Label for channel-join (member add) operations.
pub const OP_CHANNEL_JOIN: &str = "channel_join";
/// Label for channel-leave (archive) operations.
pub const OP_CHANNEL_LEAVE: &str = "channel_leave";
/// Label for reaction add/remove operations.
pub const OP_REACTION: &str = "reaction";
/// Label for pin/unpin operations.
pub const OP_PIN: &str = "pin";
/// Label for attachment-upload operations.
pub const OP_ATTACHMENT_UPLOAD: &str = "attachment_upload";

/// Outcome label for a Matrix-backend failure.
pub const MATRIX_ERROR: &str = "matrix_error";

/// Every canonical `op` label, used to pre-populate series in `init`.
const OPS: &[&str] = &[
    OP_MESSAGE_SEND,
    OP_MESSAGE_LIST,
    OP_MESSAGE_EDIT,
    OP_MESSAGE_DELETE,
    OP_CHANNEL_CREATE,
    OP_CHANNEL_JOIN,
    OP_CHANNEL_LEAVE,
    OP_REACTION,
    OP_PIN,
    OP_ATTACHMENT_UPLOAD,
];

/// Every canonical `outcome` label.
const OUTCOMES: &[&str] = &[
    "ok",
    "not_member",
    "forbidden",
    "bad_request",
    "not_found",
    MATRIX_ERROR,
    "unavailable",
    "error",
];

pub static CHAT_OPS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            metric_name("ops_total"),
            "Chat handler outcomes per operation",
        ),
        &["op", "outcome"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

/// Returns the full Prometheus metric name for a given base name.
pub fn metric_name(base: &str) -> String {
    format!("{NAMESPACE}_{base}")
}

/// Pre-populate label series so Prometheus `rate()` / `increase()` work from
/// the first scrape, even before any request. Idempotent.
pub fn init() {
    Lazy::force(&CHAT_OPS_TOTAL);
    for op in OPS {
        for outcome in OUTCOMES {
            CHAT_OPS_TOTAL.with_label_values(&[op, outcome]).inc_by(0);
        }
    }
}

/// Record one handler outcome.
#[inline]
pub fn record(op: &'static str, outcome: &'static str) {
    CHAT_OPS_TOTAL.with_label_values(&[op, outcome]).inc();
}

/// Map a `ChatError` to the canonical `outcome` label.
pub fn outcome_for_err(e: &ChatError) -> &'static str {
    match e {
        ChatError::NotMember => "not_member",
        ChatError::Forbidden => "forbidden",
        ChatError::BadRequest(_) => "bad_request",
        ChatError::ChannelNotFound(_) => "not_found",
        ChatError::Matrix(_) => MATRIX_ERROR,
        ChatError::DatabaseUnavailable | ChatError::MatrixUnavailable => "unavailable",
        _ => "error",
    }
}

/// Record `ok` on success or the mapped error label on failure, then pass the
/// result through. Lets a handler write `metrics::record_result(OP_X, &r)`.
pub fn record_result<T>(op: &'static str, result: &Result<T, ChatError>) {
    match result {
        Ok(_) => record(op, "ok"),
        Err(e) => record(op, outcome_for_err(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

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
        assert_eq!(
            metric_name("requests_total"),
            "expresso_chat_requests_total"
        );
    }

    #[test]
    fn metric_name_message_count() {
        assert_eq!(metric_name("message_count"), "expresso_chat_message_count");
    }

    #[test]
    fn all_op_labels_distinct() {
        for (i, a) in OPS.iter().enumerate() {
            for (j, b) in OPS.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn all_op_labels_lowercase_no_spaces() {
        for op in OPS {
            assert_eq!(*op, op.to_lowercase());
            assert!(!op.contains(' '));
        }
    }

    #[test]
    fn outcomes_distinct_and_nonempty() {
        for (i, a) in OUTCOMES.iter().enumerate() {
            assert!(!a.is_empty());
            for (j, b) in OUTCOMES.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn outcome_for_err_maps_each_variant() {
        assert_eq!(outcome_for_err(&ChatError::NotMember), "not_member");
        assert_eq!(outcome_for_err(&ChatError::Forbidden), "forbidden");
        assert_eq!(
            outcome_for_err(&ChatError::BadRequest("x".into())),
            "bad_request"
        );
        assert_eq!(
            outcome_for_err(&ChatError::ChannelNotFound(Uuid::nil())),
            "not_found"
        );
        assert_eq!(
            outcome_for_err(&ChatError::Matrix("x".into())),
            "matrix_error"
        );
        assert_eq!(
            outcome_for_err(&ChatError::DatabaseUnavailable),
            "unavailable"
        );
        assert_eq!(
            outcome_for_err(&ChatError::MatrixUnavailable),
            "unavailable"
        );
    }

    #[test]
    fn record_result_ok_and_err_increment() {
        // Exercises the increment path on the real registry (no panic, idempotent).
        let ok: Result<(), ChatError> = Ok(());
        record_result(OP_MESSAGE_SEND, &ok);
        let err: Result<(), ChatError> = Err(ChatError::NotMember);
        record_result(OP_MESSAGE_SEND, &err);
    }

    #[test]
    fn init_is_idempotent() {
        init();
        init();
    }

    #[test]
    fn namespace_is_nonempty_no_hyphen_no_space() {
        assert!(!NAMESPACE.is_empty());
        assert!(!NAMESPACE.contains('-'));
        assert!(!NAMESPACE.contains(' '));
    }

    #[test]
    fn metric_name_starts_with_namespace_and_contains_base() {
        let n = metric_name("channel_count");
        assert!(n.starts_with(NAMESPACE));
        assert!(n.contains("channel_count"));
    }

    #[test]
    fn ops_and_outcomes_counts() {
        assert_eq!(OPS.len(), 10);
        assert_eq!(OUTCOMES.len(), 8);
    }
}

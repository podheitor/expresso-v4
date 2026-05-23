//! Calendar service Prometheus metrics labels and helpers.

/// Metric namespace for all calendar service metrics.
pub const NAMESPACE: &str = "expresso_calendar";

/// Label value for event-create operations.
pub const OP_EVENT_CREATE: &str = "event_create";
/// Label value for event-update operations.
pub const OP_EVENT_UPDATE: &str = "event_update";
/// Label value for event-delete operations.
pub const OP_EVENT_DELETE: &str = "event_delete";
/// Label value for calendar-list operations.
pub const OP_CALENDAR_LIST: &str = "calendar_list";
/// Label value for sharing operations.
pub const OP_SHARE: &str = "share";
/// Label value for iCal export operations.
pub const OP_EXPORT_ICAL: &str = "export_ical";

/// Returns the full Prometheus metric name for a given base name.
pub fn metric_name(base: &str) -> String {
    format!("{}_{}", NAMESPACE, base)
}

/// Returns true when the operation label is known.
pub fn is_known_op(op: &str) -> bool {
    matches!(
        op,
        OP_EVENT_CREATE | OP_EVENT_UPDATE | OP_EVENT_DELETE | OP_CALENDAR_LIST | OP_SHARE | OP_EXPORT_ICAL
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_value() {
        assert_eq!(NAMESPACE, "expresso_calendar");
    }

    #[test]
    fn op_event_create_value() {
        assert_eq!(OP_EVENT_CREATE, "event_create");
    }

    #[test]
    fn op_event_update_value() {
        assert_eq!(OP_EVENT_UPDATE, "event_update");
    }

    #[test]
    fn op_event_delete_value() {
        assert_eq!(OP_EVENT_DELETE, "event_delete");
    }

    #[test]
    fn op_calendar_list_value() {
        assert_eq!(OP_CALENDAR_LIST, "calendar_list");
    }

    #[test]
    fn op_share_value() {
        assert_eq!(OP_SHARE, "share");
    }

    #[test]
    fn op_export_ical_value() {
        assert_eq!(OP_EXPORT_ICAL, "export_ical");
    }

    #[test]
    fn metric_name_prefixes_namespace() {
        assert_eq!(metric_name("requests_total"), "expresso_calendar_requests_total");
    }

    #[test]
    fn metric_name_event_count() {
        assert_eq!(metric_name("event_count"), "expresso_calendar_event_count");
    }

    #[test]
    fn is_known_op_event_create() {
        assert!(is_known_op(OP_EVENT_CREATE));
    }

    #[test]
    fn is_known_op_event_update() {
        assert!(is_known_op(OP_EVENT_UPDATE));
    }

    #[test]
    fn is_known_op_event_delete() {
        assert!(is_known_op(OP_EVENT_DELETE));
    }

    #[test]
    fn is_known_op_calendar_list() {
        assert!(is_known_op(OP_CALENDAR_LIST));
    }

    #[test]
    fn is_known_op_share() {
        assert!(is_known_op(OP_SHARE));
    }

    #[test]
    fn is_known_op_export_ical() {
        assert!(is_known_op(OP_EXPORT_ICAL));
    }

    #[test]
    fn is_known_op_rejects_unknown() {
        assert!(!is_known_op("unknown_op"));
        assert!(!is_known_op(""));
    }

    #[test]
    fn all_op_labels_are_lowercase() {
        for op in [OP_EVENT_CREATE, OP_EVENT_UPDATE, OP_EVENT_DELETE, OP_CALENDAR_LIST, OP_SHARE, OP_EXPORT_ICAL] {
            assert_eq!(op, op.to_lowercase());
        }
    }

    #[test]
    fn all_op_labels_are_distinct() {
        let labels = [OP_EVENT_CREATE, OP_EVENT_UPDATE, OP_EVENT_DELETE, OP_CALENDAR_LIST, OP_SHARE, OP_EXPORT_ICAL];
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn metric_name_contains_base() {
        let base = "latency_seconds";
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
    fn metric_name_uses_underscore_separator() {
        let result = metric_name("foo");
        assert!(result.contains('_'));
    }

    #[test]
    fn is_known_op_case_sensitive() {
        assert!(!is_known_op("EVENT_CREATE"));
        assert!(!is_known_op("Event_Create"));
    }

    #[test]
    fn namespace_contains_no_spaces() {
        assert!(!NAMESPACE.contains(' '));
    }
}

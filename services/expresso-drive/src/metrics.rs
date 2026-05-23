//! Drive service Prometheus metrics labels and helpers.

/// Metric namespace for all drive service metrics.
pub const NAMESPACE: &str = "expresso_drive";

/// Label for file-upload operations.
pub const OP_FILE_UPLOAD: &str = "file_upload";
/// Label for file-download operations.
pub const OP_FILE_DOWNLOAD: &str = "file_download";
/// Label for file-delete operations.
pub const OP_FILE_DELETE: &str = "file_delete";
/// Label for file-list operations.
pub const OP_FILE_LIST: &str = "file_list";
/// Label for share-create operations.
pub const OP_SHARE_CREATE: &str = "share_create";
/// Label for share-revoke operations.
pub const OP_SHARE_REVOKE: &str = "share_revoke";
/// Label for folder-create (mkdir) operations.
pub const OP_MKDIR: &str = "mkdir";
/// Label for quota-exceeded events.
pub const EVENT_QUOTA_EXCEEDED: &str = "quota_exceeded";

/// Returns the full Prometheus metric name for a given base name.
pub fn metric_name(base: &str) -> String {
    format!("{}_{}", NAMESPACE, base)
}

/// Returns true when the operation label is recognized.
pub fn is_known_op(op: &str) -> bool {
    matches!(
        op,
        OP_FILE_UPLOAD | OP_FILE_DOWNLOAD | OP_FILE_DELETE
            | OP_FILE_LIST | OP_SHARE_CREATE | OP_SHARE_REVOKE | OP_MKDIR
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_value() {
        assert_eq!(NAMESPACE, "expresso_drive");
    }

    #[test]
    fn op_file_upload_value() {
        assert_eq!(OP_FILE_UPLOAD, "file_upload");
    }

    #[test]
    fn op_file_download_value() {
        assert_eq!(OP_FILE_DOWNLOAD, "file_download");
    }

    #[test]
    fn op_file_delete_value() {
        assert_eq!(OP_FILE_DELETE, "file_delete");
    }

    #[test]
    fn op_file_list_value() {
        assert_eq!(OP_FILE_LIST, "file_list");
    }

    #[test]
    fn op_share_create_value() {
        assert_eq!(OP_SHARE_CREATE, "share_create");
    }

    #[test]
    fn op_share_revoke_value() {
        assert_eq!(OP_SHARE_REVOKE, "share_revoke");
    }

    #[test]
    fn op_mkdir_value() {
        assert_eq!(OP_MKDIR, "mkdir");
    }

    #[test]
    fn event_quota_exceeded_value() {
        assert_eq!(EVENT_QUOTA_EXCEEDED, "quota_exceeded");
    }

    #[test]
    fn metric_name_prefixes_namespace() {
        assert_eq!(metric_name("requests_total"), "expresso_drive_requests_total");
    }

    #[test]
    fn metric_name_upload_bytes() {
        assert_eq!(metric_name("upload_bytes"), "expresso_drive_upload_bytes");
    }

    #[test]
    fn is_known_op_file_upload() {
        assert!(is_known_op(OP_FILE_UPLOAD));
    }

    #[test]
    fn is_known_op_file_download() {
        assert!(is_known_op(OP_FILE_DOWNLOAD));
    }

    #[test]
    fn is_known_op_file_delete() {
        assert!(is_known_op(OP_FILE_DELETE));
    }

    #[test]
    fn is_known_op_file_list() {
        assert!(is_known_op(OP_FILE_LIST));
    }

    #[test]
    fn is_known_op_share_create() {
        assert!(is_known_op(OP_SHARE_CREATE));
    }

    #[test]
    fn is_known_op_share_revoke() {
        assert!(is_known_op(OP_SHARE_REVOKE));
    }

    #[test]
    fn is_known_op_mkdir() {
        assert!(is_known_op(OP_MKDIR));
    }

    #[test]
    fn is_known_op_rejects_unknown() {
        assert!(!is_known_op("rename"));
        assert!(!is_known_op(""));
    }

    #[test]
    fn all_op_labels_lowercase() {
        for op in [OP_FILE_UPLOAD, OP_FILE_DOWNLOAD, OP_FILE_DELETE,
                   OP_FILE_LIST, OP_SHARE_CREATE, OP_SHARE_REVOKE, OP_MKDIR] {
            assert_eq!(op, op.to_lowercase());
        }
    }

    #[test]
    fn all_op_labels_distinct() {
        let labels = [OP_FILE_UPLOAD, OP_FILE_DOWNLOAD, OP_FILE_DELETE,
                      OP_FILE_LIST, OP_SHARE_CREATE, OP_SHARE_REVOKE, OP_MKDIR];
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j { assert_ne!(a, b); }
            }
        }
    }

    #[test]
    fn metric_name_contains_base() {
        assert!(metric_name("file_count").contains("file_count"));
    }

    #[test]
    fn namespace_is_nonempty() {
        assert!(!NAMESPACE.is_empty());
    }

    #[test]
    fn is_known_op_case_sensitive_rejects_uppercase() {
        assert!(!is_known_op("FILE_UPLOAD"));
    }

    #[test]
    fn event_quota_exceeded_label_is_nonempty() {
        assert!(!EVENT_QUOTA_EXCEEDED.is_empty());
    }

    #[test]
    fn namespace_does_not_contain_hyphen() {
        assert!(!NAMESPACE.contains('-'));
    }
}

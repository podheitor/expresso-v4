//! Contacts service Prometheus metrics labels and helpers.

/// Metric namespace for all contacts service metrics.
pub const NAMESPACE: &str = "expresso_contacts";

/// Label for contact-create operations.
pub const OP_CONTACT_CREATE: &str = "contact_create";
/// Label for contact-update operations.
pub const OP_CONTACT_UPDATE: &str = "contact_update";
/// Label for contact-delete operations.
pub const OP_CONTACT_DELETE: &str = "contact_delete";
/// Label for contact-list operations.
pub const OP_CONTACT_LIST: &str = "contact_list";
/// Label for vCard import operations.
pub const OP_IMPORT_VCARD: &str = "import_vcard";
/// Label for CSV import operations.
pub const OP_IMPORT_CSV: &str = "import_csv";
/// Label for vCard export operations.
pub const OP_EXPORT_VCF: &str = "export_vcf";

/// Returns the full Prometheus metric name for a given base name.
pub fn metric_name(base: &str) -> String {
    format!("{}_{}", NAMESPACE, base)
}

/// Returns true when the operation label is recognized.
pub fn is_known_op(op: &str) -> bool {
    matches!(
        op,
        OP_CONTACT_CREATE | OP_CONTACT_UPDATE | OP_CONTACT_DELETE
            | OP_CONTACT_LIST | OP_IMPORT_VCARD | OP_IMPORT_CSV | OP_EXPORT_VCF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_value() {
        assert_eq!(NAMESPACE, "expresso_contacts");
    }

    #[test]
    fn op_contact_create_value() {
        assert_eq!(OP_CONTACT_CREATE, "contact_create");
    }

    #[test]
    fn op_contact_update_value() {
        assert_eq!(OP_CONTACT_UPDATE, "contact_update");
    }

    #[test]
    fn op_contact_delete_value() {
        assert_eq!(OP_CONTACT_DELETE, "contact_delete");
    }

    #[test]
    fn op_contact_list_value() {
        assert_eq!(OP_CONTACT_LIST, "contact_list");
    }

    #[test]
    fn op_import_vcard_value() {
        assert_eq!(OP_IMPORT_VCARD, "import_vcard");
    }

    #[test]
    fn op_import_csv_value() {
        assert_eq!(OP_IMPORT_CSV, "import_csv");
    }

    #[test]
    fn op_export_vcf_value() {
        assert_eq!(OP_EXPORT_VCF, "export_vcf");
    }

    #[test]
    fn metric_name_prefixes_namespace() {
        assert_eq!(metric_name("requests_total"), "expresso_contacts_requests_total");
    }

    #[test]
    fn is_known_op_contact_create() {
        assert!(is_known_op(OP_CONTACT_CREATE));
    }

    #[test]
    fn is_known_op_contact_update() {
        assert!(is_known_op(OP_CONTACT_UPDATE));
    }

    #[test]
    fn is_known_op_contact_delete() {
        assert!(is_known_op(OP_CONTACT_DELETE));
    }

    #[test]
    fn is_known_op_contact_list() {
        assert!(is_known_op(OP_CONTACT_LIST));
    }

    #[test]
    fn is_known_op_import_vcard() {
        assert!(is_known_op(OP_IMPORT_VCARD));
    }

    #[test]
    fn is_known_op_import_csv() {
        assert!(is_known_op(OP_IMPORT_CSV));
    }

    #[test]
    fn is_known_op_export_vcf() {
        assert!(is_known_op(OP_EXPORT_VCF));
    }

    #[test]
    fn is_known_op_rejects_unknown() {
        assert!(!is_known_op("search"));
        assert!(!is_known_op(""));
    }

    #[test]
    fn all_op_labels_lowercase() {
        for op in [OP_CONTACT_CREATE, OP_CONTACT_UPDATE, OP_CONTACT_DELETE,
                   OP_CONTACT_LIST, OP_IMPORT_VCARD, OP_IMPORT_CSV, OP_EXPORT_VCF] {
            assert_eq!(op, op.to_lowercase());
        }
    }

    #[test]
    fn all_op_labels_distinct() {
        let labels = [OP_CONTACT_CREATE, OP_CONTACT_UPDATE, OP_CONTACT_DELETE,
                      OP_CONTACT_LIST, OP_IMPORT_VCARD, OP_IMPORT_CSV, OP_EXPORT_VCF];
        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j { assert_ne!(a, b); }
            }
        }
    }

    #[test]
    fn metric_name_contains_base() {
        assert!(metric_name("contact_count").contains("contact_count"));
    }

    #[test]
    fn namespace_is_nonempty() {
        assert!(!NAMESPACE.is_empty());
    }

    #[test]
    fn is_known_op_case_sensitive_rejects_uppercase() {
        assert!(!is_known_op("CONTACT_CREATE"));
    }

    #[test]
    fn namespace_contains_no_spaces() {
        assert!(!NAMESPACE.contains(' '));
    }

    #[test]
    fn metric_name_starts_with_namespace() {
        assert!(metric_name("baz").starts_with(NAMESPACE));
    }

    #[test]
    fn namespace_does_not_contain_hyphen() {
        assert!(!NAMESPACE.contains('-'));
    }
}

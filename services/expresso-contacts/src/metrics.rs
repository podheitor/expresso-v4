//! Contacts service Prometheus metrics.
//!
//! Single counter `expresso_contacts_ops_total{op, outcome}` covers handler
//! outcomes. `op` is one of: contact_create, contact_update, contact_delete,
//! contact_list, import_vcard, import_csv, export_vcf. `outcome` is `ok` on
//! success or a mapped error label (see `outcome_for_err`).

use once_cell::sync::Lazy;
use prometheus::IntCounterVec;

use crate::error::ContactsError;

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

const OPS: &[&str] = &[
    OP_CONTACT_CREATE,
    OP_CONTACT_UPDATE,
    OP_CONTACT_DELETE,
    OP_CONTACT_LIST,
    OP_IMPORT_VCARD,
    OP_IMPORT_CSV,
    OP_EXPORT_VCF,
];

const OUTCOMES: &[&str] = &[
    "ok",
    "not_found",
    "bad_request",
    "forbidden",
    "not_supported",
    "unavailable",
    "error",
];

pub static CONTACTS_OPS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            metric_name("ops_total"),
            "Contacts handler outcomes per operation",
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

/// Pre-populate label series so `rate()`/`increase()` work from the first
/// scrape. Idempotent.
pub fn init() {
    Lazy::force(&CONTACTS_OPS_TOTAL);
    for op in OPS {
        for outcome in OUTCOMES {
            CONTACTS_OPS_TOTAL
                .with_label_values(&[op, outcome])
                .inc_by(0);
        }
    }
}

/// Record one handler outcome.
#[inline]
pub fn record(op: &'static str, outcome: &'static str) {
    CONTACTS_OPS_TOTAL.with_label_values(&[op, outcome]).inc();
}

/// Map a `ContactsError` to the canonical `outcome` label.
pub fn outcome_for_err(e: &ContactsError) -> &'static str {
    match e {
        ContactsError::ContactNotFound(_) | ContactsError::AddressbookNotFound(_) => "not_found",
        ContactsError::InvalidVCard(_) | ContactsError::BadRequest(_) => "bad_request",
        ContactsError::Forbidden => "forbidden",
        ContactsError::NotSupported(_) => "not_supported",
        ContactsError::DatabaseUnavailable => "unavailable",
        _ => "error",
    }
}

/// Record `ok` on success or the mapped error label on failure.
pub fn record_result<T>(op: &'static str, result: &Result<T, ContactsError>) {
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
        assert_eq!(NAMESPACE, "expresso_contacts");
    }

    #[test]
    fn op_values() {
        assert_eq!(OP_CONTACT_CREATE, "contact_create");
        assert_eq!(OP_CONTACT_UPDATE, "contact_update");
        assert_eq!(OP_CONTACT_DELETE, "contact_delete");
        assert_eq!(OP_CONTACT_LIST, "contact_list");
        assert_eq!(OP_IMPORT_VCARD, "import_vcard");
        assert_eq!(OP_IMPORT_CSV, "import_csv");
        assert_eq!(OP_EXPORT_VCF, "export_vcf");
    }

    #[test]
    fn metric_name_prefixes_namespace() {
        assert_eq!(
            metric_name("requests_total"),
            "expresso_contacts_requests_total"
        );
        assert!(metric_name("x").starts_with(NAMESPACE));
        assert_eq!(metric_name("latency"), format!("{NAMESPACE}_latency"));
    }

    #[test]
    fn op_labels_distinct_lowercase() {
        for (i, a) in OPS.iter().enumerate() {
            assert_eq!(*a, a.to_lowercase());
            assert!(!a.contains(' '));
            for (j, b) in OPS.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn outcomes_distinct_nonempty() {
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
    fn outcome_for_err_maps_variants() {
        assert_eq!(
            outcome_for_err(&ContactsError::ContactNotFound(Uuid::nil())),
            "not_found"
        );
        assert_eq!(
            outcome_for_err(&ContactsError::AddressbookNotFound("a".into())),
            "not_found"
        );
        assert_eq!(
            outcome_for_err(&ContactsError::InvalidVCard("x".into())),
            "bad_request"
        );
        assert_eq!(
            outcome_for_err(&ContactsError::BadRequest("x".into())),
            "bad_request"
        );
        assert_eq!(outcome_for_err(&ContactsError::Forbidden), "forbidden");
        assert_eq!(
            outcome_for_err(&ContactsError::NotSupported("x")),
            "not_supported"
        );
        assert_eq!(
            outcome_for_err(&ContactsError::DatabaseUnavailable),
            "unavailable"
        );
    }

    #[test]
    fn record_paths_do_not_panic() {
        init();
        init();
        record(OP_CONTACT_CREATE, "ok");
        let ok: Result<(), ContactsError> = Ok(());
        record_result(OP_CONTACT_CREATE, &ok);
        let err: Result<(), ContactsError> = Err(ContactsError::Forbidden);
        record_result(OP_CONTACT_DELETE, &err);
    }

    #[test]
    fn ops_and_outcomes_counts() {
        assert_eq!(OPS.len(), 7);
        assert_eq!(OUTCOMES.len(), 7);
    }
}

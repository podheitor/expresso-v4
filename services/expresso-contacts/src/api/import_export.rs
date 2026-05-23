//! vCard import / export for address books.
//!
//! POST /api/v1/contacts/addressbooks/:id/import  — import vCard file (vCard 3/4)
//! GET  /api/v1/contacts/addressbooks/:id/export  — export all contacts as vCard

use serde::{Deserialize, Serialize};

pub const MAX_IMPORT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VCardVersion {
    V3,
    V4,
}

impl VCardVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V3 => "3.0",
            Self::V4 => "4.0",
        }
    }
}

impl std::fmt::Display for VCardVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped:  usize,
    pub errors:   Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub version: Option<VCardVersion>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_import_bytes_constant() {
        assert_eq!(MAX_IMPORT_BYTES, 10 * 1024 * 1024);
    }

    #[test]
    fn vcard_v3_as_str() {
        assert_eq!(VCardVersion::V3.as_str(), "3.0");
    }

    #[test]
    fn vcard_v4_as_str() {
        assert_eq!(VCardVersion::V4.as_str(), "4.0");
    }

    #[test]
    fn vcard_display_v3() {
        assert_eq!(format!("{}", VCardVersion::V3), "3.0");
    }

    #[test]
    fn vcard_display_v4() {
        assert_eq!(format!("{}", VCardVersion::V4), "4.0");
    }

    #[test]
    fn vcard_equality() {
        assert_eq!(VCardVersion::V3, VCardVersion::V3);
        assert_ne!(VCardVersion::V3, VCardVersion::V4);
    }

    #[test]
    fn vcard_serde_roundtrip_v3() {
        let s = serde_json::to_string(&VCardVersion::V3).unwrap();
        let back: VCardVersion = serde_json::from_str(&s).unwrap();
        assert_eq!(back, VCardVersion::V3);
    }

    #[test]
    fn vcard_serde_roundtrip_v4() {
        let s = serde_json::to_string(&VCardVersion::V4).unwrap();
        let back: VCardVersion = serde_json::from_str(&s).unwrap();
        assert_eq!(back, VCardVersion::V4);
    }

    #[test]
    fn import_result_serde_roundtrip() {
        let r = ImportResult { imported: 5, skipped: 1, errors: vec!["bad line".into()] };
        let s = serde_json::to_string(&r).unwrap();
        let back: ImportResult = serde_json::from_str(&s).unwrap();
        assert_eq!(back.imported, 5);
        assert_eq!(back.skipped, 1);
        assert_eq!(back.errors.len(), 1);
    }

    #[test]
    fn import_result_no_errors() {
        let r = ImportResult { imported: 10, skipped: 0, errors: vec![] };
        assert!(r.errors.is_empty());
    }

    #[test]
    fn import_result_clone_preserves_counts() {
        let r = ImportResult { imported: 3, skipped: 2, errors: vec![] };
        let c = r.clone();
        assert_eq!(c.imported, 3);
        assert_eq!(c.skipped, 2);
    }

    #[test]
    fn export_query_version_optional() {
        let q: ExportQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.version.is_none());
    }

    #[test]
    fn export_query_version_v4() {
        let q: ExportQuery = serde_json::from_str(r#"{"version":"v4"}"#).unwrap();
        assert_eq!(q.version, Some(VCardVersion::V4));
    }

    #[test]
    fn vcard_copy_trait() {
        let v = VCardVersion::V4;
        let v2 = v;
        assert_eq!(v, v2);
    }

    #[test]
    fn max_import_bytes_is_ten_mib() {
        assert_eq!(MAX_IMPORT_BYTES, 10_485_760);
    }

    #[test]
    fn import_result_json_contains_imported_key() {
        let r = ImportResult { imported: 0, skipped: 0, errors: vec![] };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("imported"));
    }

    #[test]
    fn import_result_json_contains_skipped_key() {
        let r = ImportResult { imported: 0, skipped: 0, errors: vec![] };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("skipped"));
    }

    #[test]
    fn vcard_debug_contains_variant() {
        let s = format!("{:?}", VCardVersion::V3);
        assert!(s.contains("V3"));
    }

    #[test]
    fn import_result_errors_preserved() {
        let r = ImportResult {
            imported: 2, skipped: 1,
            errors: vec!["err1".into(), "err2".into()],
        };
        assert_eq!(r.errors.len(), 2);
    }

    #[test]
    fn max_import_bytes_positive() {
        assert!(MAX_IMPORT_BYTES > 0);
    }

    #[test]
    fn export_query_version_v3() {
        let q: ExportQuery = serde_json::from_str(r#"{"version":"v3"}"#).unwrap();
        assert_eq!(q.version, Some(VCardVersion::V3));
    }

    #[test]
    fn import_result_imported_zero() {
        let r = ImportResult { imported: 0, skipped: 0, errors: vec![] };
        assert_eq!(r.imported, 0);
    }

    #[test]
    fn vcard_version_debug_v4() {
        let s = format!("{:?}", VCardVersion::V4);
        assert!(s.contains("V4"));
    }

    #[test]
    fn export_query_version_none_by_default() {
        let q: ExportQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.version.is_none());
    }

    #[test]
    fn import_result_skipped_field_accessible() {
        let r = ImportResult { imported: 7, skipped: 3, errors: vec![] };
        assert_eq!(r.skipped, 3);
    }
}

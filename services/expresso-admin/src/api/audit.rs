//! Admin audit-log query endpoint.
//!
//! GET /api/admin/audit — SuperAdmin only. Wraps the shared audit module.

use serde::{Deserialize, Serialize};

pub const DEFAULT_AUDIT_LIMIT: i64 = 50;
pub const MAX_AUDIT_LIMIT:     i64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditPreset {
    H24,
    D7,
    D30,
    All,
}

impl AuditPreset {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::H24 => "24h",
            Self::D7  => "7d",
            Self::D30 => "30d",
            Self::All => "all",
        }
    }
}

impl std::fmt::Display for AuditPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditSummary {
    pub total:      i64,
    pub preset:     Option<String>,
    pub has_more:   bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_constant() {
        assert_eq!(DEFAULT_AUDIT_LIMIT, 50);
    }

    #[test]
    fn max_limit_constant() {
        assert_eq!(MAX_AUDIT_LIMIT, 500);
    }

    #[test]
    fn default_less_than_max() {
        assert!(DEFAULT_AUDIT_LIMIT < MAX_AUDIT_LIMIT);
    }

    #[test]
    fn preset_h24_as_str() {
        assert_eq!(AuditPreset::H24.as_str(), "24h");
    }

    #[test]
    fn preset_d7_as_str() {
        assert_eq!(AuditPreset::D7.as_str(), "7d");
    }

    #[test]
    fn preset_d30_as_str() {
        assert_eq!(AuditPreset::D30.as_str(), "30d");
    }

    #[test]
    fn preset_all_as_str() {
        assert_eq!(AuditPreset::All.as_str(), "all");
    }

    #[test]
    fn preset_display_h24() {
        assert_eq!(format!("{}", AuditPreset::H24), "24h");
    }

    #[test]
    fn preset_display_all() {
        assert_eq!(format!("{}", AuditPreset::All), "all");
    }

    #[test]
    fn preset_equality() {
        assert_eq!(AuditPreset::D7, AuditPreset::D7);
        assert_ne!(AuditPreset::D7, AuditPreset::D30);
    }

    #[test]
    fn preset_serde_roundtrip_h24() {
        let s = serde_json::to_string(&AuditPreset::H24).unwrap();
        let back: AuditPreset = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AuditPreset::H24);
    }

    #[test]
    fn preset_serde_roundtrip_all() {
        let s = serde_json::to_string(&AuditPreset::All).unwrap();
        let back: AuditPreset = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AuditPreset::All);
    }

    #[test]
    fn audit_summary_serde_roundtrip() {
        let s = AuditSummary { total: 42, preset: Some("7d".into()), has_more: true };
        let json = serde_json::to_string(&s).unwrap();
        let back: AuditSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total, 42);
        assert!(back.has_more);
    }

    #[test]
    fn audit_summary_preset_optional() {
        let s = AuditSummary { total: 0, preset: None, has_more: false };
        assert!(s.preset.is_none());
    }

    #[test]
    fn audit_summary_clone_preserves_total() {
        let s = AuditSummary { total: 100, preset: None, has_more: false };
        assert_eq!(s.clone().total, 100);
    }

    #[test]
    fn preset_copy_trait() {
        let p = AuditPreset::D30;
        let p2 = p;
        assert_eq!(p, p2);
    }

    #[test]
    fn max_limit_positive() {
        assert!(MAX_AUDIT_LIMIT > 0);
    }

    #[test]
    fn audit_summary_has_more_false() {
        let s = AuditSummary { total: 5, preset: None, has_more: false };
        assert!(!s.has_more);
    }

    #[test]
    fn preset_debug_contains_variant() {
        let s = format!("{:?}", AuditPreset::D7);
        assert!(s.contains("D7"));
    }

    #[test]
    fn audit_summary_json_contains_total_key() {
        let s = AuditSummary { total: 1, preset: None, has_more: false };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("total"));
    }

    #[test]
    fn audit_summary_json_contains_has_more_key() {
        let s = AuditSummary { total: 0, preset: None, has_more: true };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("has_more"));
    }

    #[test]
    fn default_limit_positive() {
        assert!(DEFAULT_AUDIT_LIMIT > 0);
    }

    #[test]
    fn preset_serde_roundtrip_d7() {
        let s = serde_json::to_string(&AuditPreset::D7).unwrap();
        let back: AuditPreset = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AuditPreset::D7);
    }

    #[test]
    fn preset_display_d30() {
        assert_eq!(format!("{}", AuditPreset::D30), "30d");
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterField {
    Kind,
    FromAddr,
    Subject,
    ToAddr,
    HasAttachment,
}

impl std::fmt::Display for FilterField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilterField::Kind          => write!(f, "kind"),
            FilterField::FromAddr      => write!(f, "from_addr"),
            FilterField::Subject       => write!(f, "subject"),
            FilterField::ToAddr        => write!(f, "to_addr"),
            FilterField::HasAttachment => write!(f, "has_attachment"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DateRange {
    pub after_secs:  Option<u64>,
    pub before_secs: Option<u64>,
}

impl DateRange {
    pub fn is_bounded(&self) -> bool {
        self.after_secs.is_some() || self.before_secs.is_some()
    }

    pub fn is_valid(&self) -> bool {
        match (self.after_secs, self.before_secs) {
            (Some(a), Some(b)) => a < b,
            _ => true,
        }
    }

    pub fn span_secs(&self) -> Option<u64> {
        match (self.after_secs, self.before_secs) {
            (Some(a), Some(b)) if b > a => Some(b - a),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFilters {
    pub kind:           Option<String>,
    pub from_addr:      Option<String>,
    pub has_attachment: Option<bool>,
    pub date_range:     Option<DateRange>,
}

impl SearchFilters {
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.from_addr.is_none()
            && self.has_attachment.is_none()
            && self.date_range.as_ref().map_or(true, |d| !d.is_bounded())
    }
}

pub fn parse_bool_param(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_field_display_kind() {
        assert_eq!(FilterField::Kind.to_string(), "kind");
    }

    #[test]
    fn filter_field_display_from_addr() {
        assert_eq!(FilterField::FromAddr.to_string(), "from_addr");
    }

    #[test]
    fn filter_field_display_subject() {
        assert_eq!(FilterField::Subject.to_string(), "subject");
    }

    #[test]
    fn filter_field_display_to_addr() {
        assert_eq!(FilterField::ToAddr.to_string(), "to_addr");
    }

    #[test]
    fn filter_field_display_has_attachment() {
        assert_eq!(FilterField::HasAttachment.to_string(), "has_attachment");
    }

    #[test]
    fn date_range_unbounded_is_not_bounded() {
        let dr = DateRange { after_secs: None, before_secs: None };
        assert!(!dr.is_bounded());
    }

    #[test]
    fn date_range_after_only_is_bounded() {
        let dr = DateRange { after_secs: Some(100), before_secs: None };
        assert!(dr.is_bounded());
    }

    #[test]
    fn date_range_before_only_is_bounded() {
        let dr = DateRange { after_secs: None, before_secs: Some(200) };
        assert!(dr.is_bounded());
    }

    #[test]
    fn date_range_valid_when_after_lt_before() {
        let dr = DateRange { after_secs: Some(100), before_secs: Some(200) };
        assert!(dr.is_valid());
    }

    #[test]
    fn date_range_invalid_when_after_ge_before() {
        let dr = DateRange { after_secs: Some(300), before_secs: Some(200) };
        assert!(!dr.is_valid());
    }

    #[test]
    fn date_range_span_secs_computed() {
        let dr = DateRange { after_secs: Some(100), before_secs: Some(300) };
        assert_eq!(dr.span_secs(), Some(200));
    }

    #[test]
    fn date_range_span_secs_none_when_unbounded() {
        let dr = DateRange { after_secs: None, before_secs: Some(300) };
        assert_eq!(dr.span_secs(), None);
    }

    #[test]
    fn search_filters_empty_by_default() {
        let f = SearchFilters { kind: None, from_addr: None, has_attachment: None, date_range: None };
        assert!(f.is_empty());
    }

    #[test]
    fn search_filters_not_empty_with_kind() {
        let f = SearchFilters { kind: Some("email".into()), from_addr: None, has_attachment: None, date_range: None };
        assert!(!f.is_empty());
    }

    #[test]
    fn search_filters_not_empty_with_from_addr() {
        let f = SearchFilters { kind: None, from_addr: Some("a@b.com".into()), has_attachment: None, date_range: None };
        assert!(!f.is_empty());
    }

    #[test]
    fn parse_bool_true_variants() {
        assert_eq!(parse_bool_param("true"), Some(true));
        assert_eq!(parse_bool_param("1"), Some(true));
        assert_eq!(parse_bool_param("yes"), Some(true));
    }

    #[test]
    fn parse_bool_false_variants() {
        assert_eq!(parse_bool_param("false"), Some(false));
        assert_eq!(parse_bool_param("0"), Some(false));
        assert_eq!(parse_bool_param("no"), Some(false));
    }

    #[test]
    fn parse_bool_unknown_returns_none() {
        assert_eq!(parse_bool_param("maybe"), None);
    }

    #[test]
    fn parse_bool_case_insensitive() {
        assert_eq!(parse_bool_param("TRUE"), Some(true));
        assert_eq!(parse_bool_param("FALSE"), Some(false));
    }

    #[test]
    fn filter_field_equality() {
        assert_eq!(FilterField::Kind, FilterField::Kind);
        assert_ne!(FilterField::Kind, FilterField::FromAddr);
    }

    #[test]
    fn serde_roundtrip_filter_field() {
        let field = FilterField::HasAttachment;
        let json = serde_json::to_string(&field).unwrap();
        let back: FilterField = serde_json::from_str(&json).unwrap();
        assert_eq!(back, FilterField::HasAttachment);
    }

    #[test]
    fn date_range_equal_after_before_is_invalid() {
        let dr = DateRange { after_secs: Some(100), before_secs: Some(100) };
        assert!(!dr.is_valid());
    }

    #[test]
    fn search_filters_not_empty_with_has_attachment() {
        let f = SearchFilters { kind: None, from_addr: None, has_attachment: Some(true), date_range: None };
        assert!(!f.is_empty());
    }

    #[test]
    fn parse_bool_empty_string_returns_none() {
        assert_eq!(parse_bool_param(""), None);
    }
}

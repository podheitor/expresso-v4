use serde::{Deserialize, Serialize};

pub const BM25_K1: f32 = 1.2;
pub const BM25_B:  f32 = 0.75;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrder {
    Relevance,
    DateDesc,
    DateAsc,
    SenderAsc,
    SenderDesc,
}

impl std::fmt::Display for SortOrder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SortOrder::Relevance  => write!(f, "relevance"),
            SortOrder::DateDesc   => write!(f, "date_desc"),
            SortOrder::DateAsc    => write!(f, "date_asc"),
            SortOrder::SenderAsc  => write!(f, "sender_asc"),
            SortOrder::SenderDesc => write!(f, "sender_desc"),
        }
    }
}

impl SortOrder {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "relevance"   => Some(SortOrder::Relevance),
            "date_desc"   => Some(SortOrder::DateDesc),
            "date_asc"    => Some(SortOrder::DateAsc),
            "sender_asc"  => Some(SortOrder::SenderAsc),
            "sender_desc" => Some(SortOrder::SenderDesc),
            _             => None,
        }
    }

    pub fn is_date_based(&self) -> bool {
        matches!(self, SortOrder::DateDesc | SortOrder::DateAsc)
    }
}

pub fn bm25_score(tf: f32, df: u32, n_docs: u32, field_len: u32, avg_field_len: f32) -> f32 {
    if n_docs == 0 || df == 0 {
        return 0.0;
    }
    let idf = ((n_docs as f32 - df as f32 + 0.5) / (df as f32 + 0.5) + 1.0).ln();
    let norm = (tf * (BM25_K1 + 1.0))
        / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * field_len as f32 / avg_field_len));
    idf * norm
}

pub fn boost_recent(base_score: f32, age_secs: u64, half_life_secs: u64) -> f32 {
    if half_life_secs == 0 {
        return base_score;
    }
    let decay = 2.0_f32.powf(-(age_secs as f32 / half_life_secs as f32));
    base_score * (1.0 + decay)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bm25_k1_constant() {
        assert!((BM25_K1 - 1.2).abs() < 1e-6);
    }

    #[test]
    fn bm25_b_constant() {
        assert!((BM25_B - 0.75).abs() < 1e-6);
    }

    #[test]
    fn sort_order_display_relevance() {
        assert_eq!(SortOrder::Relevance.to_string(), "relevance");
    }

    #[test]
    fn sort_order_display_date_desc() {
        assert_eq!(SortOrder::DateDesc.to_string(), "date_desc");
    }

    #[test]
    fn sort_order_display_date_asc() {
        assert_eq!(SortOrder::DateAsc.to_string(), "date_asc");
    }

    #[test]
    fn sort_order_display_sender_asc() {
        assert_eq!(SortOrder::SenderAsc.to_string(), "sender_asc");
    }

    #[test]
    fn sort_order_display_sender_desc() {
        assert_eq!(SortOrder::SenderDesc.to_string(), "sender_desc");
    }

    #[test]
    fn from_str_relevance() {
        assert_eq!(SortOrder::from_str("relevance"), Some(SortOrder::Relevance));
    }

    #[test]
    fn from_str_date_desc() {
        assert_eq!(SortOrder::from_str("date_desc"), Some(SortOrder::DateDesc));
    }

    #[test]
    fn from_str_date_asc_uppercase() {
        assert_eq!(SortOrder::from_str("DATE_ASC"), Some(SortOrder::DateAsc));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(SortOrder::from_str("popularity"), None);
    }

    #[test]
    fn date_desc_is_date_based() {
        assert!(SortOrder::DateDesc.is_date_based());
    }

    #[test]
    fn relevance_is_not_date_based() {
        assert!(!SortOrder::Relevance.is_date_based());
    }

    #[test]
    fn date_asc_is_date_based() {
        assert!(SortOrder::DateAsc.is_date_based());
    }

    #[test]
    fn bm25_zero_docs_returns_zero() {
        assert_eq!(bm25_score(1.0, 1, 0, 10, 10.0), 0.0);
    }

    #[test]
    fn bm25_zero_df_returns_zero() {
        assert_eq!(bm25_score(1.0, 0, 100, 10, 10.0), 0.0);
    }

    #[test]
    fn bm25_positive_score_for_valid_inputs() {
        let score = bm25_score(2.0, 5, 1000, 10, 15.0);
        assert!(score > 0.0);
    }

    #[test]
    fn boost_recent_decays_with_age() {
        let s0 = boost_recent(1.0, 0, 86400);
        let s1 = boost_recent(1.0, 86400, 86400);
        assert!(s0 > s1);
    }

    #[test]
    fn boost_recent_zero_half_life_returns_base() {
        assert!((boost_recent(2.5, 1000, 0) - 2.5).abs() < 1e-6);
    }

    #[test]
    fn boost_recent_at_zero_age_doubles() {
        let boosted = boost_recent(1.0, 0, 86400);
        assert!((boosted - 2.0).abs() < 1e-6);
    }

    #[test]
    fn sort_order_equality() {
        assert_eq!(SortOrder::Relevance, SortOrder::Relevance);
        assert_ne!(SortOrder::Relevance, SortOrder::DateDesc);
    }

    #[test]
    fn serde_roundtrip_sort_order() {
        let order = SortOrder::DateDesc;
        let json = serde_json::to_string(&order).unwrap();
        let back: SortOrder = serde_json::from_str(&json).unwrap();
        assert_eq!(back, SortOrder::DateDesc);
    }

    #[test]
    fn from_str_sender_asc() {
        assert_eq!(SortOrder::from_str("sender_asc"), Some(SortOrder::SenderAsc));
    }

    #[test]
    fn from_str_sender_desc() {
        assert_eq!(SortOrder::from_str("sender_desc"), Some(SortOrder::SenderDesc));
    }

    #[test]
    fn sender_asc_is_not_date_based() {
        assert!(!SortOrder::SenderAsc.is_date_based());
    }

    #[test]
    fn sender_desc_is_not_date_based() {
        assert!(!SortOrder::SenderDesc.is_date_based());
    }
}

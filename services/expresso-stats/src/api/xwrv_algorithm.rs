//! XWRV weighted-relevance algorithm helpers — sprint #19857.
//!
//! Computes a relevance score for analytics items using weighted signals:
//!   score = w_recency * recency + w_frequency * frequency + w_volume * volume

use serde::{Deserialize, Serialize};

pub const DEFAULT_W_RECENCY:   f64 = 0.5;
pub const DEFAULT_W_FREQUENCY: f64 = 0.3;
pub const DEFAULT_W_VOLUME:    f64 = 0.2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XwrvWeights {
    pub w_recency:   f64,
    pub w_frequency: f64,
    pub w_volume:    f64,
}

impl Default for XwrvWeights {
    fn default() -> Self {
        Self {
            w_recency:   DEFAULT_W_RECENCY,
            w_frequency: DEFAULT_W_FREQUENCY,
            w_volume:    DEFAULT_W_VOLUME,
        }
    }
}

impl XwrvWeights {
    pub fn sum(&self) -> f64 {
        self.w_recency + self.w_frequency + self.w_volume
    }

    pub fn is_valid(&self) -> bool {
        let s = self.sum();
        (s - 1.0_f64).abs() < 1e-9
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XwrvInput {
    pub recency:   f64,
    pub frequency: f64,
    pub volume:    f64,
}

pub fn compute_score(input: &XwrvInput, weights: &XwrvWeights) -> f64 {
    weights.w_recency   * input.recency
        + weights.w_frequency * input.frequency
        + weights.w_volume    * input.volume
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_weights_sum_to_one() {
        let w = XwrvWeights::default();
        assert!((w.sum() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn default_weights_is_valid() {
        assert!(XwrvWeights::default().is_valid());
    }

    #[test]
    fn default_w_recency_constant() {
        assert!((DEFAULT_W_RECENCY - 0.5).abs() < 1e-9);
    }

    #[test]
    fn default_w_frequency_constant() {
        assert!((DEFAULT_W_FREQUENCY - 0.3).abs() < 1e-9);
    }

    #[test]
    fn default_w_volume_constant() {
        assert!((DEFAULT_W_VOLUME - 0.2).abs() < 1e-9);
    }

    #[test]
    fn compute_score_uniform_weights_equals_mean() {
        let w = XwrvWeights { w_recency: 1.0 / 3.0, w_frequency: 1.0 / 3.0, w_volume: 1.0 / 3.0 };
        let i = XwrvInput { recency: 0.6, frequency: 0.9, volume: 0.3 };
        let score = compute_score(&i, &w);
        let expected = (0.6 + 0.9 + 0.3) / 3.0;
        assert!((score - expected).abs() < 1e-9);
    }

    #[test]
    fn compute_score_only_recency() {
        let w = XwrvWeights { w_recency: 1.0, w_frequency: 0.0, w_volume: 0.0 };
        let i = XwrvInput { recency: 0.75, frequency: 0.0, volume: 0.0 };
        assert!((compute_score(&i, &w) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn compute_score_zero_inputs() {
        let w = XwrvWeights::default();
        let i = XwrvInput { recency: 0.0, frequency: 0.0, volume: 0.0 };
        assert!((compute_score(&i, &w) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn compute_score_all_ones() {
        let w = XwrvWeights::default();
        let i = XwrvInput { recency: 1.0, frequency: 1.0, volume: 1.0 };
        assert!((compute_score(&i, &w) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn xwrv_weights_serde_roundtrip() {
        let w = XwrvWeights::default();
        let s = serde_json::to_string(&w).unwrap();
        let back: XwrvWeights = serde_json::from_str(&s).unwrap();
        assert!((back.w_recency - DEFAULT_W_RECENCY).abs() < 1e-9);
    }

    #[test]
    fn xwrv_input_serde_roundtrip() {
        let i = XwrvInput { recency: 0.4, frequency: 0.3, volume: 0.9 };
        let s = serde_json::to_string(&i).unwrap();
        let back: XwrvInput = serde_json::from_str(&s).unwrap();
        assert!((back.recency - 0.4).abs() < 1e-9);
    }

    #[test]
    fn weights_invalid_when_sum_not_one() {
        let w = XwrvWeights { w_recency: 0.5, w_frequency: 0.5, w_volume: 0.5 };
        assert!(!w.is_valid());
    }

    #[test]
    fn weights_clone_preserves_values() {
        let w = XwrvWeights::default();
        let c = w.clone();
        assert!((c.w_recency - w.w_recency).abs() < 1e-9);
    }

    #[test]
    fn xwrv_input_clone_preserves_values() {
        let i = XwrvInput { recency: 0.1, frequency: 0.2, volume: 0.7 };
        let c = i.clone();
        assert!((c.frequency - 0.2).abs() < 1e-9);
    }

    #[test]
    fn compute_score_default_weights_expected_value() {
        let w = XwrvWeights::default();
        let i = XwrvInput { recency: 1.0, frequency: 0.0, volume: 0.0 };
        assert!((compute_score(&i, &w) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn compute_score_frequency_only() {
        let w = XwrvWeights { w_recency: 0.0, w_frequency: 1.0, w_volume: 0.0 };
        let i = XwrvInput { recency: 0.0, frequency: 0.8, volume: 0.0 };
        assert!((compute_score(&i, &w) - 0.8).abs() < 1e-9);
    }

    #[test]
    fn compute_score_volume_only() {
        let w = XwrvWeights { w_recency: 0.0, w_frequency: 0.0, w_volume: 1.0 };
        let i = XwrvInput { recency: 0.0, frequency: 0.0, volume: 0.6 };
        assert!((compute_score(&i, &w) - 0.6).abs() < 1e-9);
    }

    #[test]
    fn weights_sum_method_correct() {
        let w = XwrvWeights { w_recency: 0.1, w_frequency: 0.2, w_volume: 0.3 };
        assert!((w.sum() - 0.6).abs() < 1e-9);
    }

    #[test]
    fn weights_all_zero_is_invalid() {
        let w = XwrvWeights { w_recency: 0.0, w_frequency: 0.0, w_volume: 0.0 };
        assert!(!w.is_valid());
    }

    #[test]
    fn compute_score_is_linear() {
        let w = XwrvWeights::default();
        let i1 = XwrvInput { recency: 0.2, frequency: 0.4, volume: 0.6 };
        let i2 = XwrvInput { recency: 0.4, frequency: 0.8, volume: 1.2 };
        let s1 = compute_score(&i1, &w);
        let s2 = compute_score(&i2, &w);
        assert!((s2 - 2.0 * s1).abs() < 1e-9);
    }

    #[test]
    fn xwrv_weights_json_contains_field_names() {
        let s = serde_json::to_string(&XwrvWeights::default()).unwrap();
        assert!(s.contains("w_recency"));
        assert!(s.contains("w_frequency"));
        assert!(s.contains("w_volume"));
    }

    #[test]
    fn xwrv_input_json_contains_field_names() {
        let i = XwrvInput { recency: 0.0, frequency: 0.0, volume: 0.0 };
        let s = serde_json::to_string(&i).unwrap();
        assert!(s.contains("recency"));
        assert!(s.contains("frequency"));
        assert!(s.contains("volume"));
    }

    #[test]
    fn compute_score_negative_inputs_allowed() {
        let w = XwrvWeights::default();
        let i = XwrvInput { recency: -1.0, frequency: 0.0, volume: 0.0 };
        assert!(compute_score(&i, &w) < 0.0);
    }

    #[test]
    fn default_weights_debug_contains_w_recency() {
        let s = format!("{:?}", XwrvWeights::default());
        assert!(s.contains("w_recency"));
    }
}

//! PKCE state helpers (S256) — wraps the oidc/pkce module for handlers.
//!
//! This module provides types used by the auth handlers for PKCE state tracking.

use serde::{Deserialize, Serialize};

pub const PKCE_CHALLENGE_METHOD: &str = "S256";
pub const PKCE_VERIFIER_MIN_LEN: usize = 43;
pub const PKCE_VERIFIER_MAX_LEN: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PkceState {
    pub code_verifier:  String,
    pub code_challenge: String,
    pub state_token:    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ChallengeMethod {
    S256,
    Plain,
}

impl ChallengeMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::S256  => "S256",
            Self::Plain => "plain",
        }
    }
}

impl std::fmt::Display for ChallengeMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn verifier_is_valid_length(v: &str) -> bool {
    v.len() >= PKCE_VERIFIER_MIN_LEN && v.len() <= PKCE_VERIFIER_MAX_LEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_method_constant() {
        assert_eq!(PKCE_CHALLENGE_METHOD, "S256");
    }

    #[test]
    fn verifier_min_len_constant() {
        assert_eq!(PKCE_VERIFIER_MIN_LEN, 43);
    }

    #[test]
    fn verifier_max_len_constant() {
        assert_eq!(PKCE_VERIFIER_MAX_LEN, 128);
    }

    #[test]
    fn method_s256_as_str() {
        assert_eq!(ChallengeMethod::S256.as_str(), "S256");
    }

    #[test]
    fn method_plain_as_str() {
        assert_eq!(ChallengeMethod::Plain.as_str(), "plain");
    }

    #[test]
    fn method_display_s256() {
        assert_eq!(format!("{}", ChallengeMethod::S256), "S256");
    }

    #[test]
    fn method_equality() {
        assert_eq!(ChallengeMethod::S256, ChallengeMethod::S256);
        assert_ne!(ChallengeMethod::S256, ChallengeMethod::Plain);
    }

    #[test]
    fn method_serde_roundtrip_s256() {
        let s = serde_json::to_string(&ChallengeMethod::S256).unwrap();
        let back: ChallengeMethod = serde_json::from_str(&s).unwrap();
        assert_eq!(back, ChallengeMethod::S256);
    }

    #[test]
    fn verifier_valid_length_43_chars() {
        let v = "a".repeat(43);
        assert!(verifier_is_valid_length(&v));
    }

    #[test]
    fn verifier_valid_length_128_chars() {
        let v = "a".repeat(128);
        assert!(verifier_is_valid_length(&v));
    }

    #[test]
    fn verifier_invalid_length_short() {
        let v = "a".repeat(42);
        assert!(!verifier_is_valid_length(&v));
    }

    #[test]
    fn verifier_invalid_length_long() {
        let v = "a".repeat(129);
        assert!(!verifier_is_valid_length(&v));
    }

    #[test]
    fn pkce_state_serde_roundtrip() {
        let s = PkceState {
            code_verifier:  "v".repeat(43),
            code_challenge: "c".repeat(43),
            state_token:    "s".repeat(43),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: PkceState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.code_verifier.len(), 43);
        assert_eq!(back.code_challenge.len(), 43);
    }

    #[test]
    fn pkce_state_clone_preserves_state_token() {
        let s = PkceState {
            code_verifier:  "v".repeat(43),
            code_challenge: "c".repeat(43),
            state_token:    "mystate".into(),
        };
        assert_eq!(s.clone().state_token, "mystate");
    }

    #[test]
    fn verifier_valid_length_middle() {
        let v = "a".repeat(80);
        assert!(verifier_is_valid_length(&v));
    }

    #[test]
    fn method_copy_trait() {
        let m = ChallengeMethod::S256;
        let m2 = m;
        assert_eq!(m, m2);
    }

    #[test]
    fn challenge_method_constant_nonempty() {
        assert!(!PKCE_CHALLENGE_METHOD.is_empty());
    }

    #[test]
    fn verifier_empty_is_invalid() {
        assert!(!verifier_is_valid_length(""));
    }

    #[test]
    fn method_debug_contains_variant() {
        let s = format!("{:?}", ChallengeMethod::Plain);
        assert!(s.contains("Plain"));
    }

    #[test]
    fn pkce_state_json_contains_code_verifier_key() {
        let s = PkceState {
            code_verifier: "v".repeat(43),
            code_challenge: "c".repeat(43),
            state_token: "t".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("code_verifier"));
    }

    #[test]
    fn min_len_less_than_max_len() {
        assert!(PKCE_VERIFIER_MIN_LEN < PKCE_VERIFIER_MAX_LEN);
    }

    #[test]
    fn challenge_method_plain_as_str() {
        assert_eq!(ChallengeMethod::Plain.as_str(), "plain");
    }

    #[test]
    fn pkce_state_code_challenge_preserved() {
        let s = PkceState {
            code_verifier:  "v".repeat(43),
            code_challenge: "challenge-value".into(),
            state_token:    "tok".into(),
        };
        assert_eq!(s.code_challenge, "challenge-value");
    }

    #[test]
    fn pkce_verifier_max_len_is_128() {
        assert_eq!(PKCE_VERIFIER_MAX_LEN, 128);
    }

    #[test]
    fn challenge_method_s256_and_plain_have_distinct_str() {
        assert_ne!(ChallengeMethod::S256.as_str(), ChallengeMethod::Plain.as_str());
    }
}

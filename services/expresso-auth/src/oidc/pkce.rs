//! PKCE — S256 code_verifier + challenge generation (RFC 7636).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

/// 43-char base64url verifier (32 bytes entropy).
pub fn generate_verifier() -> String {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// SHA-256 challenge derived from verifier.
pub fn challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

/// URL-safe random state / nonce (32 bytes → 43 chars).
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_is_urlsafe_and_correct_length() {
        let v = generate_verifier();
        assert_eq!(v.len(), 43, "32B base64url-no-pad → 43 chars");
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn challenge_is_deterministic_and_urlsafe() {
        let v = "abc123";
        let c1 = challenge_s256(v);
        let c2 = challenge_s256(v);
        assert_eq!(c1, c2);
        assert!(c1.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn challenge_matches_rfc_test_vector() {
        // RFC 7636 § appendix B
        let v = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let expected = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
        assert_eq!(challenge_s256(v), expected);
    }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpresscryptoError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_display_contains_message() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("key derivation failed"));
        assert!(e.to_string().contains("key derivation failed"));
    }

    #[test]
    fn internal_from_anyhow() {
        let source = anyhow::anyhow!("cipher error");
        let e: ExpresscryptoError = source.into();
        assert!(matches!(e, ExpresscryptoError::Internal(_)));
    }

    #[test]
    fn display_prefix_is_internal_error() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("hmac mismatch"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn debug_contains_type_name() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("x"));
        let d = format!("{e:?}");
        assert!(d.contains("Internal"));
    }

    #[test]
    fn display_includes_original_message() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("original error detail"));
        assert!(e.to_string().contains("original error detail"));
    }

    #[test]
    fn from_anyhow_is_internal_variant() {
        fn returns_error() -> Result<(), ExpresscryptoError> {
            Err(anyhow::anyhow!("test").into())
        }
        assert!(matches!(returns_error(), Err(ExpresscryptoError::Internal(_))));
    }

    #[test]
    fn empty_message_still_prefixed() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!(""));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn unicode_message_preserved() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("chave inválida: ä"));
        assert!(e.to_string().contains("chave inválida"));
    }

    #[test]
    fn long_error_message_not_truncated() {
        let msg = "e".repeat(500);
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("{msg}"));
        assert!(e.to_string().contains(&msg));
    }

    #[test]
    fn display_starts_with_internal_error_prefix() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("oops"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn internal_error_with_empty_message_not_empty() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!(""));
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn internal_error_display_contains_cause() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("aes key too short"));
        assert!(e.to_string().contains("aes key too short"));
    }

    #[test]
    fn internal_error_debug_contains_internal_variant() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("hmac mismatch"));
        assert!(format!("{e:?}").contains("Internal"));
    }

    #[test]
    fn internal_display_starts_with_prefix_long_message() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("pbkdf2 iterations below minimum"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn internal_error_display_contains_cause_text() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("argon2 error"));
        assert!(e.to_string().contains("argon2 error"));
    }

    #[test]
    fn internal_error_display_contains_specific_algorithm_name() {
        let e = ExpresscryptoError::Internal(anyhow::anyhow!("chacha20poly1305 nonce reuse"));
        assert!(e.to_string().contains("chacha20poly1305"));
    }

    #[test]
    fn internal_from_anyhow_matches_internal_variant() {
        let source = anyhow::anyhow!("scrypt params invalid");
        let e: ExpresscryptoError = source.into();
        assert!(matches!(e, ExpresscryptoError::Internal(_)));
    }
}

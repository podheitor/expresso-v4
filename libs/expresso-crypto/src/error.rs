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
}

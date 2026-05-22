use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpressstorageError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_display_contains_message() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("s3 put failed"));
        assert!(e.to_string().contains("s3 put failed"));
    }

    #[test]
    fn internal_from_anyhow() {
        let source = anyhow::anyhow!("bucket not found");
        let e: ExpressstorageError = source.into();
        assert!(matches!(e, ExpressstorageError::Internal(_)));
    }

    #[test]
    fn display_prefix_is_internal_error() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("presign failed"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn debug_contains_internal_variant() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("x"));
        assert!(format!("{e:?}").contains("Internal"));
    }

    #[test]
    fn display_contains_original_message() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("etag mismatch"));
        assert!(e.to_string().contains("etag mismatch"));
    }

    #[test]
    fn from_anyhow_produces_internal() {
        let e: ExpressstorageError = anyhow::anyhow!("test").into();
        assert!(matches!(e, ExpressstorageError::Internal(_)));
    }
}

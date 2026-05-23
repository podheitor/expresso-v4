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

    #[test]
    fn empty_message_still_prefixed() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!(""));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn unicode_message_preserved() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("bucket não encontrado"));
        assert!(e.to_string().contains("bucket não encontrado"));
    }

    #[test]
    fn long_message_not_truncated() {
        let msg = "s".repeat(256);
        let e = ExpressstorageError::Internal(anyhow::anyhow!("{msg}"));
        assert!(e.to_string().contains(&msg));
    }

    #[test]
    fn display_contains_cause_message() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("s3 bucket not found"));
        assert!(e.to_string().contains("s3 bucket not found"));
    }

    #[test]
    fn display_is_nonempty_for_any_error() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("x"));
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn debug_format_contains_internal_variant() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("object not found"));
        assert!(format!("{e:?}").contains("Internal"));
    }

    #[test]
    fn display_starts_with_internal_error_prefix() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("s3 timeout"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn display_contains_cause_text() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("bucket not found"));
        assert!(e.to_string().contains("bucket not found"));
    }

    #[test]
    fn debug_contains_message_from_anyhow() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("multipart upload aborted"));
        assert!(format!("{e:?}").contains("multipart upload aborted"));
    }

    #[test]
    fn display_ends_after_colon_with_message() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("s3 timeout"));
        let s = e.to_string();
        assert!(s.contains("s3 timeout"));
    }

    #[test]
    fn display_prefix_is_internal_error_for_etag_mismatch() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("etag mismatch"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn display_contains_colon_separator() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("test"));
        assert!(e.to_string().contains(':'));
    }

    #[test]
    fn display_is_not_empty_for_empty_cause() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!(""));
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn internal_error_display_ends_with_cause_text() {
        let e = ExpressstorageError::Internal(anyhow::anyhow!("s3 multipart upload failed"));
        assert!(e.to_string().ends_with("s3 multipart upload failed"));
    }
}

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
}

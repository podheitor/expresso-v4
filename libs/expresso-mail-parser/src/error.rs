use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExpressmailParserError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_display_contains_message() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("mime parse failed"));
        assert!(e.to_string().contains("mime parse failed"));
    }

    #[test]
    fn internal_from_anyhow() {
        let source = anyhow::anyhow!("invalid charset");
        let e: ExpressmailParserError = source.into();
        assert!(matches!(e, ExpressmailParserError::Internal(_)));
    }

    #[test]
    fn display_prefix_is_internal_error() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("header fold overflow"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn debug_contains_internal_variant() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("x"));
        assert!(format!("{e:?}").contains("Internal"));
    }
}

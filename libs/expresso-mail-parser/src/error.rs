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

    #[test]
    fn display_contains_original_message() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("content-type malformed"));
        assert!(e.to_string().contains("content-type malformed"));
    }

    #[test]
    fn from_anyhow_produces_internal() {
        let e: ExpressmailParserError = anyhow::anyhow!("test").into();
        assert!(matches!(e, ExpressmailParserError::Internal(_)));
    }

    #[test]
    fn empty_message_still_prefixed() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!(""));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn unicode_message_preserved() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("cabeçalho inválido"));
        assert!(e.to_string().contains("cabeçalho inválido"));
    }

    #[test]
    fn long_message_not_truncated() {
        let msg = "x".repeat(256);
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("{msg}"));
        assert!(e.to_string().contains(&msg));
    }

    #[test]
    fn display_starts_with_internal_error_prefix() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("oops"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn display_contains_cause_message() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("parse failed at byte 42"));
        assert!(e.to_string().contains("parse failed at byte 42"));
    }

    #[test]
    fn display_is_nonempty_for_any_error() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("x"));
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn debug_format_contains_internal_variant() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("mime boundary missing"));
        assert!(format!("{e:?}").contains("Internal"));
    }

    #[test]
    fn display_contains_cause_for_different_message() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("base64 padding error"));
        assert!(e.to_string().contains("base64 padding error"));
    }

    #[test]
    fn internal_error_starts_with_prefix() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("mime parse failed"));
        assert!(e.to_string().starts_with("internal error:"));
    }

    #[test]
    fn debug_does_not_contain_source_variant_name_unexpectedly() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("quoted-printable overflow"));
        let d = format!("{e:?}");
        assert!(d.contains("Internal") && d.contains("quoted-printable overflow"));
    }

    #[test]
    fn internal_display_ends_with_cause_after_colon() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("base64 padding error"));
        let s = e.to_string();
        assert!(s.ends_with("base64 padding error"));
    }

    #[test]
    fn display_contains_colon_separator() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("test"));
        assert!(e.to_string().contains(':'));
    }

    #[test]
    fn display_is_not_empty_for_empty_cause() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!(""));
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn internal_error_display_ends_with_cause_text() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("quoted-printable decode error"));
        assert!(e.to_string().ends_with("quoted-printable decode error"));
    }

    #[test]
    fn two_errors_with_different_messages_differ() {
        let a = ExpressmailParserError::Internal(anyhow::anyhow!("msg_a")).to_string();
        let b = ExpressmailParserError::Internal(anyhow::anyhow!("msg_b")).to_string();
        assert_ne!(a, b);
    }

    #[test]
    fn internal_error_display_is_nonempty_for_whitespace_message() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("   "));
        assert!(!e.to_string().is_empty());
    }

    #[test]
    fn display_contains_prefix_and_cause_separated_by_colon() {
        let e = ExpressmailParserError::Internal(anyhow::anyhow!("unexpected eof"));
        let s = e.to_string();
        assert!(s.starts_with("internal error:") && s.contains("unexpected eof"));
    }
}

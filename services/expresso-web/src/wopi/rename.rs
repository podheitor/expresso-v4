use serde::{Deserialize, Serialize};

pub const MAX_RENAME_LEN: usize = 255;
pub const RENAME_HEADER: &str = "X-WOPI-RequestedName";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenameError {
    InvalidName(String),
    NameTooLong { max: usize, actual: usize },
    NameCollision(String),
    NotLocked,
    LockMismatch,
}

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenameError::InvalidName(name) => write!(f, "invalid name: {}", name),
            RenameError::NameTooLong { max, actual } => write!(f, "name too long: {} > {}", actual, max),
            RenameError::NameCollision(name) => write!(f, "name collision: {}", name),
            RenameError::NotLocked => write!(f, "file not locked"),
            RenameError::LockMismatch => write!(f, "lock mismatch"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameRequest {
    pub requested_name: String,
    pub lock_id:        String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameResponse {
    pub name: String,
}

pub fn validate_rename_target(name: &str) -> Result<(), RenameError> {
    if name.is_empty() {
        return Err(RenameError::InvalidName(name.to_owned()));
    }
    if name.len() > MAX_RENAME_LEN {
        return Err(RenameError::NameTooLong { max: MAX_RENAME_LEN, actual: name.len() });
    }
    let forbidden = ['/', '\\', ':', '*', '?', '"', '<', '>', '|', '\0'];
    for c in forbidden {
        if name.contains(c) {
            return Err(RenameError::InvalidName(name.to_owned()));
        }
    }
    Ok(())
}

pub fn strip_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(pos) if pos > 0 => &name[..pos],
        _ => name,
    }
}

pub fn replace_extension(name: &str, new_ext: &str) -> String {
    let base = strip_extension(name);
    if new_ext.is_empty() {
        base.to_owned()
    } else {
        format!("{}.{}", base, new_ext.trim_start_matches('.'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_rename_len_constant() {
        assert_eq!(MAX_RENAME_LEN, 255);
    }

    #[test]
    fn rename_header_constant() {
        assert_eq!(RENAME_HEADER, "X-WOPI-RequestedName");
    }

    #[test]
    fn validate_valid_name() {
        assert!(validate_rename_target("document.odt").is_ok());
    }

    #[test]
    fn validate_empty_name_fails() {
        assert!(matches!(validate_rename_target(""), Err(RenameError::InvalidName(_))));
    }

    #[test]
    fn validate_too_long_name_fails() {
        let long = "a".repeat(MAX_RENAME_LEN + 1);
        assert!(matches!(validate_rename_target(&long), Err(RenameError::NameTooLong { .. })));
    }

    #[test]
    fn validate_name_with_slash_fails() {
        assert!(matches!(validate_rename_target("path/file"), Err(RenameError::InvalidName(_))));
    }

    #[test]
    fn validate_name_with_colon_fails() {
        assert!(matches!(validate_rename_target("C:file"), Err(RenameError::InvalidName(_))));
    }

    #[test]
    fn validate_name_with_star_fails() {
        assert!(matches!(validate_rename_target("fi*le"), Err(RenameError::InvalidName(_))));
    }

    #[test]
    fn validate_name_with_question_mark_fails() {
        assert!(matches!(validate_rename_target("fi?le"), Err(RenameError::InvalidName(_))));
    }

    #[test]
    fn validate_name_at_max_len() {
        let exact = "a".repeat(MAX_RENAME_LEN);
        assert!(validate_rename_target(&exact).is_ok());
    }

    #[test]
    fn strip_extension_removes_last_dot_segment() {
        assert_eq!(strip_extension("document.odt"), "document");
    }

    #[test]
    fn strip_extension_no_extension_returns_whole() {
        assert_eq!(strip_extension("document"), "document");
    }

    #[test]
    fn strip_extension_multiple_dots_removes_last() {
        assert_eq!(strip_extension("archive.tar.gz"), "archive.tar");
    }

    #[test]
    fn replace_extension_changes_ext() {
        assert_eq!(replace_extension("doc.odt", "docx"), "doc.docx");
    }

    #[test]
    fn replace_extension_empty_ext_removes() {
        assert_eq!(replace_extension("doc.odt", ""), "doc");
    }

    #[test]
    fn replace_extension_with_dot_prefix_stripped() {
        assert_eq!(replace_extension("doc.odt", ".pdf"), "doc.pdf");
    }

    #[test]
    fn display_invalid_name_contains_name() {
        let e = RenameError::InvalidName("bad/name".into());
        assert!(e.to_string().contains("bad/name"));
    }

    #[test]
    fn display_name_too_long_contains_sizes() {
        let e = RenameError::NameTooLong { max: 255, actual: 300 };
        assert!(e.to_string().contains("300"));
        assert!(e.to_string().contains("255"));
    }

    #[test]
    fn display_name_collision_contains_name() {
        let e = RenameError::NameCollision("existing.odt".into());
        assert!(e.to_string().contains("existing.odt"));
    }

    #[test]
    fn display_not_locked() {
        assert!(RenameError::NotLocked.to_string().contains("locked"));
    }

    #[test]
    fn display_lock_mismatch() {
        assert!(RenameError::LockMismatch.to_string().contains("mismatch"));
    }

    #[test]
    fn rename_error_equality() {
        assert_eq!(RenameError::NotLocked, RenameError::NotLocked);
        assert_ne!(RenameError::NotLocked, RenameError::LockMismatch);
    }

    #[test]
    fn validate_name_with_null_byte_fails() {
        assert!(matches!(validate_rename_target("fi\0le"), Err(RenameError::InvalidName(_))));
    }

    #[test]
    fn serde_roundtrip_rename_error() {
        let e = RenameError::NotLocked;
        let json = serde_json::to_string(&e).unwrap();
        let back: RenameError = serde_json::from_str(&json).unwrap();
        assert_eq!(back, RenameError::NotLocked);
    }

    #[test]
    fn strip_extension_dot_at_start_returns_whole() {
        assert_eq!(strip_extension(".hidden"), ".hidden");
    }
}

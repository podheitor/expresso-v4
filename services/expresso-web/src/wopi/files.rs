use serde::{Deserialize, Serialize};

pub const WOPI_VERSION: &str = "1.0";
pub const MAX_FILE_NAME_LEN: usize = 255;
pub const LOCK_TIMEOUT_SECS: u64 = 1800;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WopiFileAction {
    Edit,
    View,
    EmbedView,
    MobileEdit,
}

impl std::fmt::Display for WopiFileAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WopiFileAction::Edit       => write!(f, "edit"),
            WopiFileAction::View       => write!(f, "view"),
            WopiFileAction::EmbedView  => write!(f, "embedview"),
            WopiFileAction::MobileEdit => write!(f, "mobileedit"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckFileInfoResponse {
    pub base_file_name:  String,
    pub size:            i64,
    pub user_id:         String,
    pub user_friendly_name: String,
    pub user_can_write:  bool,
    pub supports_locks:  bool,
    pub supports_update: bool,
    pub version:         String,
}

impl CheckFileInfoResponse {
    pub fn new(name: &str, size: i64, user_id: &str, can_write: bool) -> Self {
        Self {
            base_file_name:     name.to_owned(),
            size,
            user_id:            user_id.to_owned(),
            user_friendly_name: user_id.to_owned(),
            user_can_write:     can_write,
            supports_locks:     true,
            supports_update:    true,
            version:            WOPI_VERSION.to_owned(),
        }
    }

    pub fn is_writable(&self) -> bool {
        self.user_can_write
    }
}

pub fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .take(MAX_FILE_NAME_LEN)
        .collect()
}

pub fn is_valid_lock_id(lock_id: &str) -> bool {
    !lock_id.is_empty() && lock_id.len() <= 256 && lock_id.is_ascii()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wopi_version_constant() {
        assert_eq!(WOPI_VERSION, "1.0");
    }

    #[test]
    fn max_file_name_len_constant() {
        assert_eq!(MAX_FILE_NAME_LEN, 255);
    }

    #[test]
    fn lock_timeout_constant() {
        assert_eq!(LOCK_TIMEOUT_SECS, 1800);
    }

    #[test]
    fn action_display_edit() {
        assert_eq!(WopiFileAction::Edit.to_string(), "edit");
    }

    #[test]
    fn action_display_view() {
        assert_eq!(WopiFileAction::View.to_string(), "view");
    }

    #[test]
    fn action_display_embed_view() {
        assert_eq!(WopiFileAction::EmbedView.to_string(), "embedview");
    }

    #[test]
    fn action_display_mobile_edit() {
        assert_eq!(WopiFileAction::MobileEdit.to_string(), "mobileedit");
    }

    #[test]
    fn check_file_info_new_sets_name() {
        let info = CheckFileInfoResponse::new("doc.odt", 1000, "user1", true);
        assert_eq!(info.base_file_name, "doc.odt");
    }

    #[test]
    fn check_file_info_new_sets_size() {
        let info = CheckFileInfoResponse::new("doc.odt", 2048, "user1", true);
        assert_eq!(info.size, 2048);
    }

    #[test]
    fn check_file_info_writable() {
        let info = CheckFileInfoResponse::new("f.odt", 0, "u", true);
        assert!(info.is_writable());
    }

    #[test]
    fn check_file_info_not_writable() {
        let info = CheckFileInfoResponse::new("f.odt", 0, "u", false);
        assert!(!info.is_writable());
    }

    #[test]
    fn check_file_info_supports_locks_default_true() {
        let info = CheckFileInfoResponse::new("f.odt", 0, "u", true);
        assert!(info.supports_locks);
    }

    #[test]
    fn check_file_info_version_matches_constant() {
        let info = CheckFileInfoResponse::new("f.odt", 0, "u", true);
        assert_eq!(info.version, WOPI_VERSION);
    }

    #[test]
    fn sanitize_removes_slash() {
        assert!(!sanitize_file_name("path/to/file").contains('/'));
    }

    #[test]
    fn sanitize_removes_backslash() {
        assert!(!sanitize_file_name("path\\file").contains('\\'));
    }

    #[test]
    fn sanitize_removes_colon() {
        assert!(!sanitize_file_name("file:name").contains(':'));
    }

    #[test]
    fn sanitize_truncates_at_max_len() {
        let long = "a".repeat(MAX_FILE_NAME_LEN + 10);
        assert_eq!(sanitize_file_name(&long).len(), MAX_FILE_NAME_LEN);
    }

    #[test]
    fn sanitize_normal_name_unchanged() {
        assert_eq!(sanitize_file_name("document.odt"), "document.odt");
    }

    #[test]
    fn valid_lock_id_ascii_nonempty() {
        assert!(is_valid_lock_id("lock-abc-123"));
    }

    #[test]
    fn invalid_lock_id_empty() {
        assert!(!is_valid_lock_id(""));
    }

    #[test]
    fn invalid_lock_id_too_long() {
        let long = "a".repeat(257);
        assert!(!is_valid_lock_id(&long));
    }

    #[test]
    fn action_equality() {
        assert_eq!(WopiFileAction::Edit, WopiFileAction::Edit);
        assert_ne!(WopiFileAction::Edit, WopiFileAction::View);
    }

    #[test]
    fn serde_roundtrip_file_action() {
        let action = WopiFileAction::View;
        let json = serde_json::to_string(&action).unwrap();
        let back: WopiFileAction = serde_json::from_str(&json).unwrap();
        assert_eq!(back, WopiFileAction::View);
    }

    #[test]
    fn sanitize_name_removes_angle_brackets() {
        assert!(!sanitize_file_name("f<ile>").contains('<'));
        assert!(!sanitize_file_name("f<ile>").contains('>'));
    }

    #[test]
    fn sanitize_removes_pipe_character() {
        assert!(!sanitize_file_name("fi|le").contains('|'));
    }

    #[test]
    fn check_file_info_user_friendly_name_equals_user_id() {
        let info = CheckFileInfoResponse::new("f.odt", 0, "alice", true);
        assert_eq!(info.user_friendly_name, info.user_id);
    }
}

//! WOPI discovery XML parsing helpers.

/// A WOPI action entry parsed from the Collabora discovery XML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WopiAction {
    pub name:    String,
    pub urlsrc:  String,
    pub ext:     Option<String>,
}

/// Map a file extension to a canonical MIME type for WOPI purposes.
pub fn ext_to_mime(ext: &str) -> Option<&'static str> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "odt"  => Some("application/vnd.oasis.opendocument.text"),
        "ods"  => Some("application/vnd.oasis.opendocument.spreadsheet"),
        "odp"  => Some("application/vnd.oasis.opendocument.presentation"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "doc"  => Some("application/msword"),
        "xls"  => Some("application/vnd.ms-excel"),
        "ppt"  => Some("application/vnd.ms-powerpoint"),
        "rtf"  => Some("application/rtf"),
        "txt"  => Some("text/plain"),
        "csv"  => Some("text/csv"),
        _      => None,
    }
}

/// Returns whether a WOPI action is an edit action.
pub fn is_edit_action(name: &str) -> bool {
    matches!(name, "edit" | "editnew" | "edittemplate")
}

/// Returns whether a WOPI action is a view-only action.
pub fn is_view_action(name: &str) -> bool {
    matches!(name, "view" | "embedview" | "mobileview" | "readonlyview")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_to_mime_odt() {
        assert_eq!(ext_to_mime("odt"), Some("application/vnd.oasis.opendocument.text"));
    }

    #[test]
    fn ext_to_mime_ods() {
        assert_eq!(ext_to_mime("ods"), Some("application/vnd.oasis.opendocument.spreadsheet"));
    }

    #[test]
    fn ext_to_mime_odp() {
        assert_eq!(ext_to_mime("odp"), Some("application/vnd.oasis.opendocument.presentation"));
    }

    #[test]
    fn ext_to_mime_docx() {
        assert_eq!(ext_to_mime("docx"), Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"));
    }

    #[test]
    fn ext_to_mime_xlsx() {
        assert_eq!(ext_to_mime("xlsx"), Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"));
    }

    #[test]
    fn ext_to_mime_pptx() {
        assert_eq!(ext_to_mime("pptx"), Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"));
    }

    #[test]
    fn ext_to_mime_txt() {
        assert_eq!(ext_to_mime("txt"), Some("text/plain"));
    }

    #[test]
    fn ext_to_mime_csv() {
        assert_eq!(ext_to_mime("csv"), Some("text/csv"));
    }

    #[test]
    fn ext_to_mime_rtf() {
        assert_eq!(ext_to_mime("rtf"), Some("application/rtf"));
    }

    #[test]
    fn ext_to_mime_unknown_returns_none() {
        assert_eq!(ext_to_mime("xyz"), None);
    }

    #[test]
    fn ext_to_mime_png_returns_none() {
        assert_eq!(ext_to_mime("png"), None);
    }

    #[test]
    fn ext_to_mime_with_dot_prefix_works() {
        assert_eq!(ext_to_mime(".odt"), Some("application/vnd.oasis.opendocument.text"));
    }

    #[test]
    fn ext_to_mime_case_insensitive() {
        assert_eq!(ext_to_mime("ODT"), Some("application/vnd.oasis.opendocument.text"));
    }

    #[test]
    fn is_edit_action_edit() {
        assert!(is_edit_action("edit"));
    }

    #[test]
    fn is_edit_action_editnew() {
        assert!(is_edit_action("editnew"));
    }

    #[test]
    fn is_edit_action_edittemplate() {
        assert!(is_edit_action("edittemplate"));
    }

    #[test]
    fn is_edit_action_view_returns_false() {
        assert!(!is_edit_action("view"));
    }

    #[test]
    fn is_view_action_view() {
        assert!(is_view_action("view"));
    }

    #[test]
    fn is_view_action_embedview() {
        assert!(is_view_action("embedview"));
    }

    #[test]
    fn is_view_action_mobileview() {
        assert!(is_view_action("mobileview"));
    }

    #[test]
    fn is_view_action_readonlyview() {
        assert!(is_view_action("readonlyview"));
    }

    #[test]
    fn is_view_action_edit_returns_false() {
        assert!(!is_view_action("edit"));
    }

    #[test]
    fn wopi_action_fields_accessible() {
        let a = WopiAction {
            name: "edit".into(),
            urlsrc: "https://collabora/editor".into(),
            ext: Some("odt".into()),
        };
        assert_eq!(a.name, "edit");
        assert_eq!(a.ext.as_deref(), Some("odt"));
    }

    #[test]
    fn wopi_action_ext_none() {
        let a = WopiAction {
            name: "view".into(),
            urlsrc: "https://c/viewer".into(),
            ext: None,
        };
        assert!(a.ext.is_none());
    }
}

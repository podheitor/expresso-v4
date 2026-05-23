//! Chat message attachments.
//!
//! POST /api/v1/channels/:id/attachments  — upload attachment and get a matrix content URI
//! GET  /api/v1/channels/:id/attachments  — list attachments in a channel

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_ATTACHMENT_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_FILENAME_BYTES:   usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttachmentKind {
    Image,
    Video,
    Audio,
    File,
}

impl AttachmentKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::File  => "file",
        }
    }
}

impl std::fmt::Display for AttachmentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub id:           Uuid,
    pub channel_id:   Uuid,
    pub user_id:      Uuid,
    pub filename:     String,
    pub content_type: String,
    pub size_bytes:   i64,
    pub kind:         AttachmentKind,
    pub mxc_uri:      String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_attachment_bytes_constant() {
        assert_eq!(MAX_ATTACHMENT_BYTES, 100 * 1024 * 1024);
    }

    #[test]
    fn max_filename_bytes_constant() {
        assert_eq!(MAX_FILENAME_BYTES, 255);
    }

    #[test]
    fn kind_image_as_str() {
        assert_eq!(AttachmentKind::Image.as_str(), "image");
    }

    #[test]
    fn kind_video_as_str() {
        assert_eq!(AttachmentKind::Video.as_str(), "video");
    }

    #[test]
    fn kind_audio_as_str() {
        assert_eq!(AttachmentKind::Audio.as_str(), "audio");
    }

    #[test]
    fn kind_file_as_str() {
        assert_eq!(AttachmentKind::File.as_str(), "file");
    }

    #[test]
    fn kind_display_image() {
        assert_eq!(format!("{}", AttachmentKind::Image), "image");
    }

    #[test]
    fn kind_equality() {
        assert_eq!(AttachmentKind::File, AttachmentKind::File);
        assert_ne!(AttachmentKind::File, AttachmentKind::Image);
    }

    #[test]
    fn kind_serde_roundtrip_image() {
        let s = serde_json::to_string(&AttachmentKind::Image).unwrap();
        let back: AttachmentKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AttachmentKind::Image);
    }

    #[test]
    fn kind_serde_roundtrip_file() {
        let s = serde_json::to_string(&AttachmentKind::File).unwrap();
        let back: AttachmentKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AttachmentKind::File);
    }

    #[test]
    fn attachment_serde_roundtrip() {
        let a = Attachment {
            id: Uuid::nil(), channel_id: Uuid::nil(), user_id: Uuid::nil(),
            filename: "photo.jpg".into(), content_type: "image/jpeg".into(),
            size_bytes: 204800, kind: AttachmentKind::Image,
            mxc_uri: "mxc://example.com/abc123".into(),
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: Attachment = serde_json::from_str(&s).unwrap();
        assert_eq!(back.filename, "photo.jpg");
        assert_eq!(back.kind, AttachmentKind::Image);
    }

    #[test]
    fn attachment_clone_preserves_filename() {
        let a = Attachment {
            id: Uuid::nil(), channel_id: Uuid::nil(), user_id: Uuid::nil(),
            filename: "doc.pdf".into(), content_type: "application/pdf".into(),
            size_bytes: 1024, kind: AttachmentKind::File,
            mxc_uri: "mxc://x/y".into(),
        };
        assert_eq!(a.clone().filename, "doc.pdf");
    }

    #[test]
    fn kind_copy_trait() {
        let k = AttachmentKind::Audio;
        let k2 = k;
        assert_eq!(k, k2);
    }

    #[test]
    fn max_attachment_bytes_is_100_mib() {
        assert_eq!(MAX_ATTACHMENT_BYTES, 104_857_600);
    }

    #[test]
    fn attachment_size_bytes_field_accessible() {
        let a = Attachment {
            id: Uuid::nil(), channel_id: Uuid::nil(), user_id: Uuid::nil(),
            filename: "f.mp4".into(), content_type: "video/mp4".into(),
            size_bytes: 5000, kind: AttachmentKind::Video,
            mxc_uri: "mxc://s/r".into(),
        };
        assert_eq!(a.size_bytes, 5000);
    }

    #[test]
    fn attachment_mxc_uri_preserved() {
        let a = Attachment {
            id: Uuid::nil(), channel_id: Uuid::nil(), user_id: Uuid::nil(),
            filename: "f.mp3".into(), content_type: "audio/mpeg".into(),
            size_bytes: 256, kind: AttachmentKind::Audio,
            mxc_uri: "mxc://matrix.org/zzzz".into(),
        };
        assert!(a.mxc_uri.starts_with("mxc://"));
    }

    #[test]
    fn kind_debug_contains_variant() {
        let s = format!("{:?}", AttachmentKind::Video);
        assert!(s.contains("Video"));
    }

    #[test]
    fn attachment_json_contains_mxc_uri_key() {
        let a = Attachment {
            id: Uuid::nil(), channel_id: Uuid::nil(), user_id: Uuid::nil(),
            filename: "f.txt".into(), content_type: "text/plain".into(),
            size_bytes: 10, kind: AttachmentKind::File,
            mxc_uri: "mxc://h/c".into(),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("mxc_uri"));
    }

    #[test]
    fn max_filename_bytes_positive() {
        assert!(MAX_FILENAME_BYTES > 0);
    }

    #[test]
    fn attachment_user_id_accessible() {
        let uid = Uuid::new_v4();
        let a = Attachment {
            id: Uuid::nil(), channel_id: Uuid::nil(), user_id: uid,
            filename: "x".into(), content_type: "text/plain".into(),
            size_bytes: 1, kind: AttachmentKind::File, mxc_uri: "mxc://h/i".into(),
        };
        assert_eq!(a.user_id, uid);
    }

    #[test]
    fn kind_audio_serde_roundtrip() {
        let s = serde_json::to_string(&AttachmentKind::Audio).unwrap();
        let back: AttachmentKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AttachmentKind::Audio);
    }

    #[test]
    fn kind_video_serde_roundtrip() {
        let s = serde_json::to_string(&AttachmentKind::Video).unwrap();
        let back: AttachmentKind = serde_json::from_str(&s).unwrap();
        assert_eq!(back, AttachmentKind::Video);
    }

    #[test]
    fn attachment_content_type_preserved() {
        let a = Attachment {
            id: Uuid::nil(), channel_id: Uuid::nil(), user_id: Uuid::nil(),
            filename: "f.pdf".into(), content_type: "application/pdf".into(),
            size_bytes: 1024, kind: AttachmentKind::File, mxc_uri: "mxc://h/k".into(),
        };
        assert_eq!(a.content_type, "application/pdf");
    }

    #[test]
    fn kind_display_audio() {
        assert_eq!(format!("{}", AttachmentKind::Audio), "audio");
    }

    #[test]
    fn kind_display_video() {
        assert_eq!(format!("{}", AttachmentKind::Video), "video");
    }
}

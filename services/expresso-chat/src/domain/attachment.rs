//! Attachments — file/image/video/audio uploads on chat messages.
//!
//! The bytes live in the Matrix media repo (referenced by `mxc_uri`); this
//! table is the expresso-side index so a channel's attachments can be listed
//! without scanning Matrix history. All writes go through `begin_tenant_tx` so
//! the `chat_attachments` RLS policy scopes by `app.tenant_id`.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use expresso_core::{begin_tenant_tx, DbPool};

use crate::error::Result;

/// Caps shared by the upload handler.
pub const MAX_ATTACHMENT_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_FILENAME_BYTES: usize = 255;

/// Coarse attachment category, derived from the MIME type. Drives the Matrix
/// `msgtype` and lets clients pick a renderer (thumbnail vs file chip).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "lowercase")]
#[sqlx(type_name = "text", rename_all = "lowercase")]
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
            Self::File => "file",
        }
    }

    /// Classify by MIME type prefix; anything not image/video/audio is `File`.
    pub fn from_content_type(content_type: &str) -> Self {
        let main = content_type.split('/').next().unwrap_or("");
        match main {
            "image" => Self::Image,
            "video" => Self::Video,
            "audio" => Self::Audio,
            _ => Self::File,
        }
    }

    /// Matrix `m.room.message` msgtype for this kind.
    pub fn msgtype(&self) -> &'static str {
        match self {
            Self::Image => "m.image",
            Self::Video => "m.video",
            Self::Audio => "m.audio",
            Self::File => "m.file",
        }
    }
}

impl std::fmt::Display for AttachmentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stored attachment row.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Attachment {
    pub id: Uuid,
    pub channel_id: Uuid,
    pub user_id: Uuid,
    pub event_id: String,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub kind: AttachmentKind,
    pub mxc_uri: String,
}

/// Fields needed to persist a freshly-uploaded attachment.
pub struct NewAttachment<'a> {
    pub channel_id: Uuid,
    pub user_id: Uuid,
    pub event_id: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub size_bytes: i64,
    pub kind: AttachmentKind,
    pub mxc_uri: &'a str,
}

pub struct AttachmentRepo<'a> {
    pool: &'a DbPool,
}

impl<'a> AttachmentRepo<'a> {
    pub fn new(pool: &'a DbPool) -> Self {
        Self { pool }
    }

    /// Insert an attachment row, returning the full stored record.
    pub async fn insert(&self, tenant: Uuid, new: &NewAttachment<'_>) -> Result<Attachment> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let row: Attachment = sqlx::query_as(
            r#"INSERT INTO chat_attachments
                   (channel_id, tenant_id, user_id, event_id,
                    filename, content_type, size_bytes, kind, mxc_uri)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
               RETURNING id, channel_id, user_id, event_id,
                         filename, content_type, size_bytes, kind, mxc_uri"#,
        )
        .bind(new.channel_id)
        .bind(tenant)
        .bind(new.user_id)
        .bind(new.event_id)
        .bind(new.filename)
        .bind(new.content_type)
        .bind(new.size_bytes)
        .bind(new.kind)
        .bind(new.mxc_uri)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(row)
    }

    /// List a channel's attachments, newest first, capped at `limit`.
    pub async fn list_for_channel(
        &self,
        tenant: Uuid,
        channel: Uuid,
        limit: i64,
    ) -> Result<Vec<Attachment>> {
        let mut tx = begin_tenant_tx(self.pool, tenant).await?;
        let rows: Vec<Attachment> = sqlx::query_as(
            r#"SELECT id, channel_id, user_id, event_id,
                      filename, content_type, size_bytes, kind, mxc_uri
               FROM chat_attachments
               WHERE channel_id = $1
               ORDER BY created_at DESC
               LIMIT $2"#,
        )
        .bind(channel)
        .bind(limit)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_content_type_classifies_image() {
        assert_eq!(
            AttachmentKind::from_content_type("image/png"),
            AttachmentKind::Image
        );
    }

    #[test]
    fn from_content_type_classifies_video() {
        assert_eq!(
            AttachmentKind::from_content_type("video/mp4"),
            AttachmentKind::Video
        );
    }

    #[test]
    fn from_content_type_classifies_audio() {
        assert_eq!(
            AttachmentKind::from_content_type("audio/mpeg"),
            AttachmentKind::Audio
        );
    }

    #[test]
    fn from_content_type_defaults_to_file() {
        assert_eq!(
            AttachmentKind::from_content_type("application/pdf"),
            AttachmentKind::File
        );
        assert_eq!(AttachmentKind::from_content_type(""), AttachmentKind::File);
    }

    #[test]
    fn msgtype_maps_each_kind() {
        assert_eq!(AttachmentKind::Image.msgtype(), "m.image");
        assert_eq!(AttachmentKind::Video.msgtype(), "m.video");
        assert_eq!(AttachmentKind::Audio.msgtype(), "m.audio");
        assert_eq!(AttachmentKind::File.msgtype(), "m.file");
    }

    #[test]
    fn as_str_matches_serialization() {
        let s = serde_json::to_string(&AttachmentKind::Image).unwrap();
        assert_eq!(s, "\"image\"");
        assert_eq!(AttachmentKind::Image.as_str(), "image");
    }

    #[test]
    fn kind_serde_roundtrip() {
        for k in [
            AttachmentKind::Image,
            AttachmentKind::Video,
            AttachmentKind::Audio,
            AttachmentKind::File,
        ] {
            let s = serde_json::to_string(&k).unwrap();
            let back: AttachmentKind = serde_json::from_str(&s).unwrap();
            assert_eq!(back, k);
        }
    }

    #[test]
    fn kind_display_matches_as_str() {
        assert_eq!(format!("{}", AttachmentKind::Video), "video");
        assert_eq!(format!("{}", AttachmentKind::File), "file");
    }

    #[test]
    fn attachment_serde_roundtrip() {
        let a = Attachment {
            id: Uuid::nil(),
            channel_id: Uuid::nil(),
            user_id: Uuid::nil(),
            event_id: "$evt:hs".into(),
            filename: "photo.jpg".into(),
            content_type: "image/jpeg".into(),
            size_bytes: 204_800,
            kind: AttachmentKind::Image,
            mxc_uri: "mxc://example.com/abc123".into(),
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: Attachment = serde_json::from_str(&s).unwrap();
        assert_eq!(back.filename, "photo.jpg");
        assert_eq!(back.kind, AttachmentKind::Image);
        assert_eq!(back.event_id, "$evt:hs");
    }

    #[test]
    fn max_attachment_bytes_is_100_mib() {
        assert_eq!(MAX_ATTACHMENT_BYTES, 104_857_600);
    }

    #[test]
    fn max_filename_bytes_is_255() {
        assert_eq!(MAX_FILENAME_BYTES, 255);
    }
}

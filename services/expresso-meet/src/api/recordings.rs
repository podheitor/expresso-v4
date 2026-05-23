//! Meeting recordings management.
//!
//! POST /api/v1/meetings/:id/recordings         — create a recording entry
//! GET  /api/v1/meetings/:id/recordings         — list recordings
//! GET  /api/v1/meetings/:id/recordings/:rec_id — get recording detail
//! DELETE /api/v1/meetings/:id/recordings/:rec_id — delete recording

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_RECORDING_NAME_BYTES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingStatus {
    Pending,
    Processing,
    Ready,
    Failed,
}

impl RecordingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending    => "pending",
            Self::Processing => "processing",
            Self::Ready      => "ready",
            Self::Failed     => "failed",
        }
    }
}

impl std::fmt::Display for RecordingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id:          Uuid,
    pub meeting_id:  Uuid,
    pub tenant_id:   Uuid,
    pub name:        String,
    pub status:      RecordingStatus,
    pub size_bytes:  Option<i64>,
    pub duration_s:  Option<i64>,
    pub storage_key: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRecordingBody {
    pub name:        Option<String>,
    pub storage_key: Option<String>,
    pub duration_s:  Option<i64>,
    pub size_bytes:  Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_recording_name_bytes_constant() {
        assert_eq!(MAX_RECORDING_NAME_BYTES, 200);
    }

    #[test]
    fn status_pending_as_str() {
        assert_eq!(RecordingStatus::Pending.as_str(), "pending");
    }

    #[test]
    fn status_processing_as_str() {
        assert_eq!(RecordingStatus::Processing.as_str(), "processing");
    }

    #[test]
    fn status_ready_as_str() {
        assert_eq!(RecordingStatus::Ready.as_str(), "ready");
    }

    #[test]
    fn status_failed_as_str() {
        assert_eq!(RecordingStatus::Failed.as_str(), "failed");
    }

    #[test]
    fn status_display_pending() {
        assert_eq!(format!("{}", RecordingStatus::Pending), "pending");
    }

    #[test]
    fn status_display_ready() {
        assert_eq!(format!("{}", RecordingStatus::Ready), "ready");
    }

    #[test]
    fn status_equality() {
        assert_eq!(RecordingStatus::Ready, RecordingStatus::Ready);
        assert_ne!(RecordingStatus::Ready, RecordingStatus::Failed);
    }

    #[test]
    fn status_serde_roundtrip_pending() {
        let s = serde_json::to_string(&RecordingStatus::Pending).unwrap();
        let back: RecordingStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, RecordingStatus::Pending);
    }

    #[test]
    fn status_serde_roundtrip_ready() {
        let s = serde_json::to_string(&RecordingStatus::Ready).unwrap();
        let back: RecordingStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, RecordingStatus::Ready);
    }

    #[test]
    fn recording_serde_roundtrip() {
        let r = Recording {
            id: Uuid::nil(), meeting_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "Session 1".into(), status: RecordingStatus::Ready,
            size_bytes: Some(1_048_576), duration_s: Some(600),
            storage_key: Some("recordings/abc.mp4".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: Recording = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "Session 1");
        assert_eq!(back.status, RecordingStatus::Ready);
    }

    #[test]
    fn recording_optional_fields_none() {
        let r = Recording {
            id: Uuid::nil(), meeting_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "R".into(), status: RecordingStatus::Pending,
            size_bytes: None, duration_s: None, storage_key: None,
        };
        assert!(r.size_bytes.is_none());
        assert!(r.storage_key.is_none());
    }

    #[test]
    fn create_body_all_optional() {
        let b: CreateRecordingBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.name.is_none());
        assert!(b.storage_key.is_none());
    }

    #[test]
    fn create_body_with_name() {
        let b: CreateRecordingBody =
            serde_json::from_str(r#"{"name":"Meeting 2026-05-22"}"#).unwrap();
        assert_eq!(b.name.as_deref(), Some("Meeting 2026-05-22"));
    }

    #[test]
    fn recording_clone_preserves_name() {
        let r = Recording {
            id: Uuid::nil(), meeting_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "Clone".into(), status: RecordingStatus::Processing,
            size_bytes: None, duration_s: None, storage_key: None,
        };
        assert_eq!(r.clone().name, "Clone");
    }

    #[test]
    fn status_copy_trait() {
        let s = RecordingStatus::Failed;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn recording_json_contains_status_key() {
        let r = Recording {
            id: Uuid::nil(), meeting_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "R".into(), status: RecordingStatus::Ready,
            size_bytes: None, duration_s: None, storage_key: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("status"));
    }

    #[test]
    fn status_debug_contains_variant() {
        let s = format!("{:?}", RecordingStatus::Processing);
        assert!(s.contains("Processing"));
    }

    #[test]
    fn create_body_duration_s_preserved() {
        let b: CreateRecordingBody =
            serde_json::from_str(r#"{"duration_s":3600}"#).unwrap();
        assert_eq!(b.duration_s, Some(3600));
    }

    #[test]
    fn create_body_size_bytes_preserved() {
        let b: CreateRecordingBody =
            serde_json::from_str(r#"{"size_bytes":2097152}"#).unwrap();
        assert_eq!(b.size_bytes, Some(2_097_152));
    }

    #[test]
    fn max_recording_name_bytes_positive() {
        assert!(MAX_RECORDING_NAME_BYTES > 0);
    }

    #[test]
    fn recording_storage_key_preserved() {
        let r = Recording {
            id: Uuid::nil(), meeting_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "R".into(), status: RecordingStatus::Ready,
            size_bytes: None, duration_s: None,
            storage_key: Some("path/to/file.mp4".into()),
        };
        assert_eq!(r.storage_key.as_deref(), Some("path/to/file.mp4"));
    }

    #[test]
    fn status_serde_roundtrip_failed() {
        let s = serde_json::to_string(&RecordingStatus::Failed).unwrap();
        let back: RecordingStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, RecordingStatus::Failed);
    }

    #[test]
    fn status_display_failed() {
        assert_eq!(format!("{}", RecordingStatus::Failed), "failed");
    }
}

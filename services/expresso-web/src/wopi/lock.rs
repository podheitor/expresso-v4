//! WOPI lock operation types and validation.

use serde::{Deserialize, Serialize};

/// Maximum lock ID length in bytes (WOPI spec: max 1024).
pub const MAX_LOCK_ID_BYTES: usize = 1024;

/// WOPI lock operation type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum LockOperation {
    Lock,
    Unlock,
    RefreshLock,
    GetLock,
}

impl std::fmt::Display for LockOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockOperation::Lock        => write!(f, "Lock"),
            LockOperation::Unlock      => write!(f, "Unlock"),
            LockOperation::RefreshLock => write!(f, "RefreshLock"),
            LockOperation::GetLock     => write!(f, "GetLock"),
        }
    }
}

/// A lock state for a WOPI file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockState {
    pub lock_id:    String,
    pub file_id:    String,
    pub is_locked:  bool,
}

/// Validates a lock ID per WOPI spec constraints.
pub fn validate_lock_id(lock_id: &str) -> Option<String> {
    if lock_id.is_empty() {
        return Some("lock_id must not be empty".into());
    }
    if lock_id.len() > MAX_LOCK_ID_BYTES {
        return Some(format!("lock_id exceeds {} bytes", MAX_LOCK_ID_BYTES));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_display_is_lock() {
        assert_eq!(LockOperation::Lock.to_string(), "Lock");
    }

    #[test]
    fn unlock_display_is_unlock() {
        assert_eq!(LockOperation::Unlock.to_string(), "Unlock");
    }

    #[test]
    fn refresh_lock_display() {
        assert_eq!(LockOperation::RefreshLock.to_string(), "RefreshLock");
    }

    #[test]
    fn get_lock_display() {
        assert_eq!(LockOperation::GetLock.to_string(), "GetLock");
    }

    #[test]
    fn lock_eq_same_variant() {
        assert_eq!(LockOperation::Lock, LockOperation::Lock);
    }

    #[test]
    fn lock_ne_unlock() {
        assert_ne!(LockOperation::Lock, LockOperation::Unlock);
    }

    #[test]
    fn validate_lock_id_valid() {
        assert!(validate_lock_id("lock-abc-123").is_none());
    }

    #[test]
    fn validate_lock_id_empty_returns_error() {
        assert!(validate_lock_id("").is_some());
    }

    #[test]
    fn validate_lock_id_too_long_returns_error() {
        let id = "x".repeat(MAX_LOCK_ID_BYTES + 1);
        assert!(validate_lock_id(&id).is_some());
    }

    #[test]
    fn validate_lock_id_at_max_length_is_ok() {
        let id = "x".repeat(MAX_LOCK_ID_BYTES);
        assert!(validate_lock_id(&id).is_none());
    }

    #[test]
    fn max_lock_id_bytes_is_1024() {
        assert_eq!(MAX_LOCK_ID_BYTES, 1024);
    }

    #[test]
    fn lock_state_fields_accessible() {
        let ls = LockState {
            lock_id: "lid-1".into(),
            file_id: "fid-1".into(),
            is_locked: true,
        };
        assert_eq!(ls.lock_id, "lid-1");
        assert_eq!(ls.file_id, "fid-1");
        assert!(ls.is_locked);
    }

    #[test]
    fn lock_state_unlocked() {
        let ls = LockState {
            lock_id: "".into(),
            file_id: "fid-2".into(),
            is_locked: false,
        };
        assert!(!ls.is_locked);
    }

    #[test]
    fn lock_operation_serde_roundtrip_lock() {
        let json = serde_json::to_string(&LockOperation::Lock).unwrap();
        let back: LockOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LockOperation::Lock);
    }

    #[test]
    fn lock_operation_serde_roundtrip_unlock() {
        let json = serde_json::to_string(&LockOperation::Unlock).unwrap();
        let back: LockOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LockOperation::Unlock);
    }

    #[test]
    fn validate_error_mentions_empty_for_empty_id() {
        let msg = validate_lock_id("").unwrap();
        assert!(msg.contains("empty"));
    }

    #[test]
    fn validate_error_mentions_bytes_for_too_long_id() {
        let id = "x".repeat(MAX_LOCK_ID_BYTES + 1);
        let msg = validate_lock_id(&id).unwrap();
        assert!(msg.contains("bytes"));
    }

    #[test]
    fn lock_state_eq_same() {
        let a = LockState { lock_id: "l".into(), file_id: "f".into(), is_locked: true };
        let b = LockState { lock_id: "l".into(), file_id: "f".into(), is_locked: true };
        assert_eq!(a, b);
    }

    #[test]
    fn lock_state_ne_different_lock_id() {
        let a = LockState { lock_id: "l1".into(), file_id: "f".into(), is_locked: true };
        let b = LockState { lock_id: "l2".into(), file_id: "f".into(), is_locked: true };
        assert_ne!(a, b);
    }

    #[test]
    fn lock_operation_all_variants_display_nonempty() {
        for op in [
            LockOperation::Lock, LockOperation::Unlock,
            LockOperation::RefreshLock, LockOperation::GetLock,
        ] {
            assert!(!op.to_string().is_empty());
        }
    }

    #[test]
    fn validate_single_char_lock_id_is_ok() {
        assert!(validate_lock_id("x").is_none());
    }

    #[test]
    fn refresh_lock_ne_get_lock() {
        assert_ne!(LockOperation::RefreshLock, LockOperation::GetLock);
    }

    #[test]
    fn lock_state_file_id_preserved() {
        let ls = LockState { lock_id: "l".into(), file_id: "my-file".into(), is_locked: true };
        assert_eq!(ls.file_id, "my-file");
    }

    #[test]
    fn validate_uuid_style_lock_id_is_ok() {
        assert!(validate_lock_id("550e8400-e29b-41d4-a716-446655440000").is_none());
    }

    #[test]
    fn lock_operation_serde_roundtrip_refresh_lock() {
        let json = serde_json::to_string(&LockOperation::RefreshLock).unwrap();
        let back: LockOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LockOperation::RefreshLock);
    }

    #[test]
    fn lock_operation_serde_roundtrip_get_lock() {
        let json = serde_json::to_string(&LockOperation::GetLock).unwrap();
        let back: LockOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LockOperation::GetLock);
    }
}

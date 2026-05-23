use serde::{Deserialize, Serialize};

pub const CHECKOUT_LOCK_OP: &str = "LOCK";
pub const CHECKOUT_UNLOCK_OP: &str = "UNLOCK";
pub const CHECKOUT_REFRESH_LOCK_OP: &str = "REFRESH_LOCK";
pub const CHECKOUT_GET_LOCK_OP: &str = "GET_LOCK";
pub const MAX_LOCK_ID_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockOperation {
    Lock,
    Unlock,
    RefreshLock,
    GetLock,
    UnlockAndRelock,
}

impl std::fmt::Display for LockOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockOperation::Lock           => write!(f, "LOCK"),
            LockOperation::Unlock         => write!(f, "UNLOCK"),
            LockOperation::RefreshLock    => write!(f, "REFRESH_LOCK"),
            LockOperation::GetLock        => write!(f, "GET_LOCK"),
            LockOperation::UnlockAndRelock => write!(f, "UNLOCK_AND_RELOCK"),
        }
    }
}

impl LockOperation {
    pub fn from_header(s: &str) -> Option<Self> {
        match s.trim() {
            "LOCK"             => Some(LockOperation::Lock),
            "UNLOCK"           => Some(LockOperation::Unlock),
            "REFRESH_LOCK"     => Some(LockOperation::RefreshLock),
            "GET_LOCK"         => Some(LockOperation::GetLock),
            "UNLOCK_AND_RELOCK" => Some(LockOperation::UnlockAndRelock),
            _                  => None,
        }
    }

    pub fn requires_lock_id(&self) -> bool {
        !matches!(self, LockOperation::GetLock)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockState {
    pub lock_id:    String,
    pub locked_by:  String,
    pub locked_at_secs: u64,
    pub expires_at_secs: u64,
}

impl LockState {
    pub fn is_expired(&self, now_secs: u64) -> bool {
        now_secs >= self.expires_at_secs
    }

    pub fn remaining_secs(&self, now_secs: u64) -> u64 {
        self.expires_at_secs.saturating_sub(now_secs)
    }
}

pub fn is_lock_id_valid(lock_id: &str) -> bool {
    !lock_id.is_empty() && lock_id.len() <= MAX_LOCK_ID_LEN && lock_id.is_ascii()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_lock_op_constant() {
        assert_eq!(CHECKOUT_LOCK_OP, "LOCK");
    }

    #[test]
    fn checkout_unlock_op_constant() {
        assert_eq!(CHECKOUT_UNLOCK_OP, "UNLOCK");
    }

    #[test]
    fn checkout_refresh_lock_op_constant() {
        assert_eq!(CHECKOUT_REFRESH_LOCK_OP, "REFRESH_LOCK");
    }

    #[test]
    fn checkout_get_lock_op_constant() {
        assert_eq!(CHECKOUT_GET_LOCK_OP, "GET_LOCK");
    }

    #[test]
    fn max_lock_id_len_constant() {
        assert_eq!(MAX_LOCK_ID_LEN, 256);
    }

    #[test]
    fn lock_op_display_lock() {
        assert_eq!(LockOperation::Lock.to_string(), "LOCK");
    }

    #[test]
    fn lock_op_display_unlock() {
        assert_eq!(LockOperation::Unlock.to_string(), "UNLOCK");
    }

    #[test]
    fn lock_op_display_refresh() {
        assert_eq!(LockOperation::RefreshLock.to_string(), "REFRESH_LOCK");
    }

    #[test]
    fn lock_op_display_get_lock() {
        assert_eq!(LockOperation::GetLock.to_string(), "GET_LOCK");
    }

    #[test]
    fn lock_op_display_unlock_and_relock() {
        assert_eq!(LockOperation::UnlockAndRelock.to_string(), "UNLOCK_AND_RELOCK");
    }

    #[test]
    fn from_header_lock() {
        assert_eq!(LockOperation::from_header("LOCK"), Some(LockOperation::Lock));
    }

    #[test]
    fn from_header_unlock() {
        assert_eq!(LockOperation::from_header("UNLOCK"), Some(LockOperation::Unlock));
    }

    #[test]
    fn from_header_refresh_lock() {
        assert_eq!(LockOperation::from_header("REFRESH_LOCK"), Some(LockOperation::RefreshLock));
    }

    #[test]
    fn from_header_unknown_returns_none() {
        assert_eq!(LockOperation::from_header("SAVE"), None);
    }

    #[test]
    fn from_header_trims_whitespace() {
        assert_eq!(LockOperation::from_header("  LOCK  "), Some(LockOperation::Lock));
    }

    #[test]
    fn get_lock_does_not_require_lock_id() {
        assert!(!LockOperation::GetLock.requires_lock_id());
    }

    #[test]
    fn lock_requires_lock_id() {
        assert!(LockOperation::Lock.requires_lock_id());
    }

    #[test]
    fn refresh_lock_requires_lock_id() {
        assert!(LockOperation::RefreshLock.requires_lock_id());
    }

    #[test]
    fn lock_state_expired_when_past() {
        let state = LockState { lock_id: "x".into(), locked_by: "u".into(), locked_at_secs: 100, expires_at_secs: 200 };
        assert!(state.is_expired(300));
        assert!(!state.is_expired(100));
    }

    #[test]
    fn lock_state_remaining_secs_correct() {
        let state = LockState { lock_id: "x".into(), locked_by: "u".into(), locked_at_secs: 0, expires_at_secs: 1000 };
        assert_eq!(state.remaining_secs(600), 400);
    }

    #[test]
    fn lock_state_remaining_secs_saturates_at_zero() {
        let state = LockState { lock_id: "x".into(), locked_by: "u".into(), locked_at_secs: 0, expires_at_secs: 100 };
        assert_eq!(state.remaining_secs(200), 0);
    }

    #[test]
    fn is_lock_id_valid_for_normal_id() {
        assert!(is_lock_id_valid("valid-lock-id-123"));
    }

    #[test]
    fn is_lock_id_invalid_for_empty() {
        assert!(!is_lock_id_valid(""));
    }

    #[test]
    fn is_lock_id_invalid_for_too_long() {
        assert!(!is_lock_id_valid(&"a".repeat(MAX_LOCK_ID_LEN + 1)));
    }

    #[test]
    fn lock_op_equality() {
        assert_eq!(LockOperation::Lock, LockOperation::Lock);
        assert_ne!(LockOperation::Lock, LockOperation::Unlock);
    }

    #[test]
    fn serde_roundtrip_lock_operation() {
        let op = LockOperation::RefreshLock;
        let json = serde_json::to_string(&op).unwrap();
        let back: LockOperation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, LockOperation::RefreshLock);
    }

    #[test]
    fn unlock_requires_lock_id() {
        assert!(LockOperation::Unlock.requires_lock_id());
    }
}

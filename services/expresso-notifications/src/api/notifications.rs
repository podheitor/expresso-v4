//! Notification dispatch request / response types.

use serde::{Deserialize, Serialize};

/// Supported notification channels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Push,
    Email,
    Websocket,
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::Push      => write!(f, "push"),
            Channel::Email     => write!(f, "email"),
            Channel::Websocket => write!(f, "websocket"),
        }
    }
}

/// Notification priority levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    Normal,
    High,
    Low,
}

/// A notification dispatch request body.
#[derive(Debug, Deserialize)]
pub struct DispatchRequest {
    pub tenant_id:    String,
    pub user_id:      String,
    pub channel:      Channel,
    pub title:        String,
    pub body:         String,
    #[serde(default)]
    pub priority:     Priority,
}

/// Response for a successful dispatch.
#[derive(Debug, Serialize)]
pub struct DispatchResponse {
    pub queued:    bool,
    pub message_id: String,
}

/// Cap on notification body length (characters).
pub const MAX_BODY_CHARS: usize = 4096;
/// Cap on title length.
pub const MAX_TITLE_CHARS: usize = 256;

pub fn validate_dispatch(req: &DispatchRequest) -> Option<String> {
    if req.tenant_id.trim().is_empty() {
        return Some("tenant_id must not be empty".into());
    }
    if req.user_id.trim().is_empty() {
        return Some("user_id must not be empty".into());
    }
    if req.title.chars().count() > MAX_TITLE_CHARS {
        return Some(format!("title exceeds {} chars", MAX_TITLE_CHARS));
    }
    if req.body.chars().count() > MAX_BODY_CHARS {
        return Some(format!("body exceeds {} chars", MAX_BODY_CHARS));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req(tenant: &str, user: &str, title: &str, body: &str) -> DispatchRequest {
        DispatchRequest {
            tenant_id: tenant.into(),
            user_id: user.into(),
            channel: Channel::Push,
            title: title.into(),
            body: body.into(),
            priority: Priority::Normal,
        }
    }

    #[test]
    fn validate_empty_tenant_returns_error() {
        let req = make_req("", "u1", "Hello", "World");
        assert!(validate_dispatch(&req).is_some());
    }

    #[test]
    fn validate_empty_user_returns_error() {
        let req = make_req("t1", "", "Hello", "World");
        assert!(validate_dispatch(&req).is_some());
    }

    #[test]
    fn validate_valid_request_returns_none() {
        let req = make_req("t1", "u1", "Hello", "World");
        assert!(validate_dispatch(&req).is_none());
    }

    #[test]
    fn validate_title_too_long_returns_error() {
        let title = "x".repeat(MAX_TITLE_CHARS + 1);
        let req = make_req("t1", "u1", &title, "body");
        assert!(validate_dispatch(&req).is_some());
    }

    #[test]
    fn validate_body_too_long_returns_error() {
        let body = "x".repeat(MAX_BODY_CHARS + 1);
        let req = make_req("t1", "u1", "title", &body);
        assert!(validate_dispatch(&req).is_some());
    }

    #[test]
    fn validate_exact_title_limit_is_ok() {
        let title = "x".repeat(MAX_TITLE_CHARS);
        let req = make_req("t1", "u1", &title, "body");
        assert!(validate_dispatch(&req).is_none());
    }

    #[test]
    fn validate_exact_body_limit_is_ok() {
        let body = "x".repeat(MAX_BODY_CHARS);
        let req = make_req("t1", "u1", "title", &body);
        assert!(validate_dispatch(&req).is_none());
    }

    #[test]
    fn channel_push_display() {
        assert_eq!(Channel::Push.to_string(), "push");
    }

    #[test]
    fn channel_email_display() {
        assert_eq!(Channel::Email.to_string(), "email");
    }

    #[test]
    fn channel_websocket_display() {
        assert_eq!(Channel::Websocket.to_string(), "websocket");
    }

    #[test]
    fn channel_eq_same_variant() {
        assert_eq!(Channel::Push, Channel::Push);
    }

    #[test]
    fn channel_ne_different_variants() {
        assert_ne!(Channel::Push, Channel::Email);
    }

    #[test]
    fn priority_default_is_normal() {
        let p: Priority = Default::default();
        assert_eq!(p, Priority::Normal);
    }

    #[test]
    fn priority_high_ne_normal() {
        assert_ne!(Priority::High, Priority::Normal);
    }

    #[test]
    fn priority_low_ne_high() {
        assert_ne!(Priority::Low, Priority::High);
    }

    #[test]
    fn max_body_chars_is_positive() {
        assert!(MAX_BODY_CHARS > 0);
    }

    #[test]
    fn max_title_chars_is_positive() {
        assert!(MAX_TITLE_CHARS > 0);
    }

    #[test]
    fn max_body_chars_greater_than_title() {
        assert!(MAX_BODY_CHARS > MAX_TITLE_CHARS);
    }

    #[test]
    fn validate_whitespace_only_tenant_returns_error() {
        let req = make_req("   ", "u1", "t", "b");
        assert!(validate_dispatch(&req).is_some());
    }

    #[test]
    fn validate_whitespace_only_user_returns_error() {
        let req = make_req("t1", "  ", "t", "b");
        assert!(validate_dispatch(&req).is_some());
    }

    #[test]
    fn dispatch_response_queued_field() {
        let r = DispatchResponse { queued: true, message_id: "abc".into() };
        assert!(r.queued);
    }

    #[test]
    fn dispatch_response_message_id_field() {
        let r = DispatchResponse { queued: false, message_id: "msg-42".into() };
        assert_eq!(r.message_id, "msg-42");
    }

    #[test]
    fn channel_serde_roundtrip_push() {
        let json = serde_json::to_string(&Channel::Push).unwrap();
        let back: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Channel::Push);
    }

    #[test]
    fn priority_serde_roundtrip_high() {
        let json = serde_json::to_string(&Priority::High).unwrap();
        let back: Priority = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Priority::High);
    }

    #[test]
    fn channel_serde_roundtrip_websocket() {
        let json = serde_json::to_string(&Channel::Websocket).unwrap();
        let back: Channel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Channel::Websocket);
    }
}

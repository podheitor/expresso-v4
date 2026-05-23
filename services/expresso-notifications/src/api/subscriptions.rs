//! Push subscription management types.

use serde::{Deserialize, Serialize};

/// A Web Push subscription endpoint + keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushSubscription {
    pub endpoint: String,
    pub p256dh:   String,
    pub auth:     String,
}

/// Request body for registering a push subscription.
#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub tenant_id: String,
    pub user_id:   String,
    pub subscription: PushSubscription,
}

/// Request body for unregistering a subscription.
#[derive(Debug, Deserialize)]
pub struct UnregisterRequest {
    pub tenant_id: String,
    pub user_id:   String,
    pub endpoint:  String,
}

/// Response returned after successful registration.
#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub registered: bool,
    pub subscription_id: String,
}

/// Maximum number of subscriptions per user.
pub const MAX_SUBS_PER_USER: usize = 10;

pub fn validate_subscription(sub: &PushSubscription) -> Option<String> {
    if sub.endpoint.trim().is_empty() {
        return Some("endpoint must not be empty".into());
    }
    if !sub.endpoint.starts_with("https://") {
        return Some("endpoint must use HTTPS".into());
    }
    if sub.p256dh.trim().is_empty() {
        return Some("p256dh must not be empty".into());
    }
    if sub.auth.trim().is_empty() {
        return Some("auth must not be empty".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_sub() -> PushSubscription {
        PushSubscription {
            endpoint: "https://push.example.com/sub/abc".into(),
            p256dh:   "key123".into(),
            auth:     "authsecret".into(),
        }
    }

    #[test]
    fn valid_subscription_passes_validation() {
        assert!(validate_subscription(&valid_sub()).is_none());
    }

    #[test]
    fn empty_endpoint_fails_validation() {
        let mut s = valid_sub();
        s.endpoint = "".into();
        assert!(validate_subscription(&s).is_some());
    }

    #[test]
    fn http_endpoint_fails_validation() {
        let mut s = valid_sub();
        s.endpoint = "http://push.example.com/sub/abc".into();
        assert!(validate_subscription(&s).is_some());
    }

    #[test]
    fn empty_p256dh_fails_validation() {
        let mut s = valid_sub();
        s.p256dh = "".into();
        assert!(validate_subscription(&s).is_some());
    }

    #[test]
    fn empty_auth_fails_validation() {
        let mut s = valid_sub();
        s.auth = "".into();
        assert!(validate_subscription(&s).is_some());
    }

    #[test]
    fn https_endpoint_passes() {
        let mut s = valid_sub();
        s.endpoint = "https://fcm.googleapis.com/fcm/send/abc".into();
        assert!(validate_subscription(&s).is_none());
    }

    #[test]
    fn whitespace_endpoint_fails_validation() {
        let mut s = valid_sub();
        s.endpoint = "   ".into();
        assert!(validate_subscription(&s).is_some());
    }

    #[test]
    fn whitespace_p256dh_fails_validation() {
        let mut s = valid_sub();
        s.p256dh = "  ".into();
        assert!(validate_subscription(&s).is_some());
    }

    #[test]
    fn whitespace_auth_fails_validation() {
        let mut s = valid_sub();
        s.auth = "  ".into();
        assert!(validate_subscription(&s).is_some());
    }

    #[test]
    fn max_subs_per_user_is_positive() {
        assert!(MAX_SUBS_PER_USER > 0);
    }

    #[test]
    fn max_subs_per_user_is_reasonable() {
        assert!(MAX_SUBS_PER_USER <= 100);
    }

    #[test]
    fn push_subscription_eq_same() {
        let a = valid_sub();
        let b = valid_sub();
        assert_eq!(a, b);
    }

    #[test]
    fn push_subscription_ne_different_endpoint() {
        let a = valid_sub();
        let mut b = valid_sub();
        b.endpoint = "https://other.example.com/sub/xyz".into();
        assert_ne!(a, b);
    }

    #[test]
    fn register_response_registered_field() {
        let r = RegisterResponse { registered: true, subscription_id: "sid-1".into() };
        assert!(r.registered);
    }

    #[test]
    fn register_response_subscription_id_field() {
        let r = RegisterResponse { registered: false, subscription_id: "sid-99".into() };
        assert_eq!(r.subscription_id, "sid-99");
    }

    #[test]
    fn push_subscription_serde_roundtrip() {
        let sub = valid_sub();
        let json = serde_json::to_string(&sub).unwrap();
        let back: PushSubscription = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sub);
    }

    #[test]
    fn endpoint_field_preserved_in_sub() {
        let s = valid_sub();
        assert_eq!(s.endpoint, "https://push.example.com/sub/abc");
    }

    #[test]
    fn p256dh_field_preserved_in_sub() {
        let s = valid_sub();
        assert_eq!(s.p256dh, "key123");
    }

    #[test]
    fn auth_field_preserved_in_sub() {
        let s = valid_sub();
        assert_eq!(s.auth, "authsecret");
    }

    #[test]
    fn validation_error_message_mentions_https_for_http_endpoint() {
        let mut s = valid_sub();
        s.endpoint = "http://bad.example.com".into();
        let msg = validate_subscription(&s).unwrap();
        assert!(msg.contains("HTTPS"));
    }

    #[test]
    fn validation_error_message_mentions_endpoint_when_empty() {
        let mut s = valid_sub();
        s.endpoint = "".into();
        let msg = validate_subscription(&s).unwrap();
        assert!(msg.contains("endpoint"));
    }

    #[test]
    fn validation_error_message_mentions_p256dh_when_empty() {
        let mut s = valid_sub();
        s.p256dh = "".into();
        let msg = validate_subscription(&s).unwrap();
        assert!(msg.contains("p256dh"));
    }

    #[test]
    fn validation_error_message_mentions_auth_when_empty() {
        let mut s = valid_sub();
        s.auth = "".into();
        let msg = validate_subscription(&s).unwrap();
        assert!(msg.contains("auth"));
    }

    #[test]
    fn max_subs_per_user_value_is_ten() {
        assert_eq!(MAX_SUBS_PER_USER, 10);
    }
}

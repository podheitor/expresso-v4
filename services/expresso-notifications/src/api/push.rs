//! Push notification subscription management.
//!
//! POST   /api/v1/notifications/push/subscribe    — register a push subscription
//! DELETE /api/v1/notifications/push/unsubscribe  — remove a push subscription

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_ENDPOINT_BYTES:  usize = 2048;
pub const MAX_SUBSCRIPTIONS_PER_USER: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushProvider {
    WebPush,
    Fcm,
    Apns,
}

impl PushProvider {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WebPush => "webpush",
            Self::Fcm     => "fcm",
            Self::Apns    => "apns",
        }
    }
}

impl std::fmt::Display for PushProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSubscription {
    pub id:        Uuid,
    pub user_id:   Uuid,
    pub tenant_id: Uuid,
    pub provider:  PushProvider,
    pub endpoint:  String,
    pub enabled:   bool,
}

#[derive(Debug, Deserialize)]
pub struct SubscribeBody {
    pub provider: PushProvider,
    pub endpoint: String,
    pub auth:     Option<String>,
    pub p256dh:   Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_endpoint_bytes_constant() {
        assert_eq!(MAX_ENDPOINT_BYTES, 2048);
    }

    #[test]
    fn max_subscriptions_per_user_constant() {
        assert_eq!(MAX_SUBSCRIPTIONS_PER_USER, 10);
    }

    #[test]
    fn provider_webpush_as_str() {
        assert_eq!(PushProvider::WebPush.as_str(), "webpush");
    }

    #[test]
    fn provider_fcm_as_str() {
        assert_eq!(PushProvider::Fcm.as_str(), "fcm");
    }

    #[test]
    fn provider_apns_as_str() {
        assert_eq!(PushProvider::Apns.as_str(), "apns");
    }

    #[test]
    fn provider_display_webpush() {
        assert_eq!(format!("{}", PushProvider::WebPush), "webpush");
    }

    #[test]
    fn provider_display_fcm() {
        assert_eq!(format!("{}", PushProvider::Fcm), "fcm");
    }

    #[test]
    fn provider_equality() {
        assert_eq!(PushProvider::Fcm, PushProvider::Fcm);
        assert_ne!(PushProvider::Fcm, PushProvider::Apns);
    }

    #[test]
    fn provider_serde_roundtrip_webpush() {
        let s = serde_json::to_string(&PushProvider::WebPush).unwrap();
        let back: PushProvider = serde_json::from_str(&s).unwrap();
        assert_eq!(back, PushProvider::WebPush);
    }

    #[test]
    fn provider_serde_roundtrip_apns() {
        let s = serde_json::to_string(&PushProvider::Apns).unwrap();
        let back: PushProvider = serde_json::from_str(&s).unwrap();
        assert_eq!(back, PushProvider::Apns);
    }

    #[test]
    fn push_subscription_serde_roundtrip() {
        let sub = PushSubscription {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            provider: PushProvider::WebPush,
            endpoint: "https://fcm.example.com/push/abc".into(),
            enabled: true,
        };
        let s = serde_json::to_string(&sub).unwrap();
        let back: PushSubscription = serde_json::from_str(&s).unwrap();
        assert_eq!(back.provider, PushProvider::WebPush);
        assert!(back.enabled);
    }

    #[test]
    fn subscribe_body_auth_optional() {
        let b: SubscribeBody = serde_json::from_str(
            r#"{"provider":"webpush","endpoint":"https://x.example.com/p"}"#,
        ).unwrap();
        assert!(b.auth.is_none());
    }

    #[test]
    fn subscribe_body_p256dh_optional() {
        let b: SubscribeBody = serde_json::from_str(
            r#"{"provider":"fcm","endpoint":"https://fcm.example.com/push"}"#,
        ).unwrap();
        assert!(b.p256dh.is_none());
    }

    #[test]
    fn push_subscription_clone_preserves_endpoint() {
        let sub = PushSubscription {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            provider: PushProvider::Fcm,
            endpoint: "https://endpoint.example.com".into(),
            enabled: false,
        };
        assert_eq!(sub.clone().endpoint, "https://endpoint.example.com");
    }

    #[test]
    fn provider_copy_trait() {
        let p = PushProvider::Apns;
        let p2 = p;
        assert_eq!(p, p2);
    }

    #[test]
    fn max_endpoint_bytes_positive() {
        assert!(MAX_ENDPOINT_BYTES > 0);
    }

    #[test]
    fn provider_debug_contains_variant() {
        let s = format!("{:?}", PushProvider::WebPush);
        assert!(s.contains("WebPush"));
    }

    #[test]
    fn subscribe_body_endpoint_preserved() {
        let b: SubscribeBody = serde_json::from_str(
            r#"{"provider":"apns","endpoint":"device-token-abc"}"#,
        ).unwrap();
        assert_eq!(b.endpoint, "device-token-abc");
    }

    #[test]
    fn push_subscription_enabled_false() {
        let sub = PushSubscription {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            provider: PushProvider::Apns, endpoint: "x".into(), enabled: false,
        };
        assert!(!sub.enabled);
    }

    #[test]
    fn subscribe_body_with_auth_and_p256dh() {
        let b: SubscribeBody = serde_json::from_str(
            r#"{"provider":"webpush","endpoint":"https://e.com","auth":"a1","p256dh":"p1"}"#,
        ).unwrap();
        assert_eq!(b.auth.as_deref(), Some("a1"));
        assert_eq!(b.p256dh.as_deref(), Some("p1"));
    }

    #[test]
    fn max_subscriptions_per_user_positive() {
        assert!(MAX_SUBSCRIPTIONS_PER_USER > 0);
    }

    #[test]
    fn push_subscription_tenant_id_accessible() {
        let tid = Uuid::new_v4();
        let sub = PushSubscription {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: tid,
            provider: PushProvider::WebPush, endpoint: "e".into(), enabled: true,
        };
        assert_eq!(sub.tenant_id, tid);
    }

    #[test]
    fn provider_fcm_serde_roundtrip() {
        let s = serde_json::to_string(&PushProvider::Fcm).unwrap();
        let back: PushProvider = serde_json::from_str(&s).unwrap();
        assert_eq!(back, PushProvider::Fcm);
    }

    #[test]
    fn provider_display_apns() {
        assert_eq!(format!("{}", PushProvider::Apns), "apns");
    }
}

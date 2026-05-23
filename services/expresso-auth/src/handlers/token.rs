//! Token introspection and revocation endpoints.
//!
//! POST /auth/token/introspect  — RFC 7662 token introspection
//! POST /auth/token/revoke      — RFC 7009 token revocation

use serde::{Deserialize, Serialize};

pub const TOKEN_TYPE_BEARER: &str = "Bearer";
pub const TOKEN_TYPE_MAC:    &str = "MAC";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenHint {
    AccessToken,
    RefreshToken,
}

impl TokenHint {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AccessToken  => "access_token",
            Self::RefreshToken => "refresh_token",
        }
    }
}

impl std::fmt::Display for TokenHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Deserialize)]
pub struct IntrospectRequest {
    pub token:            String,
    pub token_type_hint:  Option<TokenHint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntrospectResponse {
    pub active:      bool,
    pub sub:         Option<String>,
    pub exp:         Option<i64>,
    pub iat:         Option<i64>,
    pub client_id:   Option<String>,
    pub token_type:  Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RevokeRequest {
    pub token:           String,
    pub token_type_hint: Option<TokenHint>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_type_bearer_constant() {
        assert_eq!(TOKEN_TYPE_BEARER, "Bearer");
    }

    #[test]
    fn token_type_mac_constant() {
        assert_eq!(TOKEN_TYPE_MAC, "MAC");
    }

    #[test]
    fn hint_access_token_as_str() {
        assert_eq!(TokenHint::AccessToken.as_str(), "access_token");
    }

    #[test]
    fn hint_refresh_token_as_str() {
        assert_eq!(TokenHint::RefreshToken.as_str(), "refresh_token");
    }

    #[test]
    fn hint_display_access_token() {
        assert_eq!(format!("{}", TokenHint::AccessToken), "access_token");
    }

    #[test]
    fn hint_display_refresh_token() {
        assert_eq!(format!("{}", TokenHint::RefreshToken), "refresh_token");
    }

    #[test]
    fn hint_equality() {
        assert_eq!(TokenHint::AccessToken, TokenHint::AccessToken);
        assert_ne!(TokenHint::AccessToken, TokenHint::RefreshToken);
    }

    #[test]
    fn hint_serde_roundtrip_access_token() {
        let s = serde_json::to_string(&TokenHint::AccessToken).unwrap();
        let back: TokenHint = serde_json::from_str(&s).unwrap();
        assert_eq!(back, TokenHint::AccessToken);
    }

    #[test]
    fn hint_serde_roundtrip_refresh_token() {
        let s = serde_json::to_string(&TokenHint::RefreshToken).unwrap();
        let back: TokenHint = serde_json::from_str(&s).unwrap();
        assert_eq!(back, TokenHint::RefreshToken);
    }

    #[test]
    fn introspect_response_active_true() {
        let r = IntrospectResponse {
            active: true, sub: Some("user-xyz".into()), exp: Some(9_999_999_999),
            iat: Some(1_700_000_000), client_id: Some("my-client".into()),
            token_type: Some("Bearer".into()),
        };
        assert!(r.active);
    }

    #[test]
    fn introspect_response_inactive() {
        let r = IntrospectResponse {
            active: false, sub: None, exp: None, iat: None, client_id: None, token_type: None,
        };
        assert!(!r.active);
        assert!(r.sub.is_none());
    }

    #[test]
    fn introspect_response_serde_roundtrip() {
        let r = IntrospectResponse {
            active: true, sub: Some("u1".into()), exp: Some(1_999_999_999),
            iat: Some(1_000_000_000), client_id: None, token_type: Some("Bearer".into()),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: IntrospectResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.active, true);
        assert_eq!(back.sub.as_deref(), Some("u1"));
    }

    #[test]
    fn introspect_request_hint_optional() {
        let r: IntrospectRequest =
            serde_json::from_str(r#"{"token":"tok"}"#).unwrap();
        assert!(r.token_type_hint.is_none());
    }

    #[test]
    fn revoke_request_hint_optional() {
        let r: RevokeRequest =
            serde_json::from_str(r#"{"token":"tok"}"#).unwrap();
        assert!(r.token_type_hint.is_none());
    }

    #[test]
    fn hint_copy_trait() {
        let h = TokenHint::RefreshToken;
        let h2 = h;
        assert_eq!(h, h2);
    }

    #[test]
    fn introspect_response_clone_preserves_active() {
        let r = IntrospectResponse {
            active: true, sub: None, exp: None, iat: None, client_id: None, token_type: None,
        };
        assert!(r.clone().active);
    }

    #[test]
    fn token_type_bearer_nonempty() {
        assert!(!TOKEN_TYPE_BEARER.is_empty());
    }

    #[test]
    fn hint_debug_contains_variant() {
        let s = format!("{:?}", TokenHint::AccessToken);
        assert!(s.contains("AccessToken"));
    }

    #[test]
    fn introspect_response_json_contains_active_key() {
        let r = IntrospectResponse {
            active: false, sub: None, exp: None, iat: None, client_id: None, token_type: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("active"));
    }

    #[test]
    fn revoke_request_token_preserved() {
        let r: RevokeRequest =
            serde_json::from_str(r#"{"token":"mytoken"}"#).unwrap();
        assert_eq!(r.token, "mytoken");
    }

    #[test]
    fn introspect_response_exp_optional() {
        let r = IntrospectResponse {
            active: false, sub: None, exp: None, iat: None, client_id: None, token_type: None,
        };
        assert!(r.exp.is_none());
    }

    #[test]
    fn hint_serde_roundtrip_access() {
        let s = serde_json::to_string(&TokenHint::AccessToken).unwrap();
        let back: TokenHint = serde_json::from_str(&s).unwrap();
        assert_eq!(back, TokenHint::AccessToken);
    }

    #[test]
    fn introspect_response_client_id_optional() {
        let r = IntrospectResponse {
            active: true, sub: None, exp: None, iat: None, client_id: None, token_type: None,
        };
        assert!(r.client_id.is_none());
    }

    #[test]
    fn token_type_mac_nonempty() {
        assert!(!TOKEN_TYPE_MAC.is_empty());
    }
}

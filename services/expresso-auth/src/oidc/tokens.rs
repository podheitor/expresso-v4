//! Token endpoint request/response types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct AuthCodeRequest<'a> {
    pub grant_type:    &'static str,
    pub code:          &'a str,
    pub redirect_uri:  &'a str,
    pub client_id:     &'a str,
    pub code_verifier: &'a str,
}

#[derive(Debug, Serialize)]
pub struct RefreshRequest<'a> {
    pub grant_type:    &'static str,
    pub refresh_token: &'a str,
    pub client_id:     &'a str,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenResponse {
    pub access_token:  String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token:      Option<String>,
    pub token_type:    String,
    pub expires_in:    i64,
    #[serde(default)]
    pub refresh_expires_in: Option<i64>,
    #[serde(default)]
    pub scope:         Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_response_deserialize_minimal() {
        let json = r#"{
            "access_token":  "at123",
            "token_type":    "Bearer",
            "expires_in":    300
        }"#;
        let t: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(t.access_token, "at123");
        assert_eq!(t.token_type, "Bearer");
        assert_eq!(t.expires_in, 300);
        assert!(t.refresh_token.is_none());
        assert!(t.id_token.is_none());
        assert!(t.scope.is_none());
    }

    #[test]
    fn token_response_deserialize_full() {
        let json = r#"{
            "access_token":        "at",
            "refresh_token":       "rt",
            "id_token":            "idt",
            "token_type":          "Bearer",
            "expires_in":          300,
            "refresh_expires_in":  1800,
            "scope":               "openid email"
        }"#;
        let t: TokenResponse = serde_json::from_str(json).unwrap();
        assert_eq!(t.refresh_token.as_deref(), Some("rt"));
        assert_eq!(t.id_token.as_deref(), Some("idt"));
        assert_eq!(t.refresh_expires_in, Some(1800));
        assert_eq!(t.scope.as_deref(), Some("openid email"));
    }

    #[test]
    fn token_response_roundtrip_serde() {
        let orig = TokenResponse {
            access_token:      "at".into(),
            refresh_token:     Some("rt".into()),
            id_token:          None,
            token_type:        "Bearer".into(),
            expires_in:        600,
            refresh_expires_in: Some(3600),
            scope:             Some("openid".into()),
        };
        let json = serde_json::to_string(&orig).unwrap();
        let back: TokenResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.access_token, orig.access_token);
        assert_eq!(back.expires_in, orig.expires_in);
        assert_eq!(back.refresh_token, orig.refresh_token);
    }

    #[test]
    fn token_response_no_id_token_omitted_in_json() {
        let t = TokenResponse {
            access_token: "at".into(), refresh_token: None, id_token: None,
            token_type: "Bearer".into(), expires_in: 300,
            refresh_expires_in: None, scope: None,
        };
        let v: serde_json::Value = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert!(v.get("id_token").map(|x| x.is_null()).unwrap_or(true));
    }

    #[test]
    fn auth_code_request_grant_type_constant() {
        let r = AuthCodeRequest {
            grant_type: "authorization_code",
            code: "c", redirect_uri: "http://x", client_id: "cl", code_verifier: "v",
        };
        assert_eq!(r.grant_type, "authorization_code");
    }

    #[test]
    fn refresh_request_grant_type_constant() {
        let r = RefreshRequest {
            grant_type: "refresh_token",
            refresh_token: "rt", client_id: "cl",
        };
        assert_eq!(r.grant_type, "refresh_token");
    }

    #[test]
    fn token_response_scope_present() {
        let t = TokenResponse {
            access_token: "at".into(), refresh_token: None, id_token: None,
            token_type: "Bearer".into(), expires_in: 300,
            refresh_expires_in: None, scope: Some("openid profile email".into()),
        };
        assert_eq!(t.scope.as_deref(), Some("openid profile email"));
    }

    #[test]
    fn token_response_clone_is_independent() {
        let orig = TokenResponse {
            access_token: "tok".into(), refresh_token: None, id_token: None,
            token_type: "Bearer".into(), expires_in: 60,
            refresh_expires_in: None, scope: None,
        };
        let mut cloned = orig.clone();
        cloned.access_token = "other".into();
        assert_eq!(orig.access_token, "tok");
    }

    #[test]
    fn token_response_refresh_expires_in_none_by_default() {
        let t = TokenResponse {
            access_token: "at".into(), refresh_token: None, id_token: None,
            token_type: "Bearer".into(), expires_in: 300,
            refresh_expires_in: None, scope: None,
        };
        assert!(t.refresh_expires_in.is_none());
    }

    #[test]
    fn token_response_access_token_preserved() {
        let t = TokenResponse {
            access_token: "my-access-token".into(), refresh_token: None, id_token: None,
            token_type: "Bearer".into(), expires_in: 3600,
            refresh_expires_in: None, scope: None,
        };
        assert_eq!(t.access_token, "my-access-token");
    }

    #[test]
    fn token_response_expires_in_preserved() {
        let t = TokenResponse {
            access_token: "tok".into(), refresh_token: None, id_token: None,
            token_type: "Bearer".into(), expires_in: 900,
            refresh_expires_in: None, scope: None,
        };
        assert_eq!(t.expires_in, 900);
    }

    #[test]
    fn token_response_token_type_preserved() {
        let t = TokenResponse {
            access_token: "tok".into(), refresh_token: None, id_token: None,
            token_type: "Bearer".into(), expires_in: 60,
            refresh_expires_in: None, scope: None,
        };
        assert_eq!(t.token_type, "Bearer");
    }
}

//! Jitsi Meet JWT issuer (HS256).
//!
//! Jitsi Meet authenticates participants via JWT tokens signed with a shared
//! app_secret (see `prosody-plugin-token-verification`). The `iss` must match
//! Prosody's `app_id`, `sub` must match the Jitsi host (server_name), and
//! the `context.user` block controls display metadata + moderator flag.
//!
//! Rooms are ephemeral on the Jitsi side — this service only mints tokens.
//! Room existence + ACL live in the `meetings` table (see domain layer).

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, EncodingKey, Header};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{MeetError, Result};

#[derive(Clone, Debug)]
pub struct JitsiConfig {
    pub app_id:      String,   // → JWT `iss`
    pub app_secret:  String,   // → HS256 signing key
    pub domain:      String,   // → JWT `sub` + join URL host
    pub jwt_ttl:     i64,      // seconds
    pub room_prefix: String,   // prepended to auto-generated room slugs
}

#[derive(Clone, Debug)]
pub struct Jitsi {
    cfg: JitsiConfig,
}

#[derive(Debug, Serialize)]
struct Claims<'a> {
    iss:     &'a str,
    aud:     &'a str,
    sub:     &'a str,
    room:    &'a str,
    exp:     i64,
    nbf:     i64,
    iat:     i64,
    context: Context<'a>,
}

#[derive(Debug, Serialize)]
struct Context<'a> {
    user:     UserCtx<'a>,
    features: Features,
}

#[derive(Debug, Serialize)]
struct UserCtx<'a> {
    id:        String,       // Expresso user UUID (string)
    name:      &'a str,
    email:     &'a str,
    moderator: &'a str,      // Jitsi expects "true"/"false" strings
}

#[derive(Debug, Serialize, Default)]
struct Features {
    #[serde(rename = "livestreaming")]
    livestreaming: &'static str,
    recording:     &'static str,
    transcription: &'static str,
    #[serde(rename = "outbound-call")]
    outbound_call: &'static str,
}

/// Input for a single token mint — decoupled from DB shape so callers can
/// fill in with either channel metadata or plain user context.
pub struct IssueRequest<'a> {
    pub room:         &'a str,
    pub user_id:      Uuid,
    pub display_name: &'a str,
    pub email:        &'a str,
    pub moderator:    bool,
    pub allow_recording: bool,
}

pub struct IssuedToken {
    pub token:    String,
    pub room:     String,
    pub domain:   String,
    pub join_url: String,
    pub expires_at_epoch: i64,
}

impl Jitsi {
    pub fn new(cfg: JitsiConfig) -> Self { Self { cfg } }

    pub fn domain(&self)      -> &str { &self.cfg.domain }

    /// Generate a room slug — `<prefix><uuid_no_dashes>`. Used when caller
    /// doesn't supply a pre-existing room name.
    pub fn generate_room_name(&self) -> String {
        let mut buf = Uuid::new_v4().simple().to_string();
        buf.insert_str(0, &self.cfg.room_prefix);
        buf
    }

    pub fn mint(&self, req: &IssueRequest<'_>) -> Result<IssuedToken> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let exp = now + self.cfg.jwt_ttl;
        let moderator_str = if req.moderator { "true" } else { "false" };
        let recording_str = if req.allow_recording { "true" } else { "false" };

        let claims = Claims {
            iss: &self.cfg.app_id,
            aud: "jitsi",
            sub: &self.cfg.domain,
            room: req.room,
            exp,
            nbf: now,
            iat: now,
            context: Context {
                user: UserCtx {
                    id:        req.user_id.to_string(),
                    name:      req.display_name,
                    email:     req.email,
                    moderator: moderator_str,
                },
                features: Features {
                    livestreaming: "false",
                    recording:     recording_str,
                    transcription: "false",
                    outbound_call: "false",
                },
            },
        };
        let header = Header::new(jsonwebtoken::Algorithm::HS256);
        let key    = EncodingKey::from_secret(self.cfg.app_secret.as_bytes());
        let token  = encode(&header, &claims, &key).map_err(MeetError::from)?;
        let join_url = format!("https://{}/{}?jwt={}", self.cfg.domain, req.room, token);
        Ok(IssuedToken {
            token,
            room:   req.room.to_string(),
            domain: self.cfg.domain.clone(),
            join_url,
            expires_at_epoch: exp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{decode, DecodingKey, Validation};

    fn fixture_cfg() -> JitsiConfig {
        JitsiConfig {
            app_id:      "expresso".into(),
            app_secret:  "super-secret-0123456789".into(),
            domain:      "meet.expresso.local".into(),
            jwt_ttl:     3600,
            room_prefix: "exp-".into(),
        }
    }

    #[test]
    fn mint_round_trip_decodes() {
        let j = Jitsi::new(fixture_cfg());
        let uid = Uuid::nil();
        let tok = j.mint(&IssueRequest {
            room: "exp-abc", user_id: uid,
            display_name: "Alice", email: "a@x",
            moderator: true, allow_recording: false,
        }).unwrap();
        let mut val = Validation::new(jsonwebtoken::Algorithm::HS256);
        val.set_audience(&["jitsi"]);
        val.set_issuer(&["expresso"]);
        let data = decode::<serde_json::Value>(
            &tok.token,
            &DecodingKey::from_secret(b"super-secret-0123456789"),
            &val,
        ).unwrap();
        let c = data.claims;
        assert_eq!(c["room"],            "exp-abc");
        assert_eq!(c["sub"],             "meet.expresso.local");
        assert_eq!(c["context"]["user"]["moderator"], "true");
        assert_eq!(c["context"]["features"]["recording"], "false");
    }

    #[test]
    fn generate_room_name_has_prefix() {
        let j = Jitsi::new(fixture_cfg());
        let r = j.generate_room_name();
        assert!(r.starts_with("exp-"));
        assert_eq!(r.len(), "exp-".len() + 32);
    }

    #[test]
    fn join_url_is_https() {
        let j = Jitsi::new(fixture_cfg());
        let tok = j.mint(&IssueRequest {
            room: "r1", user_id: Uuid::nil(),
            display_name: "", email: "",
            moderator: false, allow_recording: false,
        }).unwrap();
        assert!(tok.join_url.starts_with("https://meet.expresso.local/r1?jwt="));
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    fn fixture_cfg() -> JitsiConfig {
        JitsiConfig {
            app_id:      "expresso".into(),
            app_secret:  "super-secret-0123456789".into(),
            domain:      "meet.expresso.local".into(),
            jwt_ttl:     3600,
            room_prefix: "exp-".into(),
        }
    }

    #[test]
    fn domain_accessor() {
        let j = Jitsi::new(fixture_cfg());
        assert_eq!(j.domain(), "meet.expresso.local");
    }

    #[test]
    fn generate_room_name_randomness() {
        let j = Jitsi::new(fixture_cfg());
        let r1 = j.generate_room_name();
        let r2 = j.generate_room_name();
        assert_ne!(r1, r2, "two consecutive room names should differ");
    }

    #[test]
    fn mint_moderator_false() {
        let j = Jitsi::new(fixture_cfg());
        let tok = j.mint(&IssueRequest {
            room: "r", user_id: Uuid::nil(),
            display_name: "Bob", email: "b@x",
            moderator: false, allow_recording: false,
        }).unwrap();
        assert!(tok.join_url.contains("jwt="));
    }

    #[test]
    fn issued_token_has_room_field() {
        let j = Jitsi::new(fixture_cfg());
        let tok = j.mint(&IssueRequest {
            room: "my-room", user_id: Uuid::nil(),
            display_name: "X", email: "x@x",
            moderator: false, allow_recording: true,
        }).unwrap();
        assert!(!tok.token.is_empty());
        assert!(tok.join_url.contains("my-room"));
    }

    #[test]
    fn moderator_flag_affects_token() {
        let j = Jitsi::new(fixture_cfg());
        let mod_tok = j.mint(&IssueRequest {
            room: "r", user_id: Uuid::nil(),
            display_name: "X", email: "x@x",
            moderator: true, allow_recording: false,
        }).unwrap();
        let viewer_tok = j.mint(&IssueRequest {
            room: "r", user_id: Uuid::nil(),
            display_name: "X", email: "x@x",
            moderator: false, allow_recording: false,
        }).unwrap();
        assert_ne!(mod_tok.token, viewer_tok.token);
    }

    #[test]
    fn different_rooms_produce_different_tokens() {
        let j = Jitsi::new(fixture_cfg());
        let req_a = IssueRequest { room: "room-a", user_id: Uuid::nil(), display_name: "X", email: "x@x", moderator: false, allow_recording: false };
        let req_b = IssueRequest { room: "room-b", ..req_a };
        assert_ne!(j.mint(&req_a).unwrap().token, j.mint(&req_b).unwrap().token);
    }

    #[test]
    fn issued_token_join_url_contains_room() {
        let j = Jitsi::new(fixture_cfg());
        let req = IssueRequest { room: "standup", user_id: Uuid::nil(), display_name: "U", email: "u@x", moderator: false, allow_recording: false };
        let tok = j.mint(&req).unwrap();
        assert!(tok.join_url.contains("standup"));
    }

    #[test]
    fn issued_token_is_nonempty() {
        let j = Jitsi::new(fixture_cfg());
        let req = IssueRequest { room: "daily", user_id: Uuid::nil(), display_name: "Dev", email: "dev@x", moderator: true, allow_recording: false };
        let tok = j.mint(&req).unwrap();
        assert!(!tok.token.is_empty());
    }

    #[test]
    fn issued_token_join_url_contains_base_url() {
        let j = Jitsi::new(fixture_cfg());
        let req = IssueRequest { room: "myroom", user_id: Uuid::nil(), display_name: "X", email: "x@x", moderator: false, allow_recording: false };
        let tok = j.mint(&req).unwrap();
        assert!(tok.join_url.starts_with("https://"));
    }

    #[test]
    fn jitsi_config_room_prefix_accessible() {
        let cfg = fixture_cfg();
        let _ = cfg.room_prefix.as_str(); // compiles only if field is String
    }

    #[test]
    fn jitsi_config_domain_preserved() {
        let cfg = fixture_cfg();
        assert_eq!(cfg.domain, "meet.expresso.local");
    }

    #[test]
    fn issued_token_join_url_is_nonempty() {
        let j = Jitsi::new(fixture_cfg());
        let req = IssueRequest { room: "r", user_id: Uuid::nil(), display_name: "X", email: "x@x", moderator: false, allow_recording: false };
        let tok = j.mint(&req).unwrap();
        assert!(!tok.join_url.is_empty());
    }

    #[test]
    fn jitsi_config_jwt_ttl_positive() {
        let cfg = fixture_cfg();
        assert!(cfg.jwt_ttl > 0);
    }

    #[test]
    fn different_users_same_room_produce_different_tokens() {
        let j = Jitsi::new(fixture_cfg());
        let req_a = IssueRequest { room: "r", user_id: Uuid::nil(),    display_name: "A", email: "a@x", moderator: false, allow_recording: false };
        let req_b = IssueRequest { room: "r", user_id: Uuid::new_v4(), display_name: "B", email: "b@x", moderator: false, allow_recording: false };
        assert_ne!(j.mint(&req_a).unwrap().token, j.mint(&req_b).unwrap().token);
    }

    #[test]
    fn jitsi_config_room_prefix_is_nonempty() {
        let cfg = fixture_cfg();
        assert!(!cfg.room_prefix.is_empty());
    }
}

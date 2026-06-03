//! Upstream backend URLs — mirror vite.config dev proxy.

use std::env;

fn envs(k: &str) -> Option<String> {
    env::var(k).ok().filter(|v| !v.trim().is_empty())
}

#[derive(Debug, Clone)]
pub struct Backends {
    pub auth: String,
    pub mail: String,
    pub calendar: String,
    pub contacts: String,
    pub drive: String,
    pub meet: String,
    pub chat: String,
    pub notes: String,
    pub notifications: String,
}

impl Backends {
    pub fn from_env() -> Self {
        Self {
            auth: envs("BACKEND__AUTH").unwrap_or_else(|| "http://localhost:8012".into()),
            mail: envs("BACKEND__MAIL").unwrap_or_else(|| "http://localhost:8001".into()),
            calendar: envs("BACKEND__CALENDAR").unwrap_or_else(|| "http://localhost:8002".into()),
            contacts: envs("BACKEND__CONTACTS").unwrap_or_else(|| "http://localhost:8003".into()),
            drive: envs("BACKEND__DRIVE").unwrap_or_else(|| "http://localhost:8004".into()),
            meet: envs("BACKEND__MEET").unwrap_or_else(|| "http://localhost:8011".into()),
            chat: envs("BACKEND__CHAT").unwrap_or_else(|| "http://localhost:8010".into()),
            notes: envs("BACKEND__NOTES").unwrap_or_else(|| "http://localhost:8012".into()),
            notifications: envs("BACKEND__NOTIFICATIONS")
                .unwrap_or_else(|| "http://localhost:8006".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct JitsiConfig {
    pub domain: String,
    pub app_id: String,
    pub app_secret: String,
    pub room_prefix: String,
    pub jwt_ttl: u64,
}

impl JitsiConfig {
    pub fn from_env() -> Self {
        Self {
            domain: envs("JITSI__DOMAIN").unwrap_or_else(|| "meet.expresso.local".into()),
            app_id: envs("JITSI__APP_ID").unwrap_or_else(|| "expresso".into()),
            app_secret: envs("JITSI__APP_SECRET").unwrap_or_default(),
            room_prefix: envs("JITSI__ROOM_PREFIX").unwrap_or_else(|| "exp-".into()),
            jwt_ttl: envs("JITSI__JWT_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(3600),
        }
    }
    pub fn is_enabled(&self) -> bool {
        !self.app_secret.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Public {
    /// URL do console de autoatendimento Keycloak (link WebAuthn).
    pub kc_account: String,
    /// Caminho público do endpoint login upstream (AUTH rp).
    pub auth_login_path: String,
    pub auth_logout_path: String,
    /// Base URL pública (usada p/ montar absolute redirect_uri pós-login).
    pub web_base_url: String,
}

impl Public {
    pub fn from_env() -> Self {
        Self {
            kc_account: envs("PUBLIC__KC_ACCOUNT")
                .unwrap_or_else(|| "/auth/realms/expresso/account/#/security/signingin".into()),
            auth_login_path: envs("PUBLIC__AUTH_LOGIN").unwrap_or_else(|| "/auth/login".into()),
            auth_logout_path: envs("PUBLIC__AUTH_LOGOUT").unwrap_or_else(|| "/auth/logout".into()),
            web_base_url: envs("PUBLIC__WEB_BASE_URL").unwrap_or_else(|| "".into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Wopi {
    /// Shared HMAC secret — deve casar com WOPI_SECRET do expresso-drive.
    pub secret: String,
    /// URL do Collabora Online acessível pelo BROWSER do usuário.
    /// Ex: http://localhost:9980 (dev) / https://office.exemplo.com (prod).
    pub collabora_url: String,
    /// URL do expresso-drive acessível pelo container Collabora.
    /// Ex: http://expresso-drive:8004 (compose) / https://drive.exemplo.com (prod).
    pub drive_url: String,
    /// TTL do access_token WOPI em segundos. Default 4h.
    pub token_ttl_secs: i64,
}

impl Wopi {
    pub fn from_env() -> Self {
        Self {
            secret: envs("WOPI__SECRET").unwrap_or_default(),
            collabora_url: envs("WOPI__COLLABORA_URL")
                .unwrap_or_else(|| "http://localhost:9980".into()),
            drive_url: envs("WOPI__DRIVE_URL")
                .unwrap_or_else(|| "http://expresso-drive:8004".into()),
            token_ttl_secs: envs("WOPI__TOKEN_TTL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(14400),
        }
    }

    /// Recurso WOPI é opcional — ativo só quando secret configurado.
    pub fn is_enabled(&self) -> bool {
        !self.secret.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wopi_is_enabled_with_non_empty_secret() {
        let w = Wopi {
            secret: "supersecret".into(),
            collabora_url: "http://localhost:9980".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 14400,
        };
        assert!(w.is_enabled());
    }

    #[test]
    fn wopi_is_disabled_with_empty_secret() {
        let w = Wopi {
            secret: "".into(),
            collabora_url: "http://localhost:9980".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 14400,
        };
        assert!(!w.is_enabled());
    }

    #[test]
    fn wopi_is_disabled_with_whitespace_only_secret() {
        let w = Wopi {
            secret: "   ".into(),
            collabora_url: "http://localhost:9980".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 14400,
        };
        assert!(!w.is_enabled());
    }

    #[test]
    fn backends_defaults_compile() {
        let _b = Backends {
            auth: "http://localhost:8012".into(),
            mail: "http://localhost:8001".into(),
            calendar: "http://localhost:8002".into(),
            contacts: "http://localhost:8003".into(),
            drive: "http://localhost:8004".into(),
            meet: "http://localhost:8011".into(),
            chat: "http://localhost:8010".into(),
            notes: "http://localhost:8012".into(),
            notifications: "http://localhost:8006".into(),
        };
        assert_eq!(_b.auth, "http://localhost:8012");
    }

    #[test]
    fn wopi_is_enabled_requires_non_empty_secret() {
        let w = Wopi {
            secret: "".into(),
            collabora_url: "http://collabora:9980".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 3600,
        };
        assert!(!w.is_enabled());
    }

    #[test]
    fn wopi_is_enabled_with_secret() {
        let w = Wopi {
            secret: "mysecret".into(),
            collabora_url: "http://collabora:9980".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 3600,
        };
        assert!(w.is_enabled());
    }

    #[test]
    fn wopi_token_ttl_preserved() {
        let w = Wopi {
            secret: "s".into(),
            collabora_url: "http://collabora:9980".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 7200,
        };
        assert_eq!(w.token_ttl_secs, 7200);
    }

    fn make_backends() -> Backends {
        Backends {
            auth: "http://auth:8012".into(),
            mail: "http://mail:8001".into(),
            calendar: "http://cal:8002".into(),
            contacts: "http://contacts:8003".into(),
            drive: "http://drive:8004".into(),
            meet: "http://meet:8011".into(),
            chat: "http://chat:8010".into(),
            notes: "http://notes:8012".into(),
            notifications: "http://notifications:8006".into(),
        }
    }

    #[test]
    fn backends_drive_url_accessible() {
        assert_eq!(make_backends().drive, "http://drive:8004");
    }

    #[test]
    fn backends_auth_url_accessible() {
        assert_eq!(make_backends().auth, "http://auth:8012");
    }

    #[test]
    fn backends_contacts_url_accessible() {
        assert_eq!(make_backends().contacts, "http://contacts:8003");
    }

    #[test]
    fn backends_drive_field_preserved() {
        assert_eq!(make_backends().drive, "http://drive:8004");
    }

    #[test]
    fn backends_auth_field_preserved() {
        assert_eq!(make_backends().auth, "http://auth:8012");
    }

    #[test]
    fn backends_mail_field_preserved() {
        assert_eq!(make_backends().mail, "http://mail:8001");
    }

    #[test]
    fn backends_drive_url_is_correct() {
        assert_eq!(make_backends().drive, "http://drive:8004");
    }

    #[test]
    fn backends_calendar_field_preserved() {
        assert_eq!(make_backends().calendar, "http://cal:8002");
    }

    #[test]
    fn backends_chat_field_accessible() {
        assert_eq!(make_backends().chat, "http://chat:8010");
    }

    #[test]
    fn backends_meet_field_accessible() {
        assert_eq!(make_backends().meet, "http://meet:8011");
    }

    #[test]
    fn wopi_collabora_url_preserved() {
        let w = Wopi {
            secret: "key".into(),
            collabora_url: "https://office.example.com".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 3600,
        };
        assert_eq!(w.collabora_url, "https://office.example.com");
    }

    #[test]
    fn wopi_token_ttl_7200_is_two_hours() {
        let w = Wopi {
            secret: "s".into(),
            collabora_url: "https://c".into(),
            drive_url: "http://d".into(),
            token_ttl_secs: 7200,
        };
        assert_eq!(w.token_ttl_secs, 7200);
    }

    #[test]
    fn wopi_drive_url_preserved() {
        let w = Wopi {
            secret: "k".into(),
            collabora_url: "https://c".into(),
            drive_url: "http://drive:8004".into(),
            token_ttl_secs: 3600,
        };
        assert_eq!(w.drive_url, "http://drive:8004");
    }

    #[test]
    fn wopi_token_ttl_positive() {
        let w = Wopi {
            secret: "s".into(),
            collabora_url: "https://c".into(),
            drive_url: "http://d".into(),
            token_ttl_secs: 3600,
        };
        assert!(w.token_ttl_secs > 0);
    }

    #[test]
    fn wopi_collabora_url_nonempty() {
        let w = Wopi {
            secret: "x".into(),
            collabora_url: "https://collabora.example.com".into(),
            drive_url: "http://d".into(),
            token_ttl_secs: 7200,
        };
        assert!(!w.collabora_url.is_empty());
    }

    #[test]
    fn wopi_drive_url_nonempty() {
        let w = Wopi {
            secret: "s".into(),
            collabora_url: "https://co".into(),
            drive_url: "https://drive.example.com".into(),
            token_ttl_secs: 300,
        };
        assert!(!w.drive_url.is_empty());
    }

    #[test]
    fn wopi_drive_url_and_collabora_url_can_differ() {
        let w = Wopi {
            collabora_url: "https://collabora.example.com".into(),
            drive_url: "https://drive.example.com".into(),
            secret: "s3cr3t".into(),
            token_ttl_secs: 3600,
        };
        assert_ne!(w.collabora_url, w.drive_url);
    }

    #[test]
    fn wopi_token_ttl_zero_is_zero() {
        let w = Wopi {
            collabora_url: "https://collabora.example.com".into(),
            drive_url: "https://drive.example.com".into(),
            secret: "s3cr3t".into(),
            token_ttl_secs: 0,
        };
        assert_eq!(w.token_ttl_secs, 0);
    }

    #[test]
    fn backends_calendar_url_accessible() {
        assert!(!make_backends().calendar.is_empty());
    }

    #[test]
    fn wopi_is_enabled_false_after_empty_secret_construction() {
        let w = Wopi {
            secret: String::new(),
            collabora_url: "https://c".into(),
            drive_url: "https://d".into(),
            token_ttl_secs: 3600,
        };
        assert!(!w.is_enabled());
    }

    #[test]
    fn backends_seven_distinct_fields() {
        let b = make_backends();
        let all = [
            &b.auth,
            &b.mail,
            &b.calendar,
            &b.contacts,
            &b.drive,
            &b.meet,
            &b.chat,
        ];
        let unique: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(unique.len(), 7);
    }
}

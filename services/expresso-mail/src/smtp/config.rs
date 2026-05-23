use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TlsMode {
    None,
    StartTls,
    ImplicitTls,
}

impl std::fmt::Display for TlsMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsMode::None       => write!(f, "none"),
            TlsMode::StartTls   => write!(f, "starttls"),
            TlsMode::ImplicitTls => write!(f, "implicit_tls"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpConfig {
    pub host:            String,
    pub port:            u16,
    pub tls_mode:        TlsMode,
    pub max_message_bytes: usize,
    pub max_rcpts:       usize,
    pub timeout_secs:    u64,
    pub banner:          String,
    pub require_auth:    bool,
}

impl SmtpConfig {
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    pub fn is_tls_required(&self) -> bool {
        matches!(self.tls_mode, TlsMode::ImplicitTls | TlsMode::StartTls)
    }

    pub fn submission_port() -> u16 {
        587
    }

    pub fn smtps_port() -> u16 {
        465
    }

    pub fn mx_port() -> u16 {
        25
    }
}

impl Default for SmtpConfig {
    fn default() -> Self {
        Self {
            host:              "0.0.0.0".into(),
            port:              25,
            tls_mode:          TlsMode::None,
            max_message_bytes: 50 * 1024 * 1024,
            max_rcpts:         100,
            timeout_secs:      300,
            banner:            "Expresso SMTP ready".into(),
            require_auth:      false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_mode_display_none() {
        assert_eq!(TlsMode::None.to_string(), "none");
    }

    #[test]
    fn tls_mode_display_starttls() {
        assert_eq!(TlsMode::StartTls.to_string(), "starttls");
    }

    #[test]
    fn tls_mode_display_implicit_tls() {
        assert_eq!(TlsMode::ImplicitTls.to_string(), "implicit_tls");
    }

    #[test]
    fn default_port_is_25() {
        assert_eq!(SmtpConfig::default().port, 25);
    }

    #[test]
    fn default_tls_mode_is_none() {
        assert_eq!(SmtpConfig::default().tls_mode, TlsMode::None);
    }

    #[test]
    fn default_max_rcpts_is_100() {
        assert_eq!(SmtpConfig::default().max_rcpts, 100);
    }

    #[test]
    fn default_max_message_bytes_is_50_mib() {
        assert_eq!(SmtpConfig::default().max_message_bytes, 50 * 1024 * 1024);
    }

    #[test]
    fn default_timeout_secs_is_300() {
        assert_eq!(SmtpConfig::default().timeout_secs, 300);
    }

    #[test]
    fn timeout_converts_to_duration() {
        let cfg = SmtpConfig::default();
        assert_eq!(cfg.timeout().as_secs(), 300);
    }

    #[test]
    fn is_tls_required_false_for_none() {
        let cfg = SmtpConfig::default();
        assert!(!cfg.is_tls_required());
    }

    #[test]
    fn is_tls_required_true_for_starttls() {
        let cfg = SmtpConfig { tls_mode: TlsMode::StartTls, ..SmtpConfig::default() };
        assert!(cfg.is_tls_required());
    }

    #[test]
    fn is_tls_required_true_for_implicit_tls() {
        let cfg = SmtpConfig { tls_mode: TlsMode::ImplicitTls, ..SmtpConfig::default() };
        assert!(cfg.is_tls_required());
    }

    #[test]
    fn submission_port_constant_is_587() {
        assert_eq!(SmtpConfig::submission_port(), 587);
    }

    #[test]
    fn smtps_port_constant_is_465() {
        assert_eq!(SmtpConfig::smtps_port(), 465);
    }

    #[test]
    fn mx_port_constant_is_25() {
        assert_eq!(SmtpConfig::mx_port(), 25);
    }

    #[test]
    fn banner_default_contains_smtp() {
        assert!(SmtpConfig::default().banner.to_ascii_lowercase().contains("smtp"));
    }

    #[test]
    fn require_auth_default_is_false() {
        assert!(!SmtpConfig::default().require_auth);
    }

    #[test]
    fn tls_mode_none_equality() {
        assert_eq!(TlsMode::None, TlsMode::None);
        assert_ne!(TlsMode::None, TlsMode::StartTls);
    }

    #[test]
    fn tls_mode_starttls_equality() {
        assert_eq!(TlsMode::StartTls, TlsMode::StartTls);
        assert_ne!(TlsMode::StartTls, TlsMode::ImplicitTls);
    }

    #[test]
    fn tls_mode_implicit_equality() {
        assert_eq!(TlsMode::ImplicitTls, TlsMode::ImplicitTls);
        assert_ne!(TlsMode::ImplicitTls, TlsMode::None);
    }

    #[test]
    fn config_clone_preserves_fields() {
        let cfg = SmtpConfig::default();
        let cloned = cfg.clone();
        assert_eq!(cfg.port, cloned.port);
        assert_eq!(cfg.max_rcpts, cloned.max_rcpts);
    }

    #[test]
    fn serde_roundtrip_tls_mode_none() {
        let mode = TlsMode::None;
        let json = serde_json::to_string(&mode).unwrap();
        let back: TlsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TlsMode::None);
    }

    #[test]
    fn serde_roundtrip_tls_mode_starttls() {
        let mode = TlsMode::StartTls;
        let json = serde_json::to_string(&mode).unwrap();
        let back: TlsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TlsMode::StartTls);
    }

    #[test]
    fn host_default_is_any_address() {
        assert_eq!(SmtpConfig::default().host, "0.0.0.0");
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImapTlsMode {
    None,
    StartTls,
    ImplicitTls,
}

impl std::fmt::Display for ImapTlsMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImapTlsMode::None        => write!(f, "none"),
            ImapTlsMode::StartTls    => write!(f, "starttls"),
            ImapTlsMode::ImplicitTls => write!(f, "implicit_tls"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImapConfig {
    pub host:            String,
    pub port:            u16,
    pub tls_port:        u16,
    pub tls_mode:        ImapTlsMode,
    pub max_connections: usize,
    pub idle_timeout_secs: u64,
    pub literal_plus:    bool,
    pub banner:          String,
}

impl ImapConfig {
    pub fn is_tls(&self) -> bool {
        matches!(self.tls_mode, ImapTlsMode::ImplicitTls | ImapTlsMode::StartTls)
    }

    pub fn plain_port() -> u16 {
        143
    }

    pub fn tls_only_port() -> u16 {
        993
    }
}

impl Default for ImapConfig {
    fn default() -> Self {
        Self {
            host:              "0.0.0.0".into(),
            port:              143,
            tls_port:          993,
            tls_mode:          ImapTlsMode::None,
            max_connections:   1000,
            idle_timeout_secs: 1800,
            literal_plus:      true,
            banner:            "Expresso IMAP4rev1 ready".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_mode_display_none() {
        assert_eq!(ImapTlsMode::None.to_string(), "none");
    }

    #[test]
    fn tls_mode_display_starttls() {
        assert_eq!(ImapTlsMode::StartTls.to_string(), "starttls");
    }

    #[test]
    fn tls_mode_display_implicit_tls() {
        assert_eq!(ImapTlsMode::ImplicitTls.to_string(), "implicit_tls");
    }

    #[test]
    fn default_port_is_143() {
        assert_eq!(ImapConfig::default().port, 143);
    }

    #[test]
    fn default_tls_port_is_993() {
        assert_eq!(ImapConfig::default().tls_port, 993);
    }

    #[test]
    fn default_tls_mode_is_none() {
        assert_eq!(ImapConfig::default().tls_mode, ImapTlsMode::None);
    }

    #[test]
    fn default_max_connections_is_1000() {
        assert_eq!(ImapConfig::default().max_connections, 1000);
    }

    #[test]
    fn default_idle_timeout_is_1800() {
        assert_eq!(ImapConfig::default().idle_timeout_secs, 1800);
    }

    #[test]
    fn default_literal_plus_is_true() {
        assert!(ImapConfig::default().literal_plus);
    }

    #[test]
    fn banner_default_contains_imap() {
        assert!(ImapConfig::default().banner.to_ascii_lowercase().contains("imap"));
    }

    #[test]
    fn is_tls_false_for_none() {
        assert!(!ImapConfig::default().is_tls());
    }

    #[test]
    fn is_tls_true_for_starttls() {
        let cfg = ImapConfig { tls_mode: ImapTlsMode::StartTls, ..ImapConfig::default() };
        assert!(cfg.is_tls());
    }

    #[test]
    fn is_tls_true_for_implicit() {
        let cfg = ImapConfig { tls_mode: ImapTlsMode::ImplicitTls, ..ImapConfig::default() };
        assert!(cfg.is_tls());
    }

    #[test]
    fn plain_port_constant_is_143() {
        assert_eq!(ImapConfig::plain_port(), 143);
    }

    #[test]
    fn tls_only_port_constant_is_993() {
        assert_eq!(ImapConfig::tls_only_port(), 993);
    }

    #[test]
    fn tls_mode_none_equality() {
        assert_eq!(ImapTlsMode::None, ImapTlsMode::None);
    }

    #[test]
    fn tls_mode_inequality() {
        assert_ne!(ImapTlsMode::None, ImapTlsMode::StartTls);
    }

    #[test]
    fn serde_roundtrip_tls_mode_implicit() {
        let mode = ImapTlsMode::ImplicitTls;
        let json = serde_json::to_string(&mode).unwrap();
        let back: ImapTlsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImapTlsMode::ImplicitTls);
    }

    #[test]
    fn serde_roundtrip_tls_mode_none() {
        let mode = ImapTlsMode::None;
        let json = serde_json::to_string(&mode).unwrap();
        let back: ImapTlsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImapTlsMode::None);
    }

    #[test]
    fn clone_preserves_port() {
        let cfg = ImapConfig::default();
        let cloned = cfg.clone();
        assert_eq!(cfg.port, cloned.port);
    }

    #[test]
    fn host_default_is_any_address() {
        assert_eq!(ImapConfig::default().host, "0.0.0.0");
    }

    #[test]
    fn serde_roundtrip_starttls() {
        let mode = ImapTlsMode::StartTls;
        let json = serde_json::to_string(&mode).unwrap();
        let back: ImapTlsMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ImapTlsMode::StartTls);
    }

    #[test]
    fn tls_mode_implicit_inequality_with_starttls() {
        assert_ne!(ImapTlsMode::ImplicitTls, ImapTlsMode::StartTls);
    }

    #[test]
    fn default_host_is_not_loopback() {
        assert_ne!(ImapConfig::default().host, "127.0.0.1");
    }

    #[test]
    fn plain_port_differs_from_tls_only_port() {
        assert_ne!(ImapConfig::plain_port(), ImapConfig::tls_only_port());
    }

    #[test]
    fn default_idle_timeout_is_thirty_minutes() {
        assert_eq!(ImapConfig::default().idle_timeout_secs, 30 * 60);
    }
}

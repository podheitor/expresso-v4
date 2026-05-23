use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMechanism {
    Plain,
    Login,
    CramMd5,
    XOAuth2,
}

impl fmt::Display for AuthMechanism {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AuthMechanism::Plain   => write!(f, "PLAIN"),
            AuthMechanism::Login   => write!(f, "LOGIN"),
            AuthMechanism::CramMd5 => write!(f, "CRAM-MD5"),
            AuthMechanism::XOAuth2 => write!(f, "XOAUTH2"),
        }
    }
}

impl AuthMechanism {
    pub fn is_supported(&self) -> bool {
        matches!(self, AuthMechanism::Plain)
    }

    pub fn requires_tls(&self) -> bool {
        matches!(self, AuthMechanism::Plain | AuthMechanism::Login)
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "PLAIN"   => Some(AuthMechanism::Plain),
            "LOGIN"   => Some(AuthMechanism::Login),
            "CRAM-MD5" => Some(AuthMechanism::CramMd5),
            "XOAUTH2" => Some(AuthMechanism::XOAuth2),
            _         => None,
        }
    }
}

pub fn decode_plain_credentials(blob: &[u8]) -> Option<(String, String)> {
    let parts: Vec<&[u8]> = blob.splitn(3, |&b| b == 0).collect();
    if parts.len() != 3 {
        return None;
    }
    let user = std::str::from_utf8(parts[1]).ok()?.to_owned();
    let pass = std::str::from_utf8(parts[2]).ok()?.to_owned();
    if user.is_empty() {
        return None;
    }
    Some((user, pass))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_plain() {
        assert_eq!(AuthMechanism::Plain.to_string(), "PLAIN");
    }

    #[test]
    fn display_login() {
        assert_eq!(AuthMechanism::Login.to_string(), "LOGIN");
    }

    #[test]
    fn display_cram_md5() {
        assert_eq!(AuthMechanism::CramMd5.to_string(), "CRAM-MD5");
    }

    #[test]
    fn display_xoauth2() {
        assert_eq!(AuthMechanism::XOAuth2.to_string(), "XOAUTH2");
    }

    #[test]
    fn plain_is_supported() {
        assert!(AuthMechanism::Plain.is_supported());
    }

    #[test]
    fn login_not_supported() {
        assert!(!AuthMechanism::Login.is_supported());
    }

    #[test]
    fn cram_md5_not_supported() {
        assert!(!AuthMechanism::CramMd5.is_supported());
    }

    #[test]
    fn plain_requires_tls() {
        assert!(AuthMechanism::Plain.requires_tls());
    }

    #[test]
    fn login_requires_tls() {
        assert!(AuthMechanism::Login.requires_tls());
    }

    #[test]
    fn cram_md5_does_not_require_tls() {
        assert!(!AuthMechanism::CramMd5.requires_tls());
    }

    #[test]
    fn from_str_plain() {
        assert_eq!(AuthMechanism::from_str("PLAIN"), Some(AuthMechanism::Plain));
    }

    #[test]
    fn from_str_plain_lowercase() {
        assert_eq!(AuthMechanism::from_str("plain"), Some(AuthMechanism::Plain));
    }

    #[test]
    fn from_str_login() {
        assert_eq!(AuthMechanism::from_str("LOGIN"), Some(AuthMechanism::Login));
    }

    #[test]
    fn from_str_unknown_returns_none() {
        assert_eq!(AuthMechanism::from_str("GSSAPI"), None);
    }

    #[test]
    fn decode_plain_valid() {
        let blob = b"\x00user@example.com\x00password";
        let result = decode_plain_credentials(blob);
        assert_eq!(result, Some(("user@example.com".into(), "password".into())));
    }

    #[test]
    fn decode_plain_with_authzid() {
        let blob = b"authzid\x00user@example.com\x00secret";
        let result = decode_plain_credentials(blob);
        assert_eq!(result, Some(("user@example.com".into(), "secret".into())));
    }

    #[test]
    fn decode_plain_empty_user_returns_none() {
        let blob = b"\x00\x00password";
        assert!(decode_plain_credentials(blob).is_none());
    }

    #[test]
    fn decode_plain_missing_separator_returns_none() {
        let blob = b"nousernul";
        assert!(decode_plain_credentials(blob).is_none());
    }

    #[test]
    fn decode_plain_empty_password_is_ok() {
        let blob = b"\x00user@x.com\x00";
        let result = decode_plain_credentials(blob);
        assert_eq!(result, Some(("user@x.com".into(), "".into())));
    }

    #[test]
    fn mechanism_equality() {
        assert_eq!(AuthMechanism::Plain, AuthMechanism::Plain);
    }

    #[test]
    fn mechanism_inequality() {
        assert_ne!(AuthMechanism::Plain, AuthMechanism::Login);
    }

    #[test]
    fn from_str_xoauth2() {
        assert_eq!(AuthMechanism::from_str("XOAUTH2"), Some(AuthMechanism::XOAuth2));
    }

    #[test]
    fn from_str_cram_md5() {
        assert_eq!(AuthMechanism::from_str("CRAM-MD5"), Some(AuthMechanism::CramMd5));
    }

    #[test]
    fn xoauth2_does_not_require_tls() {
        assert!(!AuthMechanism::XOAuth2.requires_tls());
    }

    #[test]
    fn from_str_login_lowercase() {
        assert_eq!(AuthMechanism::from_str("login"), Some(AuthMechanism::Login));
    }

    #[test]
    fn xoauth2_not_supported() {
        assert!(!AuthMechanism::XOAuth2.is_supported());
    }
}

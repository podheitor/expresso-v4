//! OIDC helpers — utility types and constants for the relying-party flow.

/// Standard OIDC response_type for the authorization code flow.
pub const RESPONSE_TYPE_CODE: &str = "code";
/// Default OIDC scope for standard claims.
pub const SCOPE_OPENID: &str = "openid";
/// OIDC scope for profile claims.
pub const SCOPE_PROFILE: &str = "profile";
/// OIDC scope for email claims.
pub const SCOPE_EMAIL: &str = "email";
/// Standard PKCE code_challenge_method for SHA-256.
pub const CODE_CHALLENGE_METHOD_S256: &str = "S256";

/// Builds a space-separated scope string from a slice of scope strings.
pub fn build_scope(scopes: &[&str]) -> String {
    scopes.join(" ")
}

/// Returns true when `s` looks like a well-formed HTTPS URL (starts with `https://`).
pub fn is_https_url(s: &str) -> bool {
    s.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_type_code_value() {
        assert_eq!(RESPONSE_TYPE_CODE, "code");
    }

    #[test]
    fn scope_openid_value() {
        assert_eq!(SCOPE_OPENID, "openid");
    }

    #[test]
    fn scope_profile_value() {
        assert_eq!(SCOPE_PROFILE, "profile");
    }

    #[test]
    fn scope_email_value() {
        assert_eq!(SCOPE_EMAIL, "email");
    }

    #[test]
    fn code_challenge_method_s256_value() {
        assert_eq!(CODE_CHALLENGE_METHOD_S256, "S256");
    }

    #[test]
    fn build_scope_single_scope() {
        assert_eq!(build_scope(&["openid"]), "openid");
    }

    #[test]
    fn build_scope_multiple_scopes() {
        assert_eq!(build_scope(&["openid", "email"]), "openid email");
    }

    #[test]
    fn build_scope_three_scopes() {
        assert_eq!(build_scope(&["openid", "profile", "email"]), "openid profile email");
    }

    #[test]
    fn build_scope_empty_slice_returns_empty() {
        assert_eq!(build_scope(&[]), "");
    }

    #[test]
    fn is_https_url_accepts_https() {
        assert!(is_https_url("https://idp.example.com/auth"));
    }

    #[test]
    fn is_https_url_rejects_http() {
        assert!(!is_https_url("http://idp.example.com/auth"));
    }

    #[test]
    fn is_https_url_rejects_empty() {
        assert!(!is_https_url(""));
    }

    #[test]
    fn is_https_url_rejects_partial_prefix() {
        assert!(!is_https_url("https:/idp.example.com"));
    }

    #[test]
    fn all_scope_constants_nonempty() {
        for s in [SCOPE_OPENID, SCOPE_PROFILE, SCOPE_EMAIL] {
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn scope_constants_are_lowercase() {
        for s in [SCOPE_OPENID, SCOPE_PROFILE, SCOPE_EMAIL] {
            assert_eq!(s, s.to_lowercase());
        }
    }

    #[test]
    fn response_type_code_is_lowercase() {
        assert_eq!(RESPONSE_TYPE_CODE, RESPONSE_TYPE_CODE.to_lowercase());
    }

    #[test]
    fn code_challenge_method_is_uppercase() {
        assert_eq!(CODE_CHALLENGE_METHOD_S256, CODE_CHALLENGE_METHOD_S256.to_uppercase());
    }

    #[test]
    fn build_scope_result_contains_openid() {
        let s = build_scope(&["openid", "email"]);
        assert!(s.contains("openid"));
    }

    #[test]
    fn build_scope_result_contains_email() {
        let s = build_scope(&["openid", "email"]);
        assert!(s.contains("email"));
    }

    #[test]
    fn is_https_url_rejects_data_uri() {
        assert!(!is_https_url("data:text/html,foo"));
    }

    #[test]
    fn is_https_url_rejects_javascript_scheme() {
        assert!(!is_https_url("javascript:alert(1)"));
    }

    #[test]
    fn build_scope_produces_space_separated_result() {
        let result = build_scope(&["a", "b", "c"]);
        let parts: Vec<&str> = result.split(' ').collect();
        assert_eq!(parts, vec!["a", "b", "c"]);
    }

    #[test]
    fn build_scope_single_element_no_spaces() {
        let result = build_scope(&["openid"]);
        assert!(!result.contains(' '));
    }

    #[test]
    fn is_https_url_rejects_ftp_scheme() {
        assert!(!is_https_url("ftp://example.com/file"));
    }
}

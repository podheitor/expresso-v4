//! Session cookie constants and helpers for the RP.

/// Name of the access-token cookie set on successful login.
pub const ACCESS_TOKEN_COOKIE: &str = "expresso_at";
/// Name of the refresh-token cookie, scoped to the refresh path.
pub const REFRESH_TOKEN_COOKIE: &str = "expresso_rt";
/// SameSite attribute value used for auth cookies.
pub const SAMESITE_LAX: &str = "Lax";
/// HttpOnly attribute string literal.
pub const HTTP_ONLY_ATTR: &str = "HttpOnly";
/// Secure attribute string literal.
pub const SECURE_ATTR: &str = "Secure";

/// Builds a `Set-Cookie` header value for the access token.
pub fn build_at_cookie(token: &str, max_age: i64, secure: bool) -> String {
    let sec = if secure { "; Secure" } else { "" };
    format!(
        "{name}={token}; HttpOnly; Path=/; SameSite=Lax; Max-Age={max_age}{sec}",
        name = ACCESS_TOKEN_COOKIE,
    )
}

/// Builds a `Set-Cookie` header value to clear the access token.
pub fn clear_at_cookie() -> String {
    format!("{ACCESS_TOKEN_COOKIE}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_token_cookie_name_value() {
        assert_eq!(ACCESS_TOKEN_COOKIE, "expresso_at");
    }

    #[test]
    fn refresh_token_cookie_name_value() {
        assert_eq!(REFRESH_TOKEN_COOKIE, "expresso_rt");
    }

    #[test]
    fn samesite_lax_value() {
        assert_eq!(SAMESITE_LAX, "Lax");
    }

    #[test]
    fn http_only_attr_value() {
        assert_eq!(HTTP_ONLY_ATTR, "HttpOnly");
    }

    #[test]
    fn secure_attr_value() {
        assert_eq!(SECURE_ATTR, "Secure");
    }

    #[test]
    fn build_at_cookie_contains_token() {
        let cookie = build_at_cookie("mytoken", 3600, false);
        assert!(cookie.contains("mytoken"));
    }

    #[test]
    fn build_at_cookie_contains_max_age() {
        let cookie = build_at_cookie("tok", 7200, false);
        assert!(cookie.contains("Max-Age=7200"));
    }

    #[test]
    fn build_at_cookie_secure_true_includes_secure() {
        let cookie = build_at_cookie("tok", 3600, true);
        assert!(cookie.contains("; Secure"));
    }

    #[test]
    fn build_at_cookie_secure_false_excludes_secure() {
        let cookie = build_at_cookie("tok", 3600, false);
        assert!(!cookie.contains("; Secure"));
    }

    #[test]
    fn build_at_cookie_contains_http_only() {
        let cookie = build_at_cookie("tok", 3600, false);
        assert!(cookie.contains("HttpOnly"));
    }

    #[test]
    fn build_at_cookie_contains_samesite_lax() {
        let cookie = build_at_cookie("tok", 3600, false);
        assert!(cookie.contains("SameSite=Lax"));
    }

    #[test]
    fn build_at_cookie_contains_path_slash() {
        let cookie = build_at_cookie("tok", 3600, false);
        assert!(cookie.contains("Path=/"));
    }

    #[test]
    fn build_at_cookie_starts_with_cookie_name() {
        let cookie = build_at_cookie("tok", 3600, false);
        assert!(cookie.starts_with("expresso_at="));
    }

    #[test]
    fn clear_at_cookie_contains_max_age_zero() {
        let cookie = clear_at_cookie();
        assert!(cookie.contains("Max-Age=0"));
    }

    #[test]
    fn clear_at_cookie_contains_cookie_name() {
        let cookie = clear_at_cookie();
        assert!(cookie.contains("expresso_at="));
    }

    #[test]
    fn clear_at_cookie_contains_http_only() {
        let cookie = clear_at_cookie();
        assert!(cookie.contains("HttpOnly"));
    }

    #[test]
    fn access_and_refresh_cookie_names_are_distinct() {
        assert_ne!(ACCESS_TOKEN_COOKIE, REFRESH_TOKEN_COOKIE);
    }

    #[test]
    fn cookie_names_are_nonempty() {
        assert!(!ACCESS_TOKEN_COOKIE.is_empty());
        assert!(!REFRESH_TOKEN_COOKIE.is_empty());
    }

    #[test]
    fn build_at_cookie_zero_max_age_included() {
        let cookie = build_at_cookie("t", 0, false);
        assert!(cookie.contains("Max-Age=0"));
    }

    #[test]
    fn samesite_lax_is_title_case() {
        assert_eq!(&SAMESITE_LAX[..1], "L");
    }

    #[test]
    fn http_only_attr_has_no_spaces() {
        assert!(!HTTP_ONLY_ATTR.contains(' '));
    }

    #[test]
    fn secure_attr_is_nonempty() {
        assert!(!SECURE_ATTR.is_empty());
    }

    #[test]
    fn clear_at_cookie_path_is_slash() {
        assert!(clear_at_cookie().contains("Path=/"));
    }

    #[test]
    fn build_at_cookie_negative_max_age_included() {
        let cookie = build_at_cookie("t", -1, false);
        assert!(cookie.contains("Max-Age=-1"));
    }
}

//! WOPI access token signing and validation helpers.
//!
//! Token format: `{file_id}.{tenant_id}.{user_id}.{exp}.{hmac_hex}`
//! HMAC-SHA256 over `{file_id}|{tenant_id}|{user_id}|{exp}`.

/// Number of dot-separated segments in a valid WOPI token.
pub const TOKEN_SEGMENT_COUNT: usize = 5;

/// Default TTL in seconds (4 hours).
pub const DEFAULT_TOKEN_TTL_SECS: i64 = 14400;

/// Minimum allowed TTL (1 minute).
pub const MIN_TOKEN_TTL_SECS: i64 = 60;

/// Splits a raw token string into its five segments.
/// Returns `None` if the token does not have exactly `TOKEN_SEGMENT_COUNT` segments.
pub fn split_token(token: &str) -> Option<[&str; 5]> {
    let parts: Vec<&str> = token.splitn(6, '.').collect();
    if parts.len() != TOKEN_SEGMENT_COUNT {
        return None;
    }
    Some([parts[0], parts[1], parts[2], parts[3], parts[4]])
}

/// Parses the expiry epoch from a token's fourth segment.
/// Returns `None` on parse failure.
pub fn parse_token_exp(token: &str) -> Option<i64> {
    let parts = split_token(token)?;
    parts[3].parse::<i64>().ok()
}

/// Returns whether the token has expired given the current unix timestamp.
pub fn token_expired(exp: i64, now: i64) -> bool {
    now >= exp
}

/// Validates TTL bounds: must be between MIN and a reasonable max (7 days).
pub fn validate_token_ttl(ttl_secs: i64) -> Option<String> {
    if ttl_secs < MIN_TOKEN_TTL_SECS {
        return Some(format!("ttl_secs must be >= {}", MIN_TOKEN_TTL_SECS));
    }
    if ttl_secs > 86400 * 7 {
        return Some("ttl_secs must be <= 604800 (7 days)".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE_TOKEN: &str = "file1.tenant1.user1.9999999999.deadbeef1234";

    #[test]
    fn split_token_valid() {
        let segs = split_token(FAKE_TOKEN).unwrap();
        assert_eq!(segs[0], "file1");
        assert_eq!(segs[1], "tenant1");
        assert_eq!(segs[2], "user1");
        assert_eq!(segs[3], "9999999999");
    }

    #[test]
    fn split_token_returns_five_segments() {
        let segs = split_token(FAKE_TOKEN).unwrap();
        assert_eq!(segs.len(), TOKEN_SEGMENT_COUNT);
    }

    #[test]
    fn split_token_too_few_segments_returns_none() {
        assert!(split_token("a.b.c.d").is_none());
    }

    #[test]
    fn split_token_too_many_segments_returns_none() {
        assert!(split_token("a.b.c.d.e.f").is_none());
    }

    #[test]
    fn split_token_empty_string_returns_none() {
        assert!(split_token("").is_none());
    }

    #[test]
    fn parse_token_exp_valid() {
        let exp = parse_token_exp(FAKE_TOKEN).unwrap();
        assert_eq!(exp, 9999999999);
    }

    #[test]
    fn parse_token_exp_non_numeric_returns_none() {
        assert!(parse_token_exp("a.b.c.notanumber.sig").is_none());
    }

    #[test]
    fn token_expired_when_now_equals_exp() {
        assert!(token_expired(1000, 1000));
    }

    #[test]
    fn token_expired_when_now_after_exp() {
        assert!(token_expired(1000, 2000));
    }

    #[test]
    fn token_not_expired_when_now_before_exp() {
        assert!(!token_expired(2000, 1000));
    }

    #[test]
    fn token_segment_count_is_five() {
        assert_eq!(TOKEN_SEGMENT_COUNT, 5);
    }

    #[test]
    fn default_token_ttl_is_4_hours() {
        assert_eq!(DEFAULT_TOKEN_TTL_SECS, 14400);
    }

    #[test]
    fn min_token_ttl_is_60_seconds() {
        assert_eq!(MIN_TOKEN_TTL_SECS, 60);
    }

    #[test]
    fn validate_token_ttl_valid() {
        assert!(validate_token_ttl(DEFAULT_TOKEN_TTL_SECS).is_none());
    }

    #[test]
    fn validate_token_ttl_below_min_returns_error() {
        assert!(validate_token_ttl(59).is_some());
    }

    #[test]
    fn validate_token_ttl_zero_returns_error() {
        assert!(validate_token_ttl(0).is_some());
    }

    #[test]
    fn validate_token_ttl_above_7_days_returns_error() {
        assert!(validate_token_ttl(86400 * 8).is_some());
    }

    #[test]
    fn validate_token_ttl_exactly_7_days_is_ok() {
        assert!(validate_token_ttl(86400 * 7).is_none());
    }

    #[test]
    fn validate_token_ttl_min_value_is_ok() {
        assert!(validate_token_ttl(MIN_TOKEN_TTL_SECS).is_none());
    }

    #[test]
    fn validate_error_mentions_min_for_small_ttl() {
        let msg = validate_token_ttl(1).unwrap();
        assert!(msg.contains(&MIN_TOKEN_TTL_SECS.to_string()));
    }

    #[test]
    fn split_token_fifth_segment_is_hmac() {
        let segs = split_token(FAKE_TOKEN).unwrap();
        assert_eq!(segs[4], "deadbeef1234");
    }

    #[test]
    fn default_ttl_greater_than_min_ttl() {
        assert!(DEFAULT_TOKEN_TTL_SECS > MIN_TOKEN_TTL_SECS);
    }

    #[test]
    fn token_expired_boundary_one_second_before_is_not_expired() {
        assert!(!token_expired(1001, 1000));
    }

    #[test]
    fn token_expired_boundary_one_second_after_is_expired() {
        assert!(token_expired(999, 1000));
    }

    #[test]
    fn validate_token_ttl_exactly_min_is_ok() {
        assert_eq!(validate_token_ttl(MIN_TOKEN_TTL_SECS), None);
    }

    #[test]
    fn split_token_third_segment_is_user_id() {
        let segs = split_token(FAKE_TOKEN).unwrap();
        assert_eq!(segs[2], "user1");
    }
}

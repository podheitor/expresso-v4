//! HTTP Basic authentication for the ActiveSync endpoint.
//!
//! EAS clients send `Authorization: Basic base64(user:pass)` over TLS. We verify
//! against the same legacy `users` table (pgcrypto `crypt()`) that IMAP/POP3 use,
//! and apply the shared per-username brute-force lockout. No new auth surface.

use base64::Engine;
use uuid::Uuid;

use crate::imap::lockout::LoginLockout;
use crate::state::AppState;
use once_cell::sync::Lazy;

/// Per-username brute-force throttle, same profile as IMAP/POP3
/// (10 failures / 60s → 5 min lockout).
static LOGIN_LOCKOUT: Lazy<LoginLockout> = Lazy::new(LoginLockout::default);

/// An authenticated EAS principal.
#[derive(Debug, Clone)]
pub struct EasPrincipal {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
}

/// Decode an `Authorization: Basic …` header value into `(user, pass)`.
/// Returns `None` when the scheme is missing/not Basic or the blob is malformed.
pub fn decode_basic(header_value: &str) -> Option<(String, String)> {
    let b64 = header_value.strip_prefix("Basic ")?.trim();
    let decoded = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let s = String::from_utf8(decoded).ok()?;
    let (user, pass) = s.split_once(':')?;
    if user.is_empty() {
        return None;
    }
    // EAS clients may send DOMAIN\user; keep only the user part for the email
    // lookup (the realm/tenant comes from the resolved user row).
    let user = user.rsplit('\\').next().unwrap_or(user);
    Some((user.to_owned(), pass.to_owned()))
}

/// Authenticate the request's Basic credentials against `users` (crypt()),
/// honoring the lockout. Returns the principal on success, `None` otherwise
/// (the caller maps `None` to HTTP 401).
pub async fn authenticate(state: &AppState, authorization: Option<&str>) -> Option<EasPrincipal> {
    let (user, pass) = decode_basic(authorization?)?;
    if LOGIN_LOCKOUT.is_locked_out(&user) {
        return None;
    }
    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, tenant_id FROM users \
         WHERE lower(email) = lower($1) AND password_hash = crypt($2, password_hash) LIMIT 1",
    )
    .bind(&user)
    .bind(&pass)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten();

    match row {
        Some((user_id, tenant_id)) => {
            LOGIN_LOCKOUT.clear_failures(&user);
            Some(EasPrincipal { user_id, tenant_id })
        }
        None => {
            LOGIN_LOCKOUT.record_failure(&user);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(s)
        )
    }

    #[test]
    fn decode_basic_plain() {
        let h = b64("alice@x.com:secret");
        assert_eq!(
            decode_basic(&h),
            Some(("alice@x.com".into(), "secret".into()))
        );
    }

    #[test]
    fn decode_basic_strips_domain_prefix() {
        let h = b64("CORP\\alice:pw");
        assert_eq!(decode_basic(&h), Some(("alice".into(), "pw".into())));
    }

    #[test]
    fn decode_basic_password_with_colon() {
        let h = b64("u@x.com:pa:ss");
        assert_eq!(decode_basic(&h), Some(("u@x.com".into(), "pa:ss".into())));
    }

    #[test]
    fn decode_basic_rejects_non_basic() {
        assert!(decode_basic("Bearer abc").is_none());
    }

    #[test]
    fn decode_basic_rejects_empty_user() {
        let h = b64(":pw");
        assert!(decode_basic(&h).is_none());
    }

    #[test]
    fn decode_basic_rejects_garbage_base64() {
        assert!(decode_basic("Basic !!!notbase64!!!").is_none());
    }

    #[test]
    fn decode_basic_rejects_missing_colon() {
        let h = b64("nocolon");
        assert!(decode_basic(&h).is_none());
    }
}

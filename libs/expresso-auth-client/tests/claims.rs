//! Pure-logic tests for `AuthContext::from_raw` — no network required.

use std::collections::HashMap;

use expresso_auth_client::{AuthContext, AudClaim, RawClaims, RolesBlock, AuthError};

fn base(sub: &str, tenant: Option<&str>) -> RawClaims {
    RawClaims {
        sub:                sub.to_string(),
        iss:                "http://kc/realms/expresso".into(),
        aud:                AudClaim::One("expresso-web".into()),
        exp:                9_999_999_999,
        email:              Some("alice@x".into()),
        preferred_username: Some("alice".into()),
        name:               Some("Alice Doe".into()),
        tenant_id:          tenant.map(String::from),
        realm_access:       Some(RolesBlock { roles: vec!["user".into(), "admin".into()] }),
        resource_access:    {
            let mut m = HashMap::new();
            m.insert("expresso-web".into(), RolesBlock { roles: vec!["admin".into(), "editor".into()] });
            m.insert("other-client".into(), RolesBlock { roles: vec!["leaked".into()] });
            m
        },
        acr:                None,
        amr:                None,
    }
}

#[test]
fn builds_normalized_context() {
    let r = base(
        "c7ee7d76-2113-40bd-9f8c-a28cd6ca395f",
        Some("40894092-7ec5-4693-94f0-afb1c7fb51c4"),
    );
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.email,        "alice@x");
    assert_eq!(ctx.display_name, "Alice Doe");
    assert_eq!(ctx.expires_at,   9_999_999_999);
    assert!(ctx.has_role("admin"));
    assert!(ctx.has_role("editor"));
    assert!(!ctx.has_role("leaked"), "roles from other clients must not leak in");
    // realm_access + resource_access.expresso-web merged, no duplicates.
    assert_eq!(ctx.roles.iter().filter(|r| *r == "admin").count(), 1);
}

#[test]
fn missing_tenant_id_fails() {
    let r = base("c7ee7d76-2113-40bd-9f8c-a28cd6ca395f", None);
    match AuthContext::from_raw(r, "expresso-web") {
        Err(AuthError::MissingClaim("tenant_id")) => {}
        other => panic!("expected MissingClaim, got {other:?}"),
    }
}

#[test]
fn malformed_sub_fails() {
    let r = base("not-a-uuid", Some("40894092-7ec5-4693-94f0-afb1c7fb51c4"));
    match AuthContext::from_raw(r, "expresso-web") {
        Err(AuthError::MalformedClaim("sub", _)) => {}
        other => panic!("expected MalformedClaim, got {other:?}"),
    }
}

#[test]
fn display_name_falls_back_to_preferred_username() {
    let mut r = base("c7ee7d76-2113-40bd-9f8c-a28cd6ca395f", Some("40894092-7ec5-4693-94f0-afb1c7fb51c4"));
    r.name = None;
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.display_name, "alice");
}

#[test]
fn display_name_falls_back_to_user_prefix() {
    let mut r = base("c7ee7d76-2113-40bd-9f8c-a28cd6ca395f", Some("40894092-7ec5-4693-94f0-afb1c7fb51c4"));
    r.name = None;
    r.preferred_username = None;
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.display_name, "user-c7ee7d76");
}

#[test]
fn aud_claim_handles_single_and_multi() {
    assert!(AudClaim::One("x".into()).contains("x"));
    assert!(!AudClaim::One("x".into()).contains("y"));
    assert!(AudClaim::Many(vec!["a".into(), "b".into()]).contains("b"));
    assert!(!AudClaim::Empty.contains("anything"));
}

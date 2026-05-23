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
        govbr_cpf_hash: None,
        govbr_confiabilidades: None,
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

#[test]
fn roles_deduplicated_across_realm_and_resource() {
    let r = base(
        "c7ee7d76-2113-40bd-9f8c-a28cd6ca395f",
        Some("40894092-7ec5-4693-94f0-afb1c7fb51c4"),
    );
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    // "admin" appears in both realm_access and resource_access["expresso-web"]
    assert_eq!(ctx.roles.iter().filter(|r| *r == "admin").count(), 1);
}

#[test]
fn malformed_tenant_uuid_fails() {
    let mut r = base("c7ee7d76-2113-40bd-9f8c-a28cd6ca395f", None);
    r.tenant_id = Some("not-a-uuid".into());
    match AuthContext::from_raw(r, "expresso-web") {
        Err(AuthError::MalformedClaim("tenant_id", _)) => {}
        other => panic!("expected MalformedClaim(tenant_id), got {other:?}"),
    }
}

#[test]
fn nil_sub_uuid_is_valid() {
    let r = base("00000000-0000-0000-0000-000000000000", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.user_id, uuid::Uuid::nil());
}

#[test]
fn no_tenant_id_produces_none() {
    let r = base("c7ee7d76-2113-40bd-9f8c-a28cd6ca395f", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert!(ctx.tenant_id.is_none());
}

#[test]
fn user_id_matches_sub() {
    let sub = "a1b2c3d4-0000-0000-0000-000000000001";
    let r = base(sub, None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.user_id.to_string(), sub);
}

#[test]
fn email_is_preserved_in_context() {
    let r = base("a1b2c3d4-0000-0000-0000-000000000002", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.email, "alice@x");
}

#[test]
fn roles_from_realm_access_included() {
    let r = base("a1b2c3d4-0000-0000-0000-000000000003", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert!(ctx.roles.iter().any(|r| r == "user" || r == "admin"));
}

#[test]
fn display_name_is_non_empty() {
    let r = base("a1b2c3d4-0000-0000-0000-000000000004", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert!(!ctx.display_name.is_empty());
}

#[test]
fn roles_is_empty_when_no_roles_in_claims() {
    let mut r = base("a1b2c3d4-0000-0000-0000-000000000005", None);
    r.realm_access = None;
    r.resource_access = None;
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert!(ctx.roles.is_empty());
}

#[test]
fn expires_at_is_preserved_in_context() {
    let r = base("a1b2c3d4-0000-0000-0000-000000000006", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.expires_at, 9_999_999_999);
}

#[test]
fn email_is_preserved_in_context() {
    let r = base("a1b2c3d4-0000-0000-0000-000000000007", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.email, "alice@x");
}

#[test]
fn has_role_false_when_role_absent() {
    let r = base("a1b2c3d4-0000-0000-0000-000000000008", Some("40894092-7ec5-4693-94f0-afb1c7fb51c4"));
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert!(!ctx.has_role("nonexistent_role"));
}

#[test]
fn expires_at_matches_raw_exp() {
    let r = base("a1b2c3d4-0000-0000-0000-000000000009", None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.expires_at, 9_999_999_999);
}

#[test]
fn aud_claim_empty_never_contains_empty_str() {
    assert!(!AudClaim::Empty.contains(""));
}

#[test]
fn from_raw_govbr_confiabilidades_defaults_empty_when_none() {
    let uid = uuid::Uuid::new_v4();
    let mut r = base(&uid.to_string(), None);
    r.govbr_confiabilidades = None;
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert!(ctx.govbr_confiabilidades.is_empty());
}

#[test]
fn from_raw_sub_is_preserved_in_context() {
    let sub = uuid::Uuid::new_v4().to_string();
    let r = base(&sub, None);
    let ctx = AuthContext::from_raw(r, "expresso-web").unwrap();
    assert_eq!(ctx.sub, sub);
}

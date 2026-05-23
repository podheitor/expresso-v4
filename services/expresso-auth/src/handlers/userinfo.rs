//! GET /auth/userinfo — standard OIDC UserInfo endpoint.

use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize, PartialEq)]
pub struct UserInfo {
    pub sub:          String,
    pub email:        String,
    pub name:         String,
    pub tenant_id:    Uuid,
    pub roles:        Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn userinfo_sub_round_trips_through_serde() {
        let id = Uuid::nil();
        let u = UserInfo {
            sub: id.to_string(),
            email: "a@b.com".into(),
            name: "Alice".into(),
            tenant_id: id,
            roles: vec![],
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(s.contains(&id.to_string()));
    }

    #[test]
    fn userinfo_email_serialized() {
        let u = UserInfo {
            sub: "s".into(),
            email: "user@example.com".into(),
            name: "X".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(s.contains("user@example.com"));
    }

    #[test]
    fn userinfo_name_serialized() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "Bob Smith".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(s.contains("Bob Smith"));
    }

    #[test]
    fn userinfo_roles_empty_serializes_as_array() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(s.contains("\"roles\":[]"));
    }

    #[test]
    fn userinfo_roles_non_empty_serialized() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec!["admin".into()],
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(s.contains("admin"));
    }

    #[test]
    fn userinfo_tenant_id_is_nil_uuid() {
        let id = Uuid::nil();
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: id,
            roles: vec![],
        };
        assert_eq!(u.tenant_id, id);
    }

    #[test]
    fn userinfo_sub_field_accessible() {
        let u = UserInfo {
            sub: "sub-value".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        assert_eq!(u.sub, "sub-value");
    }

    #[test]
    fn userinfo_debug_format_contains_email() {
        let u = UserInfo {
            sub: "s".into(),
            email: "debug@test.com".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        assert!(format!("{u:?}").contains("debug@test.com"));
    }

    #[test]
    fn userinfo_roles_multiple_preserved() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec!["admin".into(), "user".into()],
        };
        assert_eq!(u.roles.len(), 2);
    }

    #[test]
    fn userinfo_name_field_accessible() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "Carol".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        assert_eq!(u.name, "Carol");
    }

    #[test]
    fn userinfo_email_field_accessible() {
        let u = UserInfo {
            sub: "s".into(),
            email: "test@domain.org".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        assert_eq!(u.email, "test@domain.org");
    }

    #[test]
    fn userinfo_serde_json_contains_sub_key() {
        let u = UserInfo {
            sub: "abc123".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&u).unwrap();
        assert!(v["sub"].as_str().is_some());
    }

    #[test]
    fn userinfo_serde_json_contains_email_key() {
        let u = UserInfo {
            sub: "s".into(),
            email: "x@y.z".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&u).unwrap();
        assert_eq!(v["email"], "x@y.z");
    }

    #[test]
    fn userinfo_serde_json_contains_name_key() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "Dave".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&u).unwrap();
        assert_eq!(v["name"], "Dave");
    }

    #[test]
    fn userinfo_serde_json_roles_is_array() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec!["viewer".into()],
        };
        let v: serde_json::Value = serde_json::to_value(&u).unwrap();
        assert!(v["roles"].is_array());
    }

    #[test]
    fn userinfo_serde_json_tenant_id_is_string() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&u).unwrap();
        assert!(v["tenant_id"].as_str().is_some());
    }

    #[test]
    fn userinfo_equality_same_values() {
        let id = Uuid::nil();
        let a = UserInfo { sub: "s".into(), email: "e".into(), name: "n".into(), tenant_id: id, roles: vec![] };
        let b = UserInfo { sub: "s".into(), email: "e".into(), name: "n".into(), tenant_id: id, roles: vec![] };
        assert_eq!(a, b);
    }

    #[test]
    fn userinfo_equality_different_email_not_equal() {
        let id = Uuid::nil();
        let a = UserInfo { sub: "s".into(), email: "a@x.com".into(), name: "n".into(), tenant_id: id, roles: vec![] };
        let b = UserInfo { sub: "s".into(), email: "b@x.com".into(), name: "n".into(), tenant_id: id, roles: vec![] };
        assert_ne!(a, b);
    }

    #[test]
    fn userinfo_roles_first_element_accessible() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec!["superuser".into()],
        };
        assert_eq!(u.roles[0], "superuser");
    }

    #[test]
    fn userinfo_sub_is_not_empty_when_set() {
        let u = UserInfo {
            sub: "nonempty-sub".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        assert!(!u.sub.is_empty());
    }

    #[test]
    fn userinfo_name_is_not_empty_when_set() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "Full Name".into(),
            tenant_id: Uuid::nil(),
            roles: vec![],
        };
        assert!(!u.name.is_empty());
    }

    #[test]
    fn userinfo_roles_count_matches() {
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: Uuid::nil(),
            roles: vec!["r1".into(), "r2".into(), "r3".into()],
        };
        assert_eq!(u.roles.len(), 3);
    }

    #[test]
    fn userinfo_tenant_id_round_trips() {
        let id = Uuid::new_v4();
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: id,
            roles: vec![],
        };
        assert_eq!(u.tenant_id, id);
    }

    #[test]
    fn userinfo_serde_tenant_id_matches_input() {
        let id = Uuid::nil();
        let u = UserInfo {
            sub: "s".into(),
            email: "e".into(),
            name: "n".into(),
            tenant_id: id,
            roles: vec![],
        };
        let v: serde_json::Value = serde_json::to_value(&u).unwrap();
        assert_eq!(v["tenant_id"].as_str().unwrap(), id.to_string());
    }
}

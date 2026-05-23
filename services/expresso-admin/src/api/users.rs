//! Admin user management — list, create, update, delete users in a tenant.
//!
//! GET    /api/admin/tenants/:tid/users        — list users
//! POST   /api/admin/tenants/:tid/users        — create user
//! GET    /api/admin/tenants/:tid/users/:uid   — get user
//! PATCH  /api/admin/tenants/:tid/users/:uid   — update user
//! DELETE /api/admin/tenants/:tid/users/:uid   — delete user

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_USERNAME_BYTES: usize = 64;
pub const MAX_EMAIL_BYTES:    usize = 254;
pub const DEFAULT_PAGE_SIZE:  i64   = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Disabled,
    Pending,
}

impl UserStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active   => "active",
            Self::Disabled => "disabled",
            Self::Pending  => "pending",
        }
    }
}

impl std::fmt::Display for UserStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminUser {
    pub id:        Uuid,
    pub tenant_id: Uuid,
    pub username:  String,
    pub email:     String,
    pub status:    UserStatus,
    pub roles:     Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserBody {
    pub username:  String,
    pub email:     String,
    pub password:  Option<String>,
    pub roles:     Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserBody {
    pub status: Option<UserStatus>,
    pub roles:  Option<Vec<String>>,
    pub email:  Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_username_bytes_constant() {
        assert_eq!(MAX_USERNAME_BYTES, 64);
    }

    #[test]
    fn max_email_bytes_constant() {
        assert_eq!(MAX_EMAIL_BYTES, 254);
    }

    #[test]
    fn default_page_size_constant() {
        assert_eq!(DEFAULT_PAGE_SIZE, 50);
    }

    #[test]
    fn status_active_as_str() {
        assert_eq!(UserStatus::Active.as_str(), "active");
    }

    #[test]
    fn status_disabled_as_str() {
        assert_eq!(UserStatus::Disabled.as_str(), "disabled");
    }

    #[test]
    fn status_pending_as_str() {
        assert_eq!(UserStatus::Pending.as_str(), "pending");
    }

    #[test]
    fn status_display_active() {
        assert_eq!(format!("{}", UserStatus::Active), "active");
    }

    #[test]
    fn status_equality() {
        assert_eq!(UserStatus::Active, UserStatus::Active);
        assert_ne!(UserStatus::Active, UserStatus::Disabled);
    }

    #[test]
    fn status_serde_roundtrip_active() {
        let s = serde_json::to_string(&UserStatus::Active).unwrap();
        let back: UserStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, UserStatus::Active);
    }

    #[test]
    fn status_serde_roundtrip_disabled() {
        let s = serde_json::to_string(&UserStatus::Disabled).unwrap();
        let back: UserStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, UserStatus::Disabled);
    }

    #[test]
    fn admin_user_serde_roundtrip() {
        let u = AdminUser {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            username: "jdoe".into(), email: "jdoe@example.com".into(),
            status: UserStatus::Active, roles: vec!["user".into()],
        };
        let s = serde_json::to_string(&u).unwrap();
        let back: AdminUser = serde_json::from_str(&s).unwrap();
        assert_eq!(back.username, "jdoe");
        assert_eq!(back.status, UserStatus::Active);
    }

    #[test]
    fn create_user_body_password_optional() {
        let b: CreateUserBody = serde_json::from_str(
            r#"{"username":"alice","email":"alice@example.com"}"#,
        ).unwrap();
        assert!(b.password.is_none());
    }

    #[test]
    fn create_user_body_roles_optional() {
        let b: CreateUserBody = serde_json::from_str(
            r#"{"username":"bob","email":"bob@example.com"}"#,
        ).unwrap();
        assert!(b.roles.is_none());
    }

    #[test]
    fn update_user_body_all_optional() {
        let b: UpdateUserBody = serde_json::from_str(r#"{}"#).unwrap();
        assert!(b.status.is_none());
        assert!(b.roles.is_none());
        assert!(b.email.is_none());
    }

    #[test]
    fn update_user_body_status_disabled() {
        let b: UpdateUserBody =
            serde_json::from_str(r#"{"status":"disabled"}"#).unwrap();
        assert_eq!(b.status, Some(UserStatus::Disabled));
    }

    #[test]
    fn admin_user_clone_preserves_email() {
        let u = AdminUser {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            username: "user".into(), email: "user@test.com".into(),
            status: UserStatus::Active, roles: vec![],
        };
        assert_eq!(u.clone().email, "user@test.com");
    }

    #[test]
    fn status_copy_trait() {
        let s = UserStatus::Pending;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn admin_user_roles_field_accessible() {
        let u = AdminUser {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            username: "u".into(), email: "u@e.com".into(),
            status: UserStatus::Active, roles: vec!["admin".into()],
        };
        assert_eq!(u.roles.len(), 1);
        assert_eq!(u.roles[0], "admin");
    }

    #[test]
    fn status_debug_contains_variant() {
        let s = format!("{:?}", UserStatus::Pending);
        assert!(s.contains("Pending"));
    }

    #[test]
    fn default_page_size_positive() {
        assert!(DEFAULT_PAGE_SIZE > 0);
    }

    #[test]
    fn admin_user_json_contains_status_key() {
        let u = AdminUser {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            username: "u".into(), email: "u@e.com".into(),
            status: UserStatus::Disabled, roles: vec![],
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(s.contains("status"));
    }

    #[test]
    fn create_user_body_username_preserved() {
        let b: CreateUserBody = serde_json::from_str(
            r#"{"username":"testuser","email":"t@e.com"}"#,
        ).unwrap();
        assert_eq!(b.username, "testuser");
    }

    #[test]
    fn status_serde_roundtrip_pending() {
        let s = serde_json::to_string(&UserStatus::Pending).unwrap();
        let back: UserStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, UserStatus::Pending);
    }

    #[test]
    fn admin_user_email_preserved() {
        let u = AdminUser {
            id: Uuid::nil(), tenant_id: Uuid::nil(),
            username: "u".into(), email: "admin@corp.com".into(),
            status: UserStatus::Active, roles: vec![],
        };
        assert_eq!(u.email, "admin@corp.com");
    }

    #[test]
    fn status_display_disabled() {
        assert_eq!(format!("{}", UserStatus::Disabled), "disabled");
    }

    #[test]
    fn status_display_pending() {
        assert_eq!(format!("{}", UserStatus::Pending), "pending");
    }
}

//! Askama template structs for admin SSR.

use askama::Template;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct ServiceRow {
    pub name: &'static str,
    pub port: u16,
    pub role: &'static str,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTpl {
    pub current: &'static str,
    pub user_count: usize,
    pub realm_name: String,
    pub service_count: usize,
    pub services: Vec<ServiceRow>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct KcUser {
    pub id:        String,
    #[serde(default)]
    pub username:  String,
    #[serde(default)]
    pub email:     String,
    #[serde(rename = "firstName", default)]
    pub first:     String,
    #[serde(rename = "lastName",  default)]
    pub last:      String,
    #[serde(default)]
    pub enabled:   bool,
}

pub struct UserRow {
    pub id:        String,
    pub username:  String,
    pub email:     String,
    pub full_name: String,
    pub enabled:   bool,
}

#[derive(Template)]
#[template(path = "users.html")]
pub struct UsersTpl {
    pub current: &'static str,
    pub realm_name: String,
    pub users:      Vec<UserRow>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct KcRealm {
    pub realm:        String,
    #[serde(rename = "displayName", default)]
    pub display_name: String,
    #[serde(default)]
    pub enabled:      bool,
    #[serde(rename = "sslRequired", default)]
    pub ssl_required: String,
    #[serde(rename = "accessTokenLifespan", default)]
    pub access_token_lifespan: i64,
    #[serde(rename = "registrationAllowed", default)]
    pub registration_allowed: bool,
    #[serde(rename = "passwordPolicy", default)]
    pub password_policy: String,
}

#[derive(Template)]
#[template(path = "realm.html")]
pub struct RealmTpl {
    pub current: &'static str,
    pub realm: KcRealm,
}

#[derive(Template)]
#[template(path = "user_form.html")]
pub struct UserFormTpl {
    pub current:    &'static str,
    pub user_id:    Option<String>,
    pub username:   String,
    pub email:      String,
    pub first_name: String,
    pub last_name:  String,
    pub enabled:    bool,
    pub error:      Option<String>,
}

// ─── DAV admin (calendars / addressbooks across tenants) ────────────────────

#[derive(Debug, Clone)]
pub struct DavRow {
    pub id:           String,
    pub tenant_id:    String,
    pub tenant_name:  String,
    pub owner_email:  String,
    pub name:         String,
    pub description:  String,
    pub color:        String,
    pub is_default:   bool,
    pub ctag:         i64,
}

#[derive(Template)]
#[template(path = "calendars_admin.html")]
pub struct CalendarsAdminTpl {
    pub current: &'static str,
    pub rows:    Vec<DavRow>,
}

#[derive(Template)]
#[template(path = "addressbooks_admin.html")]
pub struct AddressbooksAdminTpl {
    pub current: &'static str,
    pub rows:    Vec<DavRow>,
}

#[derive(Template)]
#[template(path = "calendar_admin_edit.html")]
pub struct CalendarAdminEditTpl {
    pub current:     &'static str,
    pub tenant_id:   String,
    pub id:          String,
    pub tenant_name: String,
    pub owner_email: String,
    pub name:        String,
    pub description: String,
    pub color:       String,
    pub is_default:  bool,
    pub error:       Option<String>,
}

#[derive(Template)]
#[template(path = "addressbook_admin_edit.html")]
pub struct AddressbookAdminEditTpl {
    pub current:     &'static str,
    pub tenant_id:   String,
    pub id:          String,
    pub tenant_name: String,
    pub owner_email: String,
    pub name:        String,
    pub description: String,
    pub error:       Option<String>,
}

// ─── Tenants admin (super_admin only) ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TenantRow {
    pub id:         String,
    pub slug:       String,
    pub name:       String,
    pub cnpj:       String,
    pub plan:       String,
    pub status:     String,
    pub user_count: i64,
}

#[derive(Template)]
#[template(path = "tenants_admin.html")]
pub struct TenantsAdminTpl {
    pub current: &'static str,
    pub rows:    Vec<TenantRow>,
    pub flash:   Option<String>,
}

#[derive(Template)]
#[template(path = "tenant_admin_edit.html")]
pub struct TenantAdminEditTpl {
    pub current:   &'static str,
    pub id:        Option<String>, // None = create
    pub slug:      String,
    pub name:      String,
    pub cnpj:      String,
    pub plan:      String,
    pub status:    String,
    pub error:     Option<String>,
}


#[derive(Template)]
#[template(path = "audit_admin.html")]
pub struct AuditAdminTpl {
    pub current:         &'static str,
    pub rows:            Vec<AuditViewRow>,
    pub limit:           i64,
    pub action_prefix_v: String,
    pub tenant_id_v:     String,
    pub preset_v:        String,
    pub since_v:         String,
    pub until_v:         String,
    pub query_string:    String,
    pub next_href:       Option<String>,
    pub reset_href:      String,
    pub has_cursor:      bool,
    pub error:           Option<String>,
}

pub struct AuditViewRow {
    pub id:            i64,
    pub created_at_fmt: String,
    pub tenant_id:     String,
    pub user_id:       Option<String>,
    pub action:        String,
    pub resource:      Option<String>,
    pub status:        String,
    pub metadata_json: String,
}


#[derive(Template)]
#[template(path = "tenant_admin_config.html")]
pub struct TenantConfigTpl {
    pub current:     &'static str,
    pub id:          String,
    pub slug:        String,
    pub name:        String,
    pub config_json: String,
    pub error:       Option<String>,
    pub flash:       Option<String>,
}


#[derive(Template)]
#[template(path = "tenant_wizard.html")]
pub struct TenantWizardTpl {
    pub current:     &'static str,
    pub slug:        String,
    pub name:        String,
    pub plan:        String,
    pub admin_email: String,
    pub admin_user:  String,
    pub error:       Option<String>,
    pub success:     Option<String>,
}

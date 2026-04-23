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

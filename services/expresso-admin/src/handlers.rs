//! SSR handlers for admin UI.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    kc::{KcClient, NewUser, UpdateUser},
    templates::{DashboardTpl, RealmTpl, ServiceRow, UserFormTpl, UserRow, UsersTpl},
    AdminError, AppState,
};

const SERVICES: &[ServiceRow] = &[
    ServiceRow { name: "expresso-web",      port: 8090, role: "SSR / UI"        },
    ServiceRow { name: "expresso-auth",     port: 8012, role: "OIDC / Auth RP"  },
    ServiceRow { name: "expresso-admin",    port: 8101, role: "Admin UI (este)" },
    ServiceRow { name: "expresso-mail",     port: 8001, role: "Mail API"        },
    ServiceRow { name: "expresso-calendar", port: 8002, role: "Calendar + CalDAV" },
    ServiceRow { name: "expresso-contacts", port: 8003, role: "Contacts + CardDAV"},
    ServiceRow { name: "expresso-drive",    port: 8004, role: "Drive + TUS + WOPI" },
    ServiceRow { name: "expresso-chat",     port: 8010, role: "Chat (Matrix)"   },
    ServiceRow { name: "expresso-meet",     port: 8011, role: "Meet (Jitsi)"    },
    ServiceRow { name: "keycloak",          port: 8080, role: "IdP"             },
];

pub async fn dashboard(State(st): State<Arc<AppState>>) -> Result<impl IntoResponse, AdminError> {
    let users = st.kc.users().await?;
    let realm = st.kc.realm().await?;
    Ok(DashboardTpl {
        current: "dashboard",
        user_count:    users.len(),
        realm_name:    realm.realm,
        service_count: SERVICES.len(),
        services:      SERVICES.to_vec(),
    })
}

pub async fn users(State(st): State<Arc<AppState>>) -> Result<impl IntoResponse, AdminError> {
    let kcu = st.kc.users().await?;
    let realm = st.kc.realm().await?;
    let rows = kcu.into_iter().map(|u| {
        let full = format!("{} {}", u.first, u.last).trim().to_string();
        UserRow {
            id:        u.id,
            username:  u.username,
            email:     u.email,
            full_name: full,
            enabled:   u.enabled,
        }
    }).collect();
    Ok(UsersTpl { current: "users", realm_name: realm.realm, users: rows })
}

pub async fn realm_page(State(st): State<Arc<AppState>>) -> Result<impl IntoResponse, AdminError> {
    let realm = st.kc.realm().await?;
    Ok(RealmTpl { current: "realm", realm })
}

// ---- User CRUD ----

pub async fn user_new() -> impl IntoResponse {
    UserFormTpl {
        current: "users",
        user_id: None,
        username: String::new(),
        email: String::new(),
        first_name: String::new(),
        last_name: String::new(),
        enabled: true,
        error: None,
    }
}

pub async fn user_edit(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AdminError> {
    let u = st.kc.user(&id).await?;
    Ok(UserFormTpl {
        current: "users",
        user_id:    Some(u.id),
        username:   u.username,
        email:      u.email,
        first_name: u.first,
        last_name:  u.last,
        enabled:    u.enabled,
        error:      None,
    })
}

#[derive(Deserialize)]
pub struct UserCreateForm {
    pub username:   String,
    pub email:      String,
    pub first_name: String,
    pub last_name:  String,
    pub password:   String,
    #[serde(default)]
    pub enabled:    Option<String>,
    #[serde(default)]
    pub temporary:  Option<String>,
}

pub async fn user_create(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Form(f): Form<UserCreateForm>,
) -> Result<Redirect, AdminError> {
    let nu = NewUser {
        username:   f.username.trim().to_string(),
        email:      f.email.trim().to_string(),
        first_name: f.first_name.trim().to_string(),
        last_name:  f.last_name.trim().to_string(),
        enabled:    f.enabled.is_some(),
        password:   f.password,
        temporary:  f.temporary.is_some(),
    };
    let created = st.kc.create_user(&nu).await?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST, "/users/new",
        "admin.user.create", Some("user"), Some(created.clone()), Some(302),
        serde_json::json!({ "username": nu.username, "email": nu.email, "enabled": nu.enabled }),
    ).await;
    let _ = created;
    Ok(Redirect::to("/users"))
}

#[derive(Deserialize)]
pub struct UserUpdateForm {
    pub email:      String,
    pub first_name: String,
    pub last_name:  String,
    #[serde(default)]
    pub enabled:    Option<String>,
    #[serde(default)]
    pub password:   Option<String>,
    #[serde(default)]
    pub temporary:  Option<String>,
}

pub async fn user_update(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<UserUpdateForm>,
) -> Result<Redirect, AdminError> {
    let patch = UpdateUser {
        email:      Some(f.email.trim().to_string()),
        first_name: Some(f.first_name.trim().to_string()),
        last_name:  Some(f.last_name.trim().to_string()),
        enabled:    Some(f.enabled.is_some()),
    };
    st.kc.update_user(&id, &patch).await?;
    let pw_changed = f.password.as_ref().map(|s| !s.is_empty()).unwrap_or(false);
    if let Some(pw) = f.password.as_ref().filter(|s| !s.is_empty()) {
        st.kc.set_password(&id, pw, f.temporary.is_some()).await?;
    }
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST, &format!("/users/{id}/edit"),
        "admin.user.update", Some("user"), Some(id.clone()), Some(302),
        serde_json::json!({ "email": f.email, "enabled": f.enabled.is_some(), "password_changed": pw_changed }),
    ).await;
    Ok(Redirect::to("/users"))
}

pub async fn user_delete(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, AdminError> {
    st.kc.delete_user(&id).await?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST, &format!("/users/{id}/delete"),
        "admin.user.delete", Some("user"), Some(id.clone()), Some(302),
        serde_json::json!({}),
    ).await;
    Ok(Redirect::to("/users"))
}

pub async fn user_totp_enroll(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, AdminError> {
    st.kc.enroll_totp(&id).await?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST, &format!("/users/{id}/totp/enroll"),
        "admin.user.totp.enroll", Some("user"), Some(id.clone()), Some(302),
        serde_json::json!({}),
    ).await;
    Ok(Redirect::to("/users"))
}

pub async fn user_totp_reset(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Result<Redirect, AdminError> {
    let removed = st.kc.reset_totp(&id).await?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST, &format!("/users/{id}/totp/reset"),
        "admin.user.totp.reset", Some("user"), Some(id.clone()), Some(302),
        serde_json::json!({"removed": removed}),
    ).await;
    Ok(Redirect::to("/users"))
}

/// GET /users/totp-status — coverage report for ops.
///
/// Lists all realm users + whether each has TOTP enrolled. Useful
/// before flipping `ADMIN_REQUIRE_2FA=true` in prod (sprint #29).
///
/// Performance: 1 + N HTTP calls to KC (N = user count). Good enough
/// for realms <500 users; paginate/parallelize if that changes.
pub async fn users_totp_status(
    State(st): State<Arc<AppState>>,
) -> Result<axum::response::Response, AdminError> {
    use axum::response::IntoResponse;
    let users = st.kc.users().await?;
    let mut rows = String::new();
    let mut with_totp = 0u32;
    fn esc(s: &str) -> String {
        s.replace('&', "&amp;")
         .replace('<', "&lt;")
         .replace('>', "&gt;")
         .replace('"', "&quot;")
    }
    for u in &users {
        let has = st.kc.user_has_totp(&u.id).await.unwrap_or(false);
        if has { with_totp += 1; }
        let full = format!("{} {}", u.first, u.last).trim().to_string();
        let badge = if has {
            "<span style=color:#1a7f37>✓ TOTP</span>"
        } else {
            "<span style=color:#b42318>✗ sem TOTP</span>"
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&u.username),
            esc(&u.email),
            esc(&full),
            if u.enabled { "ativo" } else { "desabilitado" },
            badge,
        ));
    }
    let total = users.len() as u32;
    let pct = if total > 0 { (with_totp * 100) / total } else { 0 };
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>Cobertura TOTP</title>        <style>body{{font-family:system-ui;padding:2rem;max-width:60rem;margin:auto}}        table{{width:100%;border-collapse:collapse}}th,td{{padding:.4rem .6rem;border-bottom:1px solid #eee;text-align:left}}        .sum{{background:#f6f8fa;padding:1rem;border-radius:.5rem;margin:1rem 0}}</style>        <h1>Cobertura TOTP</h1>        <div class=sum><strong>{with_totp}</strong> de <strong>{total}</strong> usuários têm TOTP cadastrado ({pct}%).</div>        <p><a href=\"/users\">← Voltar para usuários</a></p>        <table><thead><tr><th>Username</th><th>Email</th><th>Nome</th><th>Status</th><th>TOTP</th></tr></thead>        <tbody>{rows}</tbody></table>"
    );
    Ok((
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    ).into_response())
}

pub fn kc_factory() -> KcClient { KcClient::new(crate::kc::KcConfig::from_env()) }

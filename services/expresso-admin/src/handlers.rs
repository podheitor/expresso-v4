//! SSR handlers for admin UI.

use axum::{extract::State, response::IntoResponse};
use std::sync::Arc;

use crate::{
    kc::KcClient,
    templates::{DashboardTpl, RealmTpl, ServiceRow, UserRow, UsersTpl},
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

pub fn kc_factory() -> KcClient { KcClient::new(crate::kc::KcConfig::from_env()) }

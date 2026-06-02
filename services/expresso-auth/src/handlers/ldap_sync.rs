//! Internal LDAP user-federation reflection endpoints.
//!
//! The admin service owns `ldap_config` rows; after a CRUD write it calls these
//! LAN-trusted `/internal/*` endpoints so the Keycloak realm's LDAP
//! UserStorageProvider component is created/updated/removed to match. Keycloak
//! does the directory bind/search/sync; the Rust side only pushes the
//! registration via [`KcAdmin`]. The bind credential travels in the sync body
//! and is written straight into the KC component — it is never persisted in our
//! DB (KC is the source of truth for the secret).
//!
//! Same trust model as `/internal/tokens/introspect`: reached over the internal
//! network, no end-user auth. 503 when KC admin is unconfigured (then
//! `deploy/keycloak/seed-realm.sh` §13 is the provisioning path).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::kc_admin::{KcAdmin, KcAdminConfig, LdapComponentSpec};
use crate::state::AppState;

fn kc_admin() -> Result<KcAdmin, StatusCode> {
    KcAdminConfig::from_env()
        .map(KcAdmin::new)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// Reflect an LDAP config into a tenant's realm. `realm` = tenant_id UUID.
#[derive(Debug, Deserialize)]
pub struct LdapSyncBody {
    pub realm: String,
    pub name: String,
    pub vendor: Option<String>,
    pub connection_url: String,
    pub users_dn: String,
    pub bind_dn: String,
    /// Bind-account password — written through to the KC component, not stored.
    pub bind_credential: String,
    pub username_attr: Option<String>,
    pub rdn_attr: Option<String>,
    pub uuid_attr: Option<String>,
    pub user_object_classes: Option<String>,
    pub search_scope: Option<i32>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct LdapDeleteBody {
    pub realm: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct SyncResp {
    pub ok: bool,
    pub name: String,
}

/// POST /internal/ldap/sync — create/update the realm's LDAP component.
pub async fn sync(
    State(_st): State<Arc<AppState>>,
    Json(body): Json<LdapSyncBody>,
) -> Result<Json<SyncResp>, StatusCode> {
    if body.realm.trim().is_empty() || body.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.bind_credential.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let kc = kc_admin()?;
    let spec = LdapComponentSpec {
        name: body.name.clone(),
        vendor: body.vendor.unwrap_or_else(|| "other".to_string()),
        connection_url: body.connection_url,
        users_dn: body.users_dn,
        bind_dn: body.bind_dn,
        bind_credential: body.bind_credential,
        username_attr: body.username_attr.unwrap_or_else(|| "uid".to_string()),
        rdn_attr: body.rdn_attr.unwrap_or_else(|| "uid".to_string()),
        uuid_attr: body.uuid_attr.unwrap_or_else(|| "entryUUID".to_string()),
        user_object_classes: body
            .user_object_classes
            .unwrap_or_else(|| "inetOrgPerson, organizationalPerson".to_string()),
        search_scope: body.search_scope.unwrap_or(2),
        enabled: body.enabled.unwrap_or(true),
    };
    kc.upsert_ldap_component(&body.realm, &spec)
        .await
        .map_err(|e| {
            tracing::warn!(error=%e, realm=%body.realm, name=%body.name, "kc ldap sync failed");
            StatusCode::BAD_GATEWAY
        })?;
    tracing::info!(target: "audit", event = "ldap.component.kc_sync", realm = %body.realm, name = %body.name);
    Ok(Json(SyncResp {
        ok: true,
        name: body.name,
    }))
}

/// POST /internal/ldap/remove — remove the realm's LDAP component.
pub async fn remove(
    State(_st): State<Arc<AppState>>,
    Json(body): Json<LdapDeleteBody>,
) -> Result<Json<SyncResp>, StatusCode> {
    if body.realm.trim().is_empty() || body.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let kc = kc_admin()?;
    kc.delete_ldap_component(&body.realm, &body.name)
        .await
        .map_err(|e| {
            tracing::warn!(error=%e, realm=%body.realm, name=%body.name, "kc ldap delete failed");
            StatusCode::BAD_GATEWAY
        })?;
    tracing::info!(target: "audit", event = "ldap.component.kc_delete", realm = %body.realm, name = %body.name);
    Ok(Json(SyncResp {
        ok: true,
        name: body.name,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_body_deser_minimal() {
        let json = r#"{"realm":"r","name":"corp-ad","connection_url":"ldaps://ad",
                       "users_dn":"dc=x","bind_dn":"cn=svc","bind_credential":"pw"}"#;
        let b: LdapSyncBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.name, "corp-ad");
        assert_eq!(b.bind_credential, "pw");
        assert!(b.vendor.is_none());
        assert!(b.search_scope.is_none());
    }

    #[test]
    fn delete_body_deser() {
        let b: LdapDeleteBody = serde_json::from_str(r#"{"realm":"r","name":"ad"}"#).unwrap();
        assert_eq!(b.realm, "r");
        assert_eq!(b.name, "ad");
    }

    #[test]
    fn sync_resp_serializes() {
        let j = serde_json::to_string(&SyncResp {
            ok: true,
            name: "corp-ad".into(),
        })
        .unwrap();
        assert!(j.contains("\"ok\":true"));
        assert!(j.contains("corp-ad"));
    }
}

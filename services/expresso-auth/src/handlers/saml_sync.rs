//! Internal SAML IdP reflection endpoints.
//!
//! The admin service owns `saml_idp_config` rows; after a CRUD write it calls
//! these LAN-trusted `/internal/*` endpoints so the Keycloak realm's
//! identity-provider instance is created/updated/removed to match. Keycloak then
//! brokers the SAML assertion (parsing + signature verification) — the Rust side
//! only pushes the broker registration via [`KcAdmin`].
//!
//! Same trust model as `/internal/tokens/introspect`: reached over the internal
//! network, no end-user auth. When Keycloak admin is not configured
//! (`KC_URL`/`KC_ADMIN_*` unset) the endpoints return 503 — provisioning then
//! happens via `deploy/keycloak/seed-realm.sh` instead.

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::kc_admin::{KcAdmin, KcAdminConfig, SamlIdpSpec};
use crate::state::AppState;

const DEFAULT_NAME_ID_FORMAT: &str = "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress";

fn kc_admin() -> Result<KcAdmin, StatusCode> {
    KcAdminConfig::from_env()
        .map(KcAdmin::new)
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)
}

/// Reflect a SAML IdP into a tenant's realm. `realm` is the tenant's realm name
/// (= tenant_id UUID, realm-per-tenant).
#[derive(Debug, Deserialize)]
pub struct IdpSyncBody {
    pub realm: String,
    pub alias: String,
    pub display_name: String,
    pub entity_id: String,
    pub sso_url: String,
    pub slo_url: Option<String>,
    pub signing_cert: String,
    pub name_id_format: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct IdpDeleteBody {
    pub realm: String,
    pub alias: String,
}

#[derive(Debug, Serialize)]
pub struct SyncResp {
    pub ok: bool,
    pub alias: String,
}

/// POST /internal/saml/idp-sync — create/update the realm's SAML IdP instance.
pub async fn sync(
    State(_st): State<Arc<AppState>>,
    Json(body): Json<IdpSyncBody>,
) -> Result<Json<SyncResp>, StatusCode> {
    if body.realm.trim().is_empty() || body.alias.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let kc = kc_admin()?;
    let spec = SamlIdpSpec {
        alias: body.alias.clone(),
        display_name: body.display_name,
        entity_id: body.entity_id,
        sso_url: body.sso_url,
        slo_url: body.slo_url,
        signing_cert: body.signing_cert,
        name_id_format: body
            .name_id_format
            .unwrap_or_else(|| DEFAULT_NAME_ID_FORMAT.to_string()),
        enabled: body.enabled.unwrap_or(true),
    };
    kc.upsert_saml_idp(&body.realm, &spec).await.map_err(|e| {
        tracing::warn!(error=%e, realm=%body.realm, alias=%body.alias, "kc saml-idp sync failed");
        StatusCode::BAD_GATEWAY
    })?;
    tracing::info!(target: "audit", event = "saml.idp.kc_sync", realm = %body.realm, alias = %body.alias);
    Ok(Json(SyncResp {
        ok: true,
        alias: body.alias,
    }))
}

/// POST /internal/saml/idp-delete — remove the realm's SAML IdP instance.
pub async fn remove(
    State(_st): State<Arc<AppState>>,
    Json(body): Json<IdpDeleteBody>,
) -> Result<Json<SyncResp>, StatusCode> {
    if body.realm.trim().is_empty() || body.alias.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let kc = kc_admin()?;
    kc.delete_saml_idp(&body.realm, &body.alias)
        .await
        .map_err(|e| {
            tracing::warn!(error=%e, realm=%body.realm, alias=%body.alias, "kc saml-idp delete failed");
            StatusCode::BAD_GATEWAY
        })?;
    tracing::info!(target: "audit", event = "saml.idp.kc_delete", realm = %body.realm, alias = %body.alias);
    Ok(Json(SyncResp {
        ok: true,
        alias: body.alias,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_body_deser_minimal() {
        let json = r#"{"realm":"r","alias":"okta","display_name":"Okta",
                       "entity_id":"e","sso_url":"s","signing_cert":"c"}"#;
        let b: IdpSyncBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.alias, "okta");
        assert!(b.slo_url.is_none());
        assert!(b.enabled.is_none());
        assert!(b.name_id_format.is_none());
    }

    #[test]
    fn delete_body_deser() {
        let b: IdpDeleteBody = serde_json::from_str(r#"{"realm":"r","alias":"azure"}"#).unwrap();
        assert_eq!(b.realm, "r");
        assert_eq!(b.alias, "azure");
    }

    #[test]
    fn sync_resp_serializes() {
        let j = serde_json::to_string(&SyncResp {
            ok: true,
            alias: "okta".into(),
        })
        .unwrap();
        assert!(j.contains("\"ok\":true"));
        assert!(j.contains("okta"));
    }

    #[test]
    fn default_name_id_format_is_email() {
        assert!(DEFAULT_NAME_ID_FORMAT.contains("emailAddress"));
    }
}

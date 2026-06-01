//! GET /auth/login → 302 to Keycloak authorization_endpoint.
//!
//! Multi-tenant: resolve realm from Host header → use that realm's provider.
//! Single-tenant fallback: uses static `RpConfig`.

use std::{sync::Arc, time::Instant};

use axum::{
    extract::{Query, State},
    http::{header::HOST, HeaderMap},
    response::Redirect,
};
use serde::Deserialize;

use crate::error::{Result, RpError};
use crate::oidc::pkce::{challenge_s256, generate_verifier, random_token};
use crate::state::{AppState, PendingLogin};

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    pub redirect_uri: Option<String>,
    /// Keycloak IdP alias to jump straight to (skips the KC IdP-picker). Used
    /// to send a user to their org's SAML IdP via `?idp=<alias>`.
    pub idp: Option<String>,
}

pub async fn login(
    app: State<Arc<AppState>>,
    headers: HeaderMap,
    q: Query<LoginQuery>,
) -> Result<Redirect> {
    let started = Instant::now();
    let r = login_inner(app, headers, q).await;
    crate::metrics::record_result(crate::metrics::OP_LOGIN, started, &r);
    r
}

async fn login_inner(
    State(app): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<LoginQuery>,
) -> Result<Redirect> {
    let host = headers
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Resolve (authorization_endpoint, realm, redirect_uri).
    let (auth_ep, realm_opt, redirect_uri) = if app.is_multi() {
        match app.realm_for_host(&host) {
            Some(realm) => {
                let cache = app.multi_provider.as_ref().expect("is_multi");
                let prov = cache.get_or_fetch(&realm).await?;
                let ru = app.redirect_uri_for_host(&host);
                (prov.authorization_endpoint.clone(), Some(realm), ru)
            }
            None => {
                // Host not mapped → fall back to single-realm default.
                (
                    app.provider.authorization_endpoint.clone(),
                    None,
                    app.cfg.redirect_uri.clone(),
                )
            }
        }
    } else {
        (
            app.provider.authorization_endpoint.clone(),
            None,
            app.cfg.redirect_uri.clone(),
        )
    };

    let verifier = generate_verifier();
    let challenge = challenge_s256(&verifier);
    let state_tok = random_token();
    let nonce = random_token();

    app.insert_pending(
        state_tok.clone(),
        PendingLogin {
            code_verifier: verifier,
            post_login_redirect: q.redirect_uri,
            redirect_uri: redirect_uri.clone(),
            realm: realm_opt.clone(),
            expires_at: Instant::now() + app.state_ttl(),
        },
    )
    .await;

    let mut url = url::Url::parse(&auth_ep)
        .map_err(|e| RpError::Discovery(format!("invalid authorization_endpoint: {e}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &app.cfg.client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", "openid profile email")
        .append_pair("state", &state_tok)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");

    // kc_idp_hint sends the user straight to the named external IdP (e.g. a
    // tenant's SAML provider), bypassing Keycloak's IdP-picker screen. The alias
    // is validated by Keycloak; an unknown hint simply falls back to the picker.
    if let Some(idp) = q.idp.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        url.query_pairs_mut().append_pair("kc_idp_hint", idp);
    }

    tracing::info!(
        target: "audit",
        event = "auth.login.start",
        state = %state_tok,
        host  = %host,
        realm = ?realm_opt,
        "login initiated"
    );
    Ok(Redirect::to(url.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_query_redirect_uri_optional() {
        let q: LoginQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.redirect_uri.is_none());
        assert!(q.idp.is_none());
    }

    #[test]
    fn login_query_idp_parsed() {
        let q: LoginQuery = serde_json::from_str(r#"{"idp":"okta"}"#).unwrap();
        assert_eq!(q.idp.as_deref(), Some("okta"));
    }

    #[test]
    fn login_query_idp_and_redirect_together() {
        let q: LoginQuery =
            serde_json::from_str(r#"{"idp":"azure","redirect_uri":"/mail"}"#).unwrap();
        assert_eq!(q.idp.as_deref(), Some("azure"));
        assert_eq!(q.redirect_uri.as_deref(), Some("/mail"));
    }

    #[test]
    fn login_query_redirect_uri_set() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/mail"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/mail"));
    }

    #[test]
    fn login_query_redirect_uri_null_is_none() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":null}"#).unwrap();
        assert!(q.redirect_uri.is_none());
    }

    #[test]
    fn login_query_extra_field_ignored() {
        let q: LoginQuery =
            serde_json::from_str(r#"{"redirect_uri":"/calendar","extra":"x"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/calendar"));
    }

    #[test]
    fn login_query_redirect_with_path_query() {
        let q: LoginQuery =
            serde_json::from_str(r#"{"redirect_uri":"/mail?folder=INBOX"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/mail?folder=INBOX"));
    }

    #[test]
    fn login_query_empty_string_stored_as_some() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":""}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some(""));
    }

    #[test]
    fn login_query_absent_is_none() {
        let q: LoginQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.redirect_uri.is_none());
    }

    #[test]
    fn login_query_deep_path_stored() {
        let q: LoginQuery =
            serde_json::from_str(r#"{"redirect_uri":"/drive/files/abc/edit"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/drive/files/abc/edit"));
    }

    #[test]
    fn login_query_null_redirect_uri_is_none() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":null}"#).unwrap();
        assert!(q.redirect_uri.is_none());
    }

    #[test]
    fn login_query_redirect_uri_preserved() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/dashboard"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/dashboard"));
    }

    #[test]
    fn login_query_missing_field_gives_none() {
        let q: LoginQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.redirect_uri.is_none());
    }

    #[test]
    fn login_query_redirect_uri_with_fragment_stored() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/app#section"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/app#section"));
    }

    #[test]
    fn login_query_redirect_uri_absolute_path_preserved() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/dashboard"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/dashboard"));
    }

    #[test]
    fn login_query_no_redirect_uri_is_none() {
        let q: LoginQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.redirect_uri.is_none());
    }

    #[test]
    fn login_query_with_redirect_uri_preserved() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/inbox"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/inbox"));
    }

    #[test]
    fn login_query_redirect_uri_with_multiple_query_params() {
        let q: LoginQuery =
            serde_json::from_str(r#"{"redirect_uri":"/search?q=test&page=2"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/search?q=test&page=2"));
    }

    #[test]
    fn login_query_redirect_uri_unicode_path_stored() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/correio/caixa"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/correio/caixa"));
    }

    #[test]
    fn login_query_redirect_uri_with_encoded_space() {
        let q: LoginQuery =
            serde_json::from_str(r#"{"redirect_uri":"/path%20with%20spaces"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/path%20with%20spaces"));
    }

    #[test]
    fn login_query_redirect_uri_is_some_when_set() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/settings"}"#).unwrap();
        assert!(q.redirect_uri.is_some());
    }

    #[test]
    fn login_query_redirect_uri_root_preserved() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/"));
    }

    #[test]
    fn login_query_redirect_uri_numeric_path_stored() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/123"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/123"));
    }

    #[test]
    fn login_query_redirect_uri_slash_inbox_stored() {
        let q: LoginQuery = serde_json::from_str(r#"{"redirect_uri":"/inbox"}"#).unwrap();
        assert_eq!(q.redirect_uri.as_deref(), Some("/inbox"));
    }

    #[test]
    fn login_query_redirect_uri_is_none_when_absent() {
        let q: LoginQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.redirect_uri.is_none());
    }
}

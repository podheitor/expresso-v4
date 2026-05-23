//! GET /auth/logout → clear session cookies + redirect to IdP end_session.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header::{HOST, SET_COOKIE}, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use axum::http::{header::COOKIE, HeaderMap};
use expresso_auth_client::ACCESS_TOKEN_COOKIE;
use expresso_core::audit::{self, AuditEntry};

use crate::error::{Result, RpError};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LogoutQuery {
    pub id_token_hint: Option<String>,
}

pub async fn logout(
    State(app): State<Arc<AppState>>,
    headers:    HeaderMap,
    Query(q):   Query<LogoutQuery>,
) -> Result<Response> {
    let host = headers.get(HOST).and_then(|h| h.to_str().ok()).unwrap_or("").to_string();

    // Resolve end_session_endpoint per tenant when multi.
    let (end_session, post_logout_uri) = if app.is_multi() {
        match app.realm_for_host(&host) {
            Some(realm) => {
                let cache = app.multi_provider.as_ref().expect("is_multi");
                let prov = cache.get_or_fetch(&realm).await?;
                let es = prov.end_session_endpoint.clone()
                    .ok_or_else(|| RpError::Discovery("end_session_endpoint absent".into()))?;
                (es, app.post_logout_for_host(&host))
            }
            None => {
                let es = app.provider.end_session_endpoint.clone()
                    .ok_or_else(|| RpError::Discovery("end_session_endpoint absent".into()))?;
                (es, app.cfg.post_logout_redirect_uri.clone())
            }
        }
    } else {
        let es = app.provider.end_session_endpoint.clone()
            .ok_or_else(|| RpError::Discovery("end_session_endpoint absent".into()))?;
        (es, app.cfg.post_logout_redirect_uri.clone())
    };

    // Best-effort audit.
    if let Some(pool) = app.pool.as_ref() {
        let token = headers.get(COOKIE).and_then(|h| h.to_str().ok()).and_then(|c| {
            c.split(';').map(str::trim).find_map(|kv| {
                let (k, v) = kv.split_once('=')?;
                if k == ACCESS_TOKEN_COOKIE { Some(v.to_string()) } else { None }
            })
        });
        if let Some(tok) = token {
            if let Ok(ctx) = app.validator.validate(&tok).await {
                let entry = AuditEntry {
                    tenant_id:   Some(ctx.tenant_id),
                    actor_sub:   Some(ctx.user_id.to_string()),
                    actor_email: Some(ctx.email.clone()),
                    actor_roles: ctx.roles.clone(),
                    action:      "auth.logout".into(),
                    target_type: Some("user".into()),
                    target_id:   Some(ctx.user_id.to_string()),
                    http_method: Some("GET".into()),
                    http_path:   Some("/auth/logout".into()),
                    status_code: Some(303),
                    metadata:    serde_json::json!({}),
                };
                audit::record_async(pool.clone(), entry);
            }
        }
    }

    let mut url = url::Url::parse(&end_session)
        .map_err(|e| RpError::Discovery(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("client_id", &app.cfg.client_id);
        if let Some(h) = q.id_token_hint.as_deref() {
            qp.append_pair("id_token_hint", h);
        }
        if let Some(pl) = post_logout_uri.as_deref() {
            qp.append_pair("post_logout_redirect_uri", pl);
        }
    }

    let mut resp = Redirect::to(url.as_str()).into_response();
    *resp.status_mut() = StatusCode::SEE_OTHER;
    let h = resp.headers_mut();
    h.append(SET_COOKIE,
        format!("{ACCESS_TOKEN_COOKIE}=; HttpOnly; Path=/; SameSite=Lax; Max-Age=0").parse().unwrap());
    h.append(SET_COOKIE,
        "expresso_rt=; HttpOnly; Path=/auth/refresh; SameSite=Lax; Max-Age=0".parse().unwrap());
    tracing::info!(target: "audit", event = "auth.logout", host = %host, "user logged out");
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logout_query_id_token_hint_optional() {
        let q: LogoutQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.id_token_hint.is_none());
    }

    #[test]
    fn logout_query_id_token_hint_set() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"tkn123"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("tkn123"));
    }

    #[test]
    fn logout_query_null_hint_is_none() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":null}"#).unwrap();
        assert!(q.id_token_hint.is_none());
    }

    #[test]
    fn logout_query_extra_field_ignored() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"t","state":"s"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("t"));
    }

    #[test]
    fn logout_query_empty_hint_is_some_empty() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":""}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some(""));
    }

    #[test]
    fn logout_query_long_token_hint_stored() {
        let long = "eyJhbGciOiJSUzI1NiJ9.".repeat(10);
        let json = format!(r#"{{"id_token_hint":"{long}"}}"#);
        let q: LogoutQuery = serde_json::from_str(&json).unwrap();
        assert!(q.id_token_hint.is_some());
    }

    #[test]
    fn logout_query_absent_is_none() {
        let q: LogoutQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.id_token_hint.is_none());
    }

    #[test]
    fn logout_query_opaque_token_hint() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"abc.def.ghi"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("abc.def.ghi"));
    }

    #[test]
    fn logout_query_explicit_null_hint_is_none() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":null}"#).unwrap();
        assert!(q.id_token_hint.is_none());
    }

    #[test]
    fn logout_query_hint_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"eyJhb..."}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("eyJhb..."));
    }

    #[test]
    fn logout_query_missing_hint_is_none() {
        let q: LogoutQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.id_token_hint.is_none());
    }

    #[test]
    fn logout_query_hint_with_dots_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"a.b.c.d"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("a.b.c.d"));
    }

    #[test]
    fn logout_query_empty_hint_treated_as_some_empty() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":""}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some(""));
    }

    #[test]
    fn logout_query_missing_hint_is_definitely_none() {
        let q: LogoutQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.id_token_hint.is_none());
    }

    #[test]
    fn logout_query_hint_with_value_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"eyJhbG..."}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("eyJhbG..."));
    }

    #[test]
    fn logout_query_jwt_three_part_hint_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"header.payload.sig"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("header.payload.sig"));
    }

    #[test]
    fn logout_query_hint_with_equals_sign_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"tok=padded=="}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("tok=padded=="));
    }

    #[test]
    fn logout_query_hint_with_url_encoded_chars_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"tok%2Fval"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("tok%2Fval"));
    }

    #[test]
    fn logout_query_hint_is_some_when_nonempty() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"nonempty"}"#).unwrap();
        assert!(q.id_token_hint.is_some());
    }

    #[test]
    fn logout_query_hint_short_value_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"x"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("x"));
    }

    #[test]
    fn logout_query_hint_numeric_string_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"12345"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("12345"));
    }

    #[test]
    fn logout_query_hint_uuid_like_string_preserved() {
        let q: LogoutQuery = serde_json::from_str(r#"{"id_token_hint":"550e8400-e29b-41d4-a716-446655440000"}"#).unwrap();
        assert_eq!(q.id_token_hint.as_deref(), Some("550e8400-e29b-41d4-a716-446655440000"));
    }
}

//! GET /auth/logout → clear session cookies + redirect to IdP end_session.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header::SET_COOKIE, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;

use expresso_auth_client::ACCESS_TOKEN_COOKIE;

use crate::error::{Result, RpError};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LogoutQuery {
    pub id_token_hint: Option<String>,
}

pub async fn logout(
    State(app): State<Arc<AppState>>,
    Query(q):   Query<LogoutQuery>,
) -> Result<Response> {
    let end_session = app.provider.end_session_endpoint.as_ref()
        .ok_or_else(|| RpError::Discovery("end_session_endpoint absent".into()))?;
    let mut url = url::Url::parse(end_session)
        .map_err(|e| RpError::Discovery(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("client_id", &app.cfg.client_id);
        if let Some(h) = q.id_token_hint.as_deref() {
            qp.append_pair("id_token_hint", h);
        }
        if let Some(pl) = app.cfg.post_logout_redirect_uri.as_deref() {
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
    Ok(resp)
}

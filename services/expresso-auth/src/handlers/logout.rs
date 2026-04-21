//! GET /auth/logout → redirect to Keycloak end_session_endpoint.
//! Optional `id_token_hint` query propagated to IdP.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    response::Redirect,
};
use serde::Deserialize;

use crate::error::{Result, RpError};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LogoutQuery {
    pub id_token_hint: Option<String>,
}

pub async fn logout(
    State(app): State<Arc<AppState>>,
    Query(q):   Query<LogoutQuery>,
) -> Result<Redirect> {
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
    Ok(Redirect::to(url.as_str()))
}

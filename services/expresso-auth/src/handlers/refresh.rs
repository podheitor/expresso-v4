//! POST /auth/refresh { refresh_token } → TokenResponse

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::Deserialize;

use crate::error::{Result, RpError};
use crate::oidc::tokens::{RefreshRequest, TokenResponse};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct RefreshBody {
    pub refresh_token: String,
}

pub async fn refresh(
    State(app):  State<Arc<AppState>>,
    Json(body):  Json<RefreshBody>,
) -> Result<Json<TokenResponse>> {
    let form = RefreshRequest {
        grant_type:    "refresh_token",
        refresh_token: &body.refresh_token,
        client_id:     &app.cfg.client_id,
    };
    let resp = app.http
        .post(&app.provider.token_endpoint)
        .form(&form)
        .send()
        .await?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(RpError::Refresh(body));
    }
    let tokens: TokenResponse = resp.json().await
        .map_err(|e| RpError::Refresh(e.to_string()))?;
    Ok(Json(tokens))
}

//! Token endpoint request/response types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct AuthCodeRequest<'a> {
    pub grant_type:    &'static str,
    pub code:          &'a str,
    pub redirect_uri:  &'a str,
    pub client_id:     &'a str,
    pub code_verifier: &'a str,
}

#[derive(Debug, Serialize)]
pub struct RefreshRequest<'a> {
    pub grant_type:    &'static str,
    pub refresh_token: &'a str,
    pub client_id:     &'a str,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TokenResponse {
    pub access_token:  String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token:      Option<String>,
    pub token_type:    String,
    pub expires_in:    i64,
    #[serde(default)]
    pub refresh_expires_in: Option<i64>,
    #[serde(default)]
    pub scope:         Option<String>,
}

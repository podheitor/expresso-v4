//! OIDC discovery — resolves authorization / token / end_session endpoints.

use serde::Deserialize;
use std::time::Duration;

use crate::error::{Result, RpError};

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint:         String,
    #[serde(default)]
    pub end_session_endpoint:   Option<String>,
    pub issuer:                 String,
}

impl ProviderMetadata {
    /// Fetch `{issuer}/.well-known/openid-configuration`.
    pub async fn fetch(issuer: &str, timeout: Duration) -> Result<Self> {
        let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| RpError::Discovery(e.to_string()))?;
        let resp = client.get(&url).send().await
            .map_err(|e| RpError::Discovery(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(RpError::Discovery(format!("HTTP {}", resp.status())));
        }
        resp.json::<Self>().await.map_err(|e| RpError::Discovery(e.to_string()))
    }
}

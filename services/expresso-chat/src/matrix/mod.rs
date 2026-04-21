//! Matrix Client-Server API wrapper (thin).
//!
//! Uses AppService impersonation (`?user_id=@alice:example.com`) so a single
//! access token can act on behalf of any tenant-mapped Matrix user. See
//! Synapse AppService docs + MSC2965.
//!
//! Only implements the handful of endpoints we need right now:
//! - create room (POST /createRoom)
//! - invite user (POST /rooms/{roomId}/invite)
//! - send message (PUT /rooms/{roomId}/send/m.room.message/{txnId})
//! - list messages (GET /rooms/{roomId}/messages)

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::error::{ChatError, Result};

#[derive(Clone, Debug)]
pub struct MatrixConfig {
    pub hs_url:      String,   // e.g. https://synapse.expresso.local
    pub server_name: String,   // e.g. expresso.local (MXID suffix)
    pub as_token:    Option<String>,  // AppService token — impersonation allowed
    pub admin_token: Option<String>,  // Admin API token — user provisioning
}

#[derive(Clone, Debug)]
pub struct MatrixClient {
    cfg:  MatrixConfig,
    http: Client,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomPreset {
    PrivateChat,
    TrustedPrivateChat,
    PublicChat,
}

impl RoomPreset {
    fn as_str(&self) -> &'static str {
        match self {
            Self::PrivateChat        => "private_chat",
            Self::TrustedPrivateChat => "trusted_private_chat",
            Self::PublicChat         => "public_chat",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateRoomRequest<'a> {
    pub name:    &'a str,
    pub topic:   Option<&'a str>,
    pub preset:  RoomPreset,
    pub invite:  &'a [String],   // list of MXIDs
}

#[derive(Debug, Deserialize)]
pub struct CreateRoomResponse {
    pub room_id: String,
}

impl MatrixClient {
    pub fn new(cfg: MatrixConfig) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .expect("reqwest client build");
        Self { cfg, http }
    }

    /// Build an MXID from an Expresso user id. MVP rule: `@expresso-{uuid}:{server_name}`.
    /// Real mapping (email-based or SSO claim) lands when Keycloak integration wires up.
    pub fn mxid_for(&self, user_id: Uuid) -> String {
        format!("@expresso-{}:{}", user_id, self.cfg.server_name)
    }

    fn as_token(&self) -> Result<&str> {
        self.cfg.as_token.as_deref()
            .ok_or_else(|| ChatError::Matrix("MATRIX__AS_TOKEN not configured".into()))
    }

    /// Client-Server endpoint URL + impersonation query param.
    fn cs_url(&self, path: &str, acting_as: &str) -> Result<reqwest::Url> {
        let base = format!("{}/_matrix/client/v3{}", self.cfg.hs_url.trim_end_matches('/'), path);
        let mut url = reqwest::Url::parse(&base)
            .map_err(|e| ChatError::Matrix(format!("bad url: {e}")))?;
        url.query_pairs_mut().append_pair("user_id", acting_as);
        Ok(url)
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T> {
        let resp = req.send().await
            .map_err(|e| ChatError::Matrix(format!("request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!("HS {}: {}", status, body)));
        }
        resp.json::<T>().await
            .map_err(|e| ChatError::Matrix(format!("decode: {e}")))
    }

    pub async fn create_room(
        &self,
        acting_as: &str,
        req: &CreateRoomRequest<'_>,
    ) -> Result<CreateRoomResponse> {
        let url = self.cs_url("/createRoom", acting_as)?;
        let body = json!({
            "name":   req.name,
            "topic":  req.topic,
            "preset": req.preset.as_str(),
            "invite": req.invite,
        });
        self.send(self.http.post(url).bearer_auth(self.as_token()?).json(&body)).await
    }

    pub async fn invite_user(
        &self,
        acting_as: &str,
        room_id: &str,
        mxid_to_invite: &str,
    ) -> Result<()> {
        let path = format!("/rooms/{}/invite", urlencode(room_id));
        let url = self.cs_url(&path, acting_as)?;
        let body = json!({ "user_id": mxid_to_invite });
        let resp = self.http.post(url).bearer_auth(self.as_token()?).json(&body).send().await
            .map_err(|e| ChatError::Matrix(format!("invite failed: {e}")))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!("invite HS {}: {}", s, b)));
        }
        Ok(())
    }

    /// Send a plain text message. Returns event_id.
    pub async fn send_text(
        &self,
        acting_as: &str,
        room_id: &str,
        body: &str,
    ) -> Result<String> {
        let txn = Uuid::new_v4();
        let path = format!("/rooms/{}/send/m.room.message/{}", urlencode(room_id), txn);
        let url = self.cs_url(&path, acting_as)?;
        let payload = json!({ "msgtype": "m.text", "body": body });
        #[derive(Deserialize)] struct R { event_id: String }
        let r: R = self.send(self.http.put(url).bearer_auth(self.as_token()?).json(&payload)).await?;
        Ok(r.event_id)
    }

    /// List recent messages (reverse-chronological; limit capped at 100 by HS).
    pub async fn list_messages(
        &self,
        acting_as: &str,
        room_id: &str,
        limit: u32,
    ) -> Result<Value> {
        let path = format!("/rooms/{}/messages", urlencode(room_id));
        let mut url = self.cs_url(&path, acting_as)?;
        url.query_pairs_mut()
            .append_pair("dir", "b")
            .append_pair("limit", &limit.min(100).to_string());
        let resp = self.http.get(url).bearer_auth(self.as_token()?).send().await
            .map_err(|e| ChatError::Matrix(format!("list failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!("list HS {}: {}", status, body)));
        }
        resp.json::<Value>().await
            .map_err(|e| ChatError::Matrix(format!("decode list: {e}")))
    }
}

fn urlencode(s: &str) -> String {
    // Matrix room/event IDs contain `!` `:` which need percent-encoding in URL paths.
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mxid_format() {
        let cfg = MatrixConfig {
            hs_url: "http://x".into(),
            server_name: "expresso.local".into(),
            as_token: None, admin_token: None,
        };
        let c = MatrixClient::new(cfg);
        let u = Uuid::nil();
        assert_eq!(c.mxid_for(u), "@expresso-00000000-0000-0000-0000-000000000000:expresso.local");
    }
}

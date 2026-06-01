//! Matrix Client-Server API wrapper (thin).
//!
//! Uses AppService impersonation (`?user_id=@alice:example.com`) so a single
//! access token can act on behalf of any tenant-mapped Matrix user. See
//! Synapse AppService spec (appservice/v1).
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
    pub hs_url: String,           // e.g. https://synapse.expresso.local
    pub server_name: String,      // e.g. expresso.local (MXID suffix)
    pub as_token: Option<String>, // AppService token — impersonation allowed
    #[allow(dead_code)]
    // reserved — Synapse Admin API (deactivate/rename) lands with Keycloak sync
    pub admin_token: Option<String>, // Admin API token — user provisioning
}

#[derive(Clone, Debug)]
pub struct MatrixClient {
    cfg: MatrixConfig,
    http: Client,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
// Variant names mirror the Matrix `preset` protocol values; the shared
// `Chat` suffix is part of that external vocabulary, not redundant naming.
#[allow(clippy::enum_variant_names)]
pub enum RoomPreset {
    PrivateChat,
    TrustedPrivateChat,
    PublicChat,
}

impl RoomPreset {
    fn as_str(&self) -> &'static str {
        match self {
            Self::PrivateChat => "private_chat",
            Self::TrustedPrivateChat => "trusted_private_chat",
            Self::PublicChat => "public_chat",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateRoomRequest<'a> {
    pub name: &'a str,
    pub topic: Option<&'a str>,
    pub preset: RoomPreset,
    pub invite: &'a [String], // list of MXIDs
}

#[derive(Debug, Deserialize)]
pub struct CreateRoomResponse {
    pub room_id: String,
}

/// Parameters for a file message referencing uploaded media. `msgtype` is one
/// of `m.image`/`m.video`/`m.audio`/`m.file`; `mxc_uri` is an `upload_media`
/// result.
#[derive(Debug)]
pub struct FileMessage<'a> {
    pub msgtype: &'a str,
    pub filename: &'a str,
    pub content_type: &'a str,
    pub mxc_uri: &'a str,
    pub size_bytes: i64,
}

impl MatrixClient {
    /// Admin API token (Synapse `/_synapse/admin/*`) — wired for user provisioning
    /// once Keycloak→Matrix sync lands. Returned lazily so callers can fail with
    /// a typed error instead of a boot-time panic.
    #[allow(dead_code)]
    pub fn admin_token(&self) -> Result<&str> {
        self.cfg
            .admin_token
            .as_deref()
            .ok_or_else(|| ChatError::Matrix("MATRIX__ADMIN_TOKEN not configured".into()))
    }

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
        self.cfg
            .as_token
            .as_deref()
            .ok_or_else(|| ChatError::Matrix("MATRIX__AS_TOKEN not configured".into()))
    }

    /// Client-Server endpoint URL + impersonation query param.
    fn cs_url(&self, path: &str, acting_as: &str) -> Result<reqwest::Url> {
        let base = format!(
            "{}/_matrix/client/v3{}",
            self.cfg.hs_url.trim_end_matches('/'),
            path
        );
        let mut url =
            reqwest::Url::parse(&base).map_err(|e| ChatError::Matrix(format!("bad url: {e}")))?;
        url.query_pairs_mut().append_pair("user_id", acting_as);
        Ok(url)
    }

    /// Media-repo endpoint URL (`/_matrix/media/v3`) + impersonation query param.
    fn media_url(&self, path: &str, acting_as: &str) -> Result<reqwest::Url> {
        let base = format!(
            "{}/_matrix/media/v3{}",
            self.cfg.hs_url.trim_end_matches('/'),
            path
        );
        let mut url =
            reqwest::Url::parse(&base).map_err(|e| ChatError::Matrix(format!("bad url: {e}")))?;
        url.query_pairs_mut().append_pair("user_id", acting_as);
        Ok(url)
    }

    async fn send<T: for<'de> Deserialize<'de>>(&self, req: reqwest::RequestBuilder) -> Result<T> {
        let resp = req
            .send()
            .await
            .map_err(|e| ChatError::Matrix(format!("request failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!("HS {}: {}", status, body)));
        }
        resp.json::<T>()
            .await
            .map_err(|e| ChatError::Matrix(format!("decode: {e}")))
    }

    /// Ensure AS-scope user exists on HS. Idempotent → tolerates M_USER_IN_USE.
    /// Synapse requires AS to pre-register users within its exclusive namespace
    /// before impersonation (?user_id=…) via CS API works.
    async fn ensure_registered(&self, mxid: &str) -> Result<()> {
        let localpart = localpart_from_mxid(mxid)
            .ok_or_else(|| ChatError::Matrix(format!("bad mxid: {mxid}")))?;
        let url = format!(
            "{}/_matrix/client/v3/register",
            self.cfg.hs_url.trim_end_matches('/')
        );
        let body = json!({
            "type":     "m.login.application_service",
            "username": localpart,
        });
        let resp = self
            .http
            .post(&url)
            .bearer_auth(self.as_token()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChatError::Matrix(format!("register failed: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        if text.contains("M_USER_IN_USE") {
            return Ok(());
        }
        Err(ChatError::Matrix(format!(
            "register HS {}: {}",
            status, text
        )))
    }

    pub async fn create_room(
        &self,
        acting_as: &str,
        req: &CreateRoomRequest<'_>,
    ) -> Result<CreateRoomResponse> {
        self.ensure_registered(acting_as).await?;
        let url = self.cs_url("/createRoom", acting_as)?;
        let body = json!({
            "name":   req.name,
            "topic":  req.topic,
            "preset": req.preset.as_str(),
            "invite": req.invite,
        });
        self.send(
            self.http
                .post(url)
                .bearer_auth(self.as_token()?)
                .json(&body),
        )
        .await
    }

    pub async fn invite_user(
        &self,
        acting_as: &str,
        room_id: &str,
        mxid_to_invite: &str,
    ) -> Result<()> {
        self.ensure_registered(acting_as).await?;
        let path = format!("/rooms/{}/invite", urlencode(room_id));
        let url = self.cs_url(&path, acting_as)?;
        let body = json!({ "user_id": mxid_to_invite });
        let resp = self
            .http
            .post(url)
            .bearer_auth(self.as_token()?)
            .json(&body)
            .send()
            .await
            .map_err(|e| ChatError::Matrix(format!("invite failed: {e}")))?;
        if !resp.status().is_success() {
            let s = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!("invite HS {}: {}", s, b)));
        }
        Ok(())
    }

    /// Send a plain text message. Returns event_id.
    pub async fn send_text(&self, acting_as: &str, room_id: &str, body: &str) -> Result<String> {
        self.ensure_registered(acting_as).await?;
        let txn = Uuid::new_v4();
        let path = format!("/rooms/{}/send/m.room.message/{}", urlencode(room_id), txn);
        let url = self.cs_url(&path, acting_as)?;
        let payload = json!({ "msgtype": "m.text", "body": body });
        #[derive(Deserialize)]
        struct R {
            event_id: String,
        }
        let r: R = self
            .send(
                self.http
                    .put(url)
                    .bearer_auth(self.as_token()?)
                    .json(&payload),
            )
            .await?;
        Ok(r.event_id)
    }

    /// Edit a message: send a new `m.room.message` carrying an `m.replace`
    /// relation to `target_event_id` (MSC2676). `body`/`msgtype` are the
    /// fallback for clients that don't render edits; `m.new_content` is the
    /// replacement. Returns the edit event's id. The HS enforces that only the
    /// original sender may edit (the relation is rejected otherwise).
    pub async fn edit_text(
        &self,
        acting_as: &str,
        room_id: &str,
        target_event_id: &str,
        new_body: &str,
    ) -> Result<String> {
        self.ensure_registered(acting_as).await?;
        let txn = Uuid::new_v4();
        let path = format!("/rooms/{}/send/m.room.message/{}", urlencode(room_id), txn);
        let url = self.cs_url(&path, acting_as)?;
        let payload = json!({
            "msgtype": "m.text",
            "body": format!("* {new_body}"),
            "m.new_content": { "msgtype": "m.text", "body": new_body },
            "m.relates_to": { "rel_type": "m.replace", "event_id": target_event_id },
        });
        #[derive(Deserialize)]
        struct R {
            event_id: String,
        }
        let r: R = self
            .send(
                self.http
                    .put(url)
                    .bearer_auth(self.as_token()?)
                    .json(&payload),
            )
            .await?;
        Ok(r.event_id)
    }

    /// Redact (delete) an event. `PUT /rooms/{room}/redact/{event}/{txn}`.
    /// Returns the redaction event's id. The HS enforces redaction permission
    /// (sender, or a power-level high enough to redact others).
    pub async fn redact(
        &self,
        acting_as: &str,
        room_id: &str,
        target_event_id: &str,
        reason: Option<&str>,
    ) -> Result<String> {
        self.ensure_registered(acting_as).await?;
        let txn = Uuid::new_v4();
        let path = format!(
            "/rooms/{}/redact/{}/{}",
            urlencode(room_id),
            urlencode(target_event_id),
            txn
        );
        let url = self.cs_url(&path, acting_as)?;
        let payload = match reason {
            Some(r) => json!({ "reason": r }),
            None => json!({}),
        };
        #[derive(Deserialize)]
        struct R {
            event_id: String,
        }
        let r: R = self
            .send(
                self.http
                    .put(url)
                    .bearer_auth(self.as_token()?)
                    .json(&payload),
            )
            .await?;
        Ok(r.event_id)
    }

    /// Reply inside a thread: send an `m.room.message` carrying an `m.thread`
    /// relation to `root_event_id` (MSC3440). `m.in_reply_to` + the
    /// `is_falling_back` flag give clients without thread support a sensible
    /// reply fallback. Returns the reply event's id.
    pub async fn reply_in_thread(
        &self,
        acting_as: &str,
        room_id: &str,
        root_event_id: &str,
        body: &str,
    ) -> Result<String> {
        self.ensure_registered(acting_as).await?;
        let txn = Uuid::new_v4();
        let path = format!("/rooms/{}/send/m.room.message/{}", urlencode(room_id), txn);
        let url = self.cs_url(&path, acting_as)?;
        let payload = json!({
            "msgtype": "m.text",
            "body": body,
            "m.relates_to": {
                "rel_type": "m.thread",
                "event_id": root_event_id,
                "is_falling_back": true,
                "m.in_reply_to": { "event_id": root_event_id },
            },
        });
        #[derive(Deserialize)]
        struct R {
            event_id: String,
        }
        let r: R = self
            .send(
                self.http
                    .put(url)
                    .bearer_auth(self.as_token()?)
                    .json(&payload),
            )
            .await?;
        Ok(r.event_id)
    }

    /// List events in a thread rooted at `root_event_id` via
    /// `GET /rooms/{room}/relations/{event}/m.thread` (MSC3440). Returns the raw
    /// `chunk` response (chronological); `limit` is capped at 100 by the HS.
    pub async fn list_thread(
        &self,
        acting_as: &str,
        room_id: &str,
        root_event_id: &str,
        limit: u32,
    ) -> Result<Value> {
        self.ensure_registered(acting_as).await?;
        let path = format!(
            "/rooms/{}/relations/{}/m.thread",
            urlencode(room_id),
            urlencode(root_event_id),
        );
        let mut url = self.cs_url(&path, acting_as)?;
        url.query_pairs_mut()
            .append_pair("limit", &limit.min(100).to_string());
        let resp = self
            .http
            .get(url)
            .bearer_auth(self.as_token()?)
            .send()
            .await
            .map_err(|e| ChatError::Matrix(format!("thread list failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!(
                "thread list HS {}: {}",
                status, body
            )));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| ChatError::Matrix(format!("decode thread list: {e}")))
    }

    /// Read a room's pinned event ids from the `m.room.pinned_events` state
    /// event. Returns an empty list when no pins are set (the HS answers 404
    /// `M_NOT_FOUND` for an absent state event — treated as "no pins").
    pub async fn get_pinned_events(&self, acting_as: &str, room_id: &str) -> Result<Vec<String>> {
        self.ensure_registered(acting_as).await?;
        let path = format!("/rooms/{}/state/m.room.pinned_events/", urlencode(room_id));
        let url = self.cs_url(&path, acting_as)?;
        let resp = self
            .http
            .get(url)
            .bearer_auth(self.as_token()?)
            .send()
            .await
            .map_err(|e| ChatError::Matrix(format!("get pinned failed: {e}")))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!(
                "get pinned HS {}: {}",
                status, body
            )));
        }
        #[derive(Deserialize)]
        struct R {
            #[serde(default)]
            pinned: Vec<String>,
        }
        let r: R = resp
            .json()
            .await
            .map_err(|e| ChatError::Matrix(format!("decode pinned: {e}")))?;
        Ok(r.pinned)
    }

    /// Replace a room's pinned event ids by writing the `m.room.pinned_events`
    /// state event. The HS enforces the power level required to send state
    /// (surfaced as a Matrix error if the caller lacks it).
    pub async fn set_pinned_events(
        &self,
        acting_as: &str,
        room_id: &str,
        pinned: &[String],
    ) -> Result<String> {
        self.ensure_registered(acting_as).await?;
        let path = format!("/rooms/{}/state/m.room.pinned_events/", urlencode(room_id));
        let url = self.cs_url(&path, acting_as)?;
        let payload = json!({ "pinned": pinned });
        #[derive(Deserialize)]
        struct R {
            event_id: String,
        }
        let r: R = self
            .send(
                self.http
                    .put(url)
                    .bearer_auth(self.as_token()?)
                    .json(&payload),
            )
            .await?;
        Ok(r.event_id)
    }

    /// Upload bytes to the Matrix media repo (`POST /_matrix/media/v3/upload`).
    /// Returns the resulting `mxc://` content URI. `filename` is sent so the HS
    /// records it; `content_type` becomes the stored MIME type.
    pub async fn upload_media(
        &self,
        acting_as: &str,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String> {
        self.ensure_registered(acting_as).await?;
        let mut url = self.media_url("/upload", acting_as)?;
        url.query_pairs_mut().append_pair("filename", filename);
        #[derive(Deserialize)]
        struct R {
            content_uri: String,
        }
        let r: R = self
            .send(
                self.http
                    .post(url)
                    .bearer_auth(self.as_token()?)
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(bytes),
            )
            .await?;
        Ok(r.content_uri)
    }

    /// Send a file message referencing previously-uploaded media. Returns the
    /// message event id.
    pub async fn send_file_message(
        &self,
        acting_as: &str,
        room_id: &str,
        msg: &FileMessage<'_>,
    ) -> Result<String> {
        self.ensure_registered(acting_as).await?;
        let txn = Uuid::new_v4();
        let path = format!("/rooms/{}/send/m.room.message/{}", urlencode(room_id), txn);
        let url = self.cs_url(&path, acting_as)?;
        let payload = json!({
            "msgtype": msg.msgtype,
            "body": msg.filename,
            "url": msg.mxc_uri,
            "info": { "mimetype": msg.content_type, "size": msg.size_bytes },
        });
        #[derive(Deserialize)]
        struct R {
            event_id: String,
        }
        let r: R = self
            .send(
                self.http
                    .put(url)
                    .bearer_auth(self.as_token()?)
                    .json(&payload),
            )
            .await?;
        Ok(r.event_id)
    }

    /// List recent messages (reverse-chronological; limit capped at 100 by HS).
    pub async fn list_messages(&self, acting_as: &str, room_id: &str, limit: u32) -> Result<Value> {
        self.ensure_registered(acting_as).await?;
        let path = format!("/rooms/{}/messages", urlencode(room_id));
        let mut url = self.cs_url(&path, acting_as)?;
        url.query_pairs_mut()
            .append_pair("dir", "b")
            .append_pair("limit", &limit.min(100).to_string());
        let resp = self
            .http
            .get(url)
            .bearer_auth(self.as_token()?)
            .send()
            .await
            .map_err(|e| ChatError::Matrix(format!("list failed: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Matrix(format!("list HS {}: {}", status, body)));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| ChatError::Matrix(format!("decode list: {e}")))
    }

    /// Substring-search a room's recent messages. Scans the latest `scan_limit`
    /// events (capped at 100 by the HS) and returns the `m.room.message` events
    /// whose `content.body` contains `query` (case-insensitive). This is a
    /// pragmatic search over the recent window — it does NOT use the HS
    /// full-text `/search` (which requires server-side indexing that may be
    /// disabled); deep history beyond the scan window is not searched.
    pub async fn search_messages(
        &self,
        acting_as: &str,
        room_id: &str,
        query: &str,
        scan_limit: u32,
    ) -> Result<Value> {
        let raw = self.list_messages(acting_as, room_id, scan_limit).await?;
        let needle = query.to_lowercase();
        let matches: Vec<Value> = raw
            .get("chunk")
            .and_then(Value::as_array)
            .map(|chunk| {
                chunk
                    .iter()
                    .filter(|ev| event_body_contains(ev, &needle))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        Ok(serde_json::json!({ "chunk": matches, "count": matches.len() }))
    }
}

/// True when a Matrix event is a message whose `content.body` contains `needle`
/// (which the caller has already lowercased).
fn event_body_contains(ev: &Value, needle: &str) -> bool {
    ev.get("content")
        .and_then(|c| c.get("body"))
        .and_then(Value::as_str)
        .is_some_and(|b| b.to_lowercase().contains(needle))
}

fn urlencode(s: &str) -> String {
    // Matrix room/event IDs contain `!` `:` which need percent-encoding in URL paths.
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Extract localpart from a MXID (`@local:server` → `local`). Returns None on
/// malformed input (missing `@`, missing `:`, empty localpart).
fn localpart_from_mxid(mxid: &str) -> Option<&str> {
    let rest = mxid.strip_prefix('@')?;
    let (local, server) = rest.split_once(':')?;
    if local.is_empty() || server.is_empty() {
        return None;
    }
    Some(local)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_body_contains_matches_case_insensitive() {
        let ev = serde_json::json!({"content": {"body": "Hello World"}});
        assert!(event_body_contains(&ev, "hello"));
        assert!(event_body_contains(&ev, "world"));
        assert!(!event_body_contains(&ev, "bye"));
    }

    #[test]
    fn event_body_contains_false_without_body() {
        let ev = serde_json::json!({"content": {"membership": "join"}});
        assert!(!event_body_contains(&ev, "anything"));
        let ev2 = serde_json::json!({"type": "m.reaction"});
        assert!(!event_body_contains(&ev2, "x"));
    }

    #[test]
    fn mxid_format() {
        let cfg = MatrixConfig {
            hs_url: "http://x".into(),
            server_name: "expresso.local".into(),
            as_token: None,
            admin_token: None,
        };
        let c = MatrixClient::new(cfg);
        let u = Uuid::nil();
        assert_eq!(
            c.mxid_for(u),
            "@expresso-00000000-0000-0000-0000-000000000000:expresso.local"
        );
    }

    #[test]
    fn localpart_valid() {
        assert_eq!(localpart_from_mxid("@alice:expresso.local"), Some("alice"));
        assert_eq!(
            localpart_from_mxid("@expresso-7d8b:expresso.local"),
            Some("expresso-7d8b")
        );
    }

    #[test]
    fn localpart_rejects_malformed() {
        assert_eq!(
            localpart_from_mxid("alice:expresso.local"),
            None,
            "missing @"
        );
        assert_eq!(localpart_from_mxid("@alice"), None, "missing :");
        assert_eq!(localpart_from_mxid("@:expresso.local"), None, "empty local");
        assert_eq!(localpart_from_mxid("@alice:"), None, "empty server");
        assert_eq!(localpart_from_mxid(""), None, "empty");
    }

    #[test]
    fn urlencode_plain_ascii_unchanged() {
        assert_eq!(urlencode("hello"), "hello");
    }

    #[test]
    fn urlencode_special_chars_encoded() {
        let enc = urlencode("!room:server.com");
        assert!(enc.contains('%'), "exclamation should be encoded");
    }

    #[test]
    fn urlencode_empty_string() {
        assert_eq!(urlencode(""), "");
    }

    #[test]
    fn mxid_contains_uuid() {
        let cfg = MatrixConfig {
            hs_url: "http://x".into(),
            server_name: "s.local".into(),
            as_token: None,
            admin_token: None,
        };
        let c = MatrixClient::new(cfg);
        let u = Uuid::nil();
        let mxid = c.mxid_for(u);
        assert!(mxid.contains("00000000-0000-0000-0000-000000000000"));
    }

    #[test]
    fn localpart_strips_at_and_server() {
        assert_eq!(localpart_from_mxid("@bot:matrix.org"), Some("bot"));
    }

    #[test]
    fn localpart_no_at_returns_none() {
        assert_eq!(localpart_from_mxid("bot:matrix.org"), None);
    }

    #[test]
    fn localpart_empty_local_returns_none() {
        assert_eq!(localpart_from_mxid("@:matrix.org"), None);
    }

    #[test]
    fn localpart_with_special_chars_in_server() {
        assert_eq!(
            localpart_from_mxid("@alice:chat.example.com"),
            Some("alice")
        );
    }

    #[test]
    fn localpart_no_at_sign_returns_none() {
        assert_eq!(localpart_from_mxid("notanmxid"), None);
    }

    #[test]
    fn mxid_starts_with_at_sign() {
        let cfg = MatrixConfig {
            hs_url: "http://x".into(),
            server_name: "s.local".into(),
            as_token: None,
            admin_token: None,
        };
        let c = MatrixClient::new(cfg);
        let mxid = c.mxid_for(Uuid::nil());
        assert!(mxid.starts_with('@'));
    }

    #[test]
    fn room_preset_public_as_str() {
        assert_eq!(RoomPreset::PublicChat.as_str(), "public_chat");
    }

    #[test]
    fn room_preset_private_as_str() {
        assert_eq!(RoomPreset::PrivateChat.as_str(), "private_chat");
    }

    #[test]
    fn room_preset_trusted_private_as_str() {
        assert_eq!(
            RoomPreset::TrustedPrivateChat.as_str(),
            "trusted_private_chat"
        );
    }

    #[test]
    fn mxid_ends_with_server_name() {
        let cfg = MatrixConfig {
            hs_url: "http://x".into(),
            server_name: "chat.expresso.local".into(),
            as_token: None,
            admin_token: None,
        };
        let c = MatrixClient::new(cfg);
        let mxid = c.mxid_for(Uuid::nil());
        assert!(mxid.ends_with(":chat.expresso.local"));
    }

    #[test]
    fn room_preset_variants_all_different() {
        assert_ne!(
            RoomPreset::PrivateChat.as_str(),
            RoomPreset::PublicChat.as_str()
        );
        assert_ne!(
            RoomPreset::TrustedPrivateChat.as_str(),
            RoomPreset::PublicChat.as_str()
        );
    }

    #[test]
    fn room_preset_public_as_str_is_public_chat() {
        assert_eq!(RoomPreset::PublicChat.as_str(), "public_chat");
    }

    #[test]
    fn room_preset_trusted_private_as_str_contains_trusted() {
        assert!(RoomPreset::TrustedPrivateChat.as_str().contains("trusted"));
    }

    #[test]
    fn room_preset_private_as_str_is_private_chat() {
        assert_eq!(RoomPreset::PrivateChat.as_str(), "private_chat");
    }

    #[test]
    fn room_preset_trusted_private_as_str_is_trusted_private_chat() {
        assert_eq!(
            RoomPreset::TrustedPrivateChat.as_str(),
            "trusted_private_chat"
        );
    }

    #[test]
    fn room_preset_public_as_str_ne_private_chat() {
        assert_ne!(
            RoomPreset::PublicChat.as_str(),
            RoomPreset::PrivateChat.as_str()
        );
    }
}

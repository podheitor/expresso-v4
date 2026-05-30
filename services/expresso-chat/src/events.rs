//! Process-local chat event bus for realtime SSE delivery.
//!
//! When a message is sent (or a typing signal fires), the handler publishes
//! a `ChatEvent` here; the SSE endpoint (`api::stream`) subscribes
//! and forwards events to connected clients, filtered by tenant + channel.
//!
//! In-process only by design: every write goes through this service, so a
//! broadcast channel is enough for live delivery. Durable history stays in
//! Matrix (`GET …/messages`). Cross-instance fan-out would need a shared bus
//! (NATS), mirroring `expresso-calendar`'s EventBus — not required for MVP.

use serde::Serialize;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Ring capacity. A slow SSE client that lags past this many events gets a
/// `Lagged` error on its receiver and is simply skipped (it can re-fetch
/// history via the REST list endpoint on reconnect).
const BUS_CAPACITY: usize = 256;

/// Kind of realtime signal. `message`/`edit`/`delete`/`reaction` reflect
/// durable state (Matrix message or relation / local chat_reactions row);
/// `typing` is ephemeral — never stored, only forwarded live. (Presence is a
/// planned kind; added when online/offline tracking lands.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatEventKind {
    Message,
    Reply,
    Edit,
    Delete,
    Typing,
    Reaction,
}

impl ChatEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Reply => "reply",
            Self::Edit => "edit",
            Self::Delete => "delete",
            Self::Typing => "typing",
            Self::Reaction => "reaction",
        }
    }
}

/// A single realtime event scoped to one channel within one tenant.
#[derive(Debug, Clone, Serialize)]
pub struct ChatEvent {
    pub tenant_id: Uuid,
    pub channel_id: Uuid,
    pub kind: ChatEventKind,
    /// Acting user (message author / who is typing). Clients use it to skip
    /// echoing their own typing indicator.
    pub user_id: Uuid,
    /// Target id: the Matrix event id for `message`/`reaction` events; `None`
    /// for ephemeral signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// Message text for `message` events; `None` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Emoji for `reaction` events; `None` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// For `reaction` events: `true` = added, `false` = removed. `None` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<bool>,
    /// Thread root event id for `reply` events; `None` otherwise. Lets clients
    /// route the reply under its thread without re-fetching the relation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_root: Option<String>,
}

impl ChatEvent {
    /// A delivered chat message.
    pub fn message(
        tenant_id: Uuid,
        channel_id: Uuid,
        user_id: Uuid,
        event_id: String,
        body: String,
    ) -> Self {
        Self {
            tenant_id,
            channel_id,
            kind: ChatEventKind::Message,
            user_id,
            event_id: Some(event_id),
            body: Some(body),
            emoji: None,
            added: None,
            thread_root: None,
        }
    }

    /// A reply posted inside a thread rooted at `thread_root`.
    pub fn reply(
        tenant_id: Uuid,
        channel_id: Uuid,
        user_id: Uuid,
        thread_root: String,
        event_id: String,
        body: String,
    ) -> Self {
        Self {
            tenant_id,
            channel_id,
            kind: ChatEventKind::Reply,
            user_id,
            event_id: Some(event_id),
            body: Some(body),
            emoji: None,
            added: None,
            thread_root: Some(thread_root),
        }
    }

    /// An ephemeral "user is typing" signal.
    pub fn typing(tenant_id: Uuid, channel_id: Uuid, user_id: Uuid) -> Self {
        Self {
            tenant_id,
            channel_id,
            kind: ChatEventKind::Typing,
            user_id,
            event_id: None,
            body: None,
            emoji: None,
            added: None,
            thread_root: None,
        }
    }

    /// A message edit: `target_event_id` now reads `new_body`.
    pub fn edit(
        tenant_id: Uuid,
        channel_id: Uuid,
        user_id: Uuid,
        target_event_id: String,
        new_body: String,
    ) -> Self {
        Self {
            tenant_id,
            channel_id,
            kind: ChatEventKind::Edit,
            user_id,
            event_id: Some(target_event_id),
            body: Some(new_body),
            emoji: None,
            added: None,
            thread_root: None,
        }
    }

    /// A message deletion (redaction) of `target_event_id`.
    pub fn delete(
        tenant_id: Uuid,
        channel_id: Uuid,
        user_id: Uuid,
        target_event_id: String,
    ) -> Self {
        Self {
            tenant_id,
            channel_id,
            kind: ChatEventKind::Delete,
            user_id,
            event_id: Some(target_event_id),
            body: None,
            emoji: None,
            added: None,
            thread_root: None,
        }
    }

    /// A reaction add/remove on a message (`target_event_id`).
    pub fn reaction(
        tenant_id: Uuid,
        channel_id: Uuid,
        user_id: Uuid,
        target_event_id: String,
        emoji: String,
        added: bool,
    ) -> Self {
        Self {
            tenant_id,
            channel_id,
            kind: ChatEventKind::Reaction,
            user_id,
            event_id: Some(target_event_id),
            body: None,
            emoji: Some(emoji),
            added: Some(added),
            thread_root: None,
        }
    }
}

/// Cloneable handle to the in-process broadcast channel.
#[derive(Clone)]
pub struct ChatBus {
    tx: broadcast::Sender<ChatEvent>,
}

impl ChatBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Best-effort publish. Returns the number of live subscribers that
    /// received it (0 when nobody is connected — not an error).
    pub fn publish(&self, ev: ChatEvent) -> usize {
        self.tx.send(ev).unwrap_or(0)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ChatEvent> {
        self.tx.subscribe()
    }
}

impl Default for ChatBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Uuid, Uuid, Uuid) {
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4())
    }

    #[test]
    fn kind_as_str_matches_serialization() {
        assert_eq!(ChatEventKind::Message.as_str(), "message");
        assert_eq!(ChatEventKind::Typing.as_str(), "typing");
    }

    #[test]
    fn message_event_carries_body_and_event_id() {
        let (t, c, u) = ids();
        let ev = ChatEvent::message(t, c, u, "$evt:hs".into(), "hello".into());
        assert_eq!(ev.kind, ChatEventKind::Message);
        assert_eq!(ev.event_id.as_deref(), Some("$evt:hs"));
        assert_eq!(ev.body.as_deref(), Some("hello"));
    }

    #[test]
    fn reply_event_carries_thread_root() {
        let (t, c, u) = ids();
        let ev = ChatEvent::reply(t, c, u, "$root:hs".into(), "$reply:hs".into(), "yes".into());
        assert_eq!(ev.kind, ChatEventKind::Reply);
        assert_eq!(ev.thread_root.as_deref(), Some("$root:hs"));
        assert_eq!(ev.event_id.as_deref(), Some("$reply:hs"));
        assert_eq!(ev.body.as_deref(), Some("yes"));
    }

    #[test]
    fn reply_event_serializes_thread_root() {
        let (t, c, u) = ids();
        let ev = ChatEvent::reply(t, c, u, "$root:hs".into(), "$reply:hs".into(), "yes".into());
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"kind\":\"reply\""), "got: {json}");
        assert!(json.contains("thread_root"), "got: {json}");
    }

    #[test]
    fn message_event_omits_thread_root() {
        let (t, c, u) = ids();
        let json = serde_json::to_string(&ChatEvent::message(t, c, u, "$e:hs".into(), "hi".into()))
            .unwrap();
        assert!(!json.contains("thread_root"), "got: {json}");
    }

    #[test]
    fn typing_event_is_ephemeral() {
        let (t, c, u) = ids();
        let ev = ChatEvent::typing(t, c, u);
        assert_eq!(ev.kind, ChatEventKind::Typing);
        assert!(ev.event_id.is_none());
        assert!(ev.body.is_none());
    }

    #[test]
    fn typing_event_omits_null_fields_in_json() {
        let (t, c, u) = ids();
        let json = serde_json::to_string(&ChatEvent::typing(t, c, u)).unwrap();
        assert!(!json.contains("event_id"), "got: {json}");
        assert!(!json.contains("body"), "got: {json}");
        assert!(json.contains("\"kind\":\"typing\""), "got: {json}");
    }

    #[tokio::test]
    async fn publish_delivers_to_subscriber() {
        let bus = ChatBus::new();
        let mut rx = bus.subscribe();
        let (t, c, u) = ids();
        let n = bus.publish(ChatEvent::message(t, c, u, "$e:hs".into(), "hi".into()));
        assert_eq!(n, 1, "one subscriber should receive");
        let got = rx.recv().await.unwrap();
        assert_eq!(got.channel_id, c);
        assert_eq!(got.body.as_deref(), Some("hi"));
    }

    #[test]
    fn publish_without_subscribers_returns_zero() {
        let bus = ChatBus::new();
        let (t, c, u) = ids();
        assert_eq!(bus.publish(ChatEvent::typing(t, c, u)), 0);
    }

    #[tokio::test]
    async fn two_subscribers_both_receive() {
        let bus = ChatBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let (t, c, u) = ids();
        assert_eq!(bus.publish(ChatEvent::typing(t, c, u)), 2);
        assert_eq!(rx1.recv().await.unwrap().channel_id, c);
        assert_eq!(rx2.recv().await.unwrap().channel_id, c);
    }
}

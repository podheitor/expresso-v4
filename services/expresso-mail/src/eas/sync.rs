//! EAS Sync command (MS-ASCMD §2.2.2.20) — mail item sync, read direction.
//!
//! The client sends `<Sync><Collections><Collection><SyncKey>K</SyncKey>
//! <CollectionId>id</CollectionId>…`. On K="0" we reset the collection's state
//! and return an empty Add set with a fresh key (EAS priming round); on a
//! non-zero key we emit an Add per message newer than the high-water UID we last
//! sent that device, then advance the stored key + UID. Client→server changes
//! (\Seen toggle, delete) are applied from the request `<Commands>` first.
//!
//! Envelope fields come from the `messages` row; the body is fetched from
//! storage and truncated to the client's `BodyPreference/TruncationSize` (see
//! `resolve_bodies`), falling back to `preview_text` when the raw body is
//! unavailable. MIME multipart decoding of the body is a later refinement.

use expresso_wbxml::{
    decode, encode,
    tokens::{air_sync, air_sync_base, email, page},
    Event,
};
use uuid::Uuid;

use crate::state::AppState;

/// How many messages to emit in one Sync response (EAS WindowSize default 100).
const WINDOW_SIZE: i64 = 100;

/// A client→server change in a Sync request.
#[derive(Debug, PartialEq, Eq)]
pub enum ClientChange {
    /// Mark read/unread (`<Change>` carrying `<Read>`). ServerId is `mboxid:uid`.
    SetRead { server_id: String, read: bool },
    /// Delete a message (`<Delete>`). ServerId is `mboxid:uid`.
    Delete { server_id: String },
}

/// Parsed fields from a Sync request collection (only what the MVP needs).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncRequest {
    pub sync_key: String,
    pub collection_id: String,
    pub changes: Vec<ClientChange>,
    /// Client `BodyPreference/TruncationSize` (bytes). `None` → server default.
    pub truncation_size: Option<usize>,
}

/// Default body truncation when the client doesn't specify one — keeps the
/// preview cheap while real clients negotiate a larger size.
const DEFAULT_TRUNCATION: usize = 32 * 1024;

/// Parse the first `<Collection>`'s SyncKey + CollectionId from a Sync request.
/// Missing fields default to empty; the caller treats an empty/zero key as the
/// priming round.
pub fn parse_sync_request(body: &[u8]) -> SyncRequest {
    let Ok(events) = decode(body) else {
        return SyncRequest::default();
    };
    let mut req = SyncRequest::default();
    // `field` carries (page, token) of the leaf whose Text we're capturing, so
    // TruncationSize (AirSyncBase page) is told apart from AirSync fields.
    let mut field: Option<(u8, u8)> = None;
    for ev in events {
        match ev {
            Event::StartElement { page: p, token, .. } => field = Some((p, token)),
            Event::Text(t) => {
                match field {
                    Some((page::AIR_SYNC, air_sync::SYNC_KEY)) if req.sync_key.is_empty() => {
                        req.sync_key = t;
                    }
                    Some((page::AIR_SYNC, air_sync::COLLECTION_ID))
                        if req.collection_id.is_empty() =>
                    {
                        req.collection_id = t;
                    }
                    Some((page::AIR_SYNC_BASE, air_sync_base::TRUNCATION_SIZE)) => {
                        req.truncation_size = t.parse().ok();
                    }
                    _ => {}
                }
                field = None;
            }
            _ => field = None,
        }
    }
    // Client→server commands are parsed in a focused second pass.
    req.changes = parse_changes(body);
    req
}

/// Focused parse of client `<Commands>` (Change/Delete) — kept separate from the
/// header parse so each stays simple. Tracks the current command + its ServerId
/// and Read value, emitting a [`ClientChange`] when the command element closes.
fn parse_changes(body: &[u8]) -> Vec<ClientChange> {
    let Ok(events) = decode(body) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth_is_delete: Option<bool> = None;
    let mut server_id: Option<String> = None;
    let mut read: Option<bool> = None;
    let mut want: Option<u8> = None; // which leaf text we're capturing
    for ev in events {
        match &ev {
            Event::StartElement { page: p, token, .. } if *p == page::AIR_SYNC => match *token {
                air_sync::CHANGE => {
                    depth_is_delete = Some(false);
                    server_id = None;
                    read = None;
                }
                air_sync::DELETE => {
                    depth_is_delete = Some(true);
                    server_id = None;
                    read = None;
                }
                air_sync::SERVER_ID => want = Some(air_sync::SERVER_ID),
                _ => want = None,
            },
            Event::StartElement { page: p, token, .. }
                if *p == page::EMAIL && *token == email::READ =>
            {
                want = Some(email::READ);
            }
            Event::Text(t) => {
                match want {
                    Some(air_sync::SERVER_ID) => server_id = Some(t.clone()),
                    Some(email::READ) => read = Some(t == "1"),
                    _ => {}
                }
                want = None;
            }
            Event::EndElement => {
                // Close of a Change/Delete: emit when we have a ServerId.
                if let (Some(is_delete), Some(sid)) = (depth_is_delete, server_id.clone()) {
                    if is_delete {
                        out.push(ClientChange::Delete { server_id: sid });
                        depth_is_delete = None;
                        server_id = None;
                    } else if let Some(r) = read {
                        out.push(ClientChange::SetRead {
                            server_id: sid,
                            read: r,
                        });
                        depth_is_delete = None;
                        server_id = None;
                        read = None;
                    }
                }
            }
            _ => want = None,
        }
    }
    out
}

/// One message as EAS Sync sees it.
struct MailItem {
    uid: i64,
    server_id: Uuid,
    subject: Option<String>,
    from_addr: Option<String>,
    to_addrs: serde_json::Value,
    date: Option<time::OffsetDateTime>,
    preview: Option<String>,
    body_path: Option<String>,
    flags: Vec<String>,
    /// Resolved plain-text body for the EAS response, filled in by
    /// `resolve_bodies` honoring the client's TruncationSize.
    body_text: String,
    body_truncated: bool,
}

/// Build the Sync response for a collection. `device_id` scopes the per-device
/// SyncKey/UID state. Returns the WBXML body.
pub async fn sync_response(
    state: &AppState,
    user_id: Uuid,
    tenant_id: Uuid,
    device_id: &str,
    req: &SyncRequest,
) -> Vec<u8> {
    let Ok(collection_id) = Uuid::parse_str(&req.collection_id) else {
        return sync_error("8"); // Status 8 = invalid CollectionId.
    };

    // Priming round: client sends SyncKey 0 → reset state, return key 1, no items.
    if req.sync_key == "0" || req.sync_key.is_empty() {
        let _ = reset_state(state, tenant_id, user_id, device_id, collection_id).await;
        return sync_ok(&req.collection_id, 1, &[]);
    }

    // Apply any client→server changes (\Seen toggles, deletes) before computing
    // the server→client delta. Best-effort: a failed change doesn't abort Sync.
    apply_client_changes(state, tenant_id, collection_id, &req.changes).await;

    let (key, last_uid) = load_state(state, tenant_id, user_id, device_id, collection_id).await;
    let mut items = load_new_items(state, tenant_id, collection_id, last_uid).await;
    resolve_bodies(
        state,
        &mut items,
        req.truncation_size.unwrap_or(DEFAULT_TRUNCATION),
    )
    .await;
    let new_key = key + 1;
    let new_high = items.iter().map(|i| i.uid).max().unwrap_or(last_uid);
    let _ = save_state(
        state,
        tenant_id,
        user_id,
        device_id,
        collection_id,
        new_key,
        new_high,
    )
    .await;
    sync_ok(&req.collection_id, new_key, &items)
}

/// Build a Sync error response carrying `status`.
fn sync_error(status: &str) -> Vec<u8> {
    let a = page::AIR_SYNC;
    encode(&[
        Event::start(a, air_sync::SYNC),
        Event::start(a, air_sync::STATUS),
        Event::Text(status.into()),
        Event::EndElement,
        Event::EndElement,
    ])
}

/// Build a successful Sync response with `key` and the message Adds.
fn sync_ok(collection_id: &str, key: i64, items: &[MailItem]) -> Vec<u8> {
    let a = page::AIR_SYNC;
    let mut doc = vec![
        Event::start(a, air_sync::SYNC),
        Event::start(a, air_sync::COLLECTIONS),
        Event::start(a, air_sync::COLLECTION),
        Event::start(a, air_sync::SYNC_KEY),
        Event::Text(key.to_string()),
        Event::EndElement,
        Event::start(a, air_sync::COLLECTION_ID),
        Event::Text(collection_id.into()),
        Event::EndElement,
        Event::start(a, air_sync::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
    ];
    if !items.is_empty() {
        doc.push(Event::start(a, air_sync::COMMANDS));
        for it in items {
            push_add(&mut doc, it);
        }
        doc.push(Event::EndElement); // Commands
    }
    doc.push(Event::EndElement); // Collection
    doc.push(Event::EndElement); // Collections
    doc.push(Event::EndElement); // Sync
    encode(&doc)
}

/// Append one `<Add>` (ServerId + ApplicationData with Email envelope + body).
fn push_add(doc: &mut Vec<Event>, it: &MailItem) {
    let a = page::AIR_SYNC;
    let e = page::EMAIL;
    let b = page::AIR_SYNC_BASE;
    doc.push(Event::start(a, air_sync::ADD));
    doc.push(Event::start(a, air_sync::SERVER_ID));
    doc.push(Event::Text(format!("{}:{}", it.server_id, it.uid)));
    doc.push(Event::EndElement);
    doc.push(Event::start(a, air_sync::APPLICATION_DATA));

    if let Some(s) = &it.subject {
        push_text(doc, e, email::SUBJECT, s);
    }
    if let Some(f) = &it.from_addr {
        push_text(doc, e, email::FROM, f);
    }
    push_text(doc, e, email::TO, &display_to(&it.to_addrs));
    if let Some(d) = it.date {
        push_text(doc, e, email::DATE_RECEIVED, &format_eas_date(d));
    }
    let read = if it.flags.iter().any(|fl| fl == "\\Seen") {
        "1"
    } else {
        "0"
    };
    push_text(doc, e, email::READ, read);

    // Body (AirSyncBase): plain text, fetched + truncated by resolve_bodies.
    doc.push(Event::start(b, air_sync_base::BODY));
    push_text(doc, b, air_sync_base::TYPE, "1"); // 1 = plain text
    push_text(
        doc,
        b,
        air_sync_base::ESTIMATED_DATA_SIZE,
        &it.body_text.len().to_string(),
    );
    push_text(
        doc,
        b,
        air_sync_base::TRUNCATED,
        if it.body_truncated { "1" } else { "0" },
    );
    push_text(doc, b, air_sync_base::DATA, &it.body_text);
    doc.push(Event::EndElement); // Body

    doc.push(Event::EndElement); // ApplicationData
    doc.push(Event::EndElement); // Add
}

fn push_text(doc: &mut Vec<Event>, page: u8, token: u8, text: &str) {
    doc.push(Event::start(page, token));
    doc.push(Event::Text(text.into()));
    doc.push(Event::EndElement);
}

/// Render the `to_addrs` JSON array `[{addr,name}]` as a comma-joined header.
fn display_to(to: &serde_json::Value) -> String {
    let Some(arr) = to.as_array() else {
        return String::new();
    };
    arr.iter()
        .filter_map(|v| v.get("addr").and_then(|a| a.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// EAS dates are ISO-8601 UTC `YYYY-MM-DDTHH:MM:SS.000Z`.
fn format_eas_date(d: time::OffsetDateTime) -> String {
    let u = d.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
        u.year(),
        u.month() as u8,
        u.day(),
        u.hour(),
        u.minute(),
        u.second()
    )
}

/// Raw `messages` row shape for the Sync query (factored out to keep the query
/// type under clippy's complexity threshold).
type MailRow = (
    i64,
    Uuid,
    Option<String>,
    Option<String>,
    serde_json::Value,
    Option<time::OffsetDateTime>,
    Option<String>,
    Option<String>,
    Vec<String>,
);

async fn load_new_items(
    state: &AppState,
    tenant_id: Uuid,
    mailbox_id: Uuid,
    last_uid: i64,
) -> Vec<MailItem> {
    let rows: Vec<MailRow> = sqlx::query_as(
        "SELECT uid, id, subject, from_addr, to_addrs, date, preview_text, body_path, flags \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND uid > $3 \
             ORDER BY uid ASC LIMIT $4",
    )
    .bind(mailbox_id)
    .bind(tenant_id)
    .bind(last_uid)
    .bind(WINDOW_SIZE)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(
            |(uid, server_id, subject, from_addr, to_addrs, date, preview, body_path, flags)| {
                MailItem {
                    uid,
                    server_id,
                    subject,
                    from_addr,
                    to_addrs,
                    date,
                    preview,
                    body_path,
                    flags,
                    body_text: String::new(),
                    body_truncated: false,
                }
            },
        )
        .collect()
}

/// Fill each item's `body_text`/`body_truncated`: fetch the raw message from
/// storage, decode its plain-text MIME part, and truncate to `max_bytes`.
/// Falls back to `preview_text` when the body can't be fetched.
async fn resolve_bodies(state: &AppState, items: &mut [MailItem], max_bytes: usize) {
    for it in items.iter_mut() {
        let full = match &it.body_path {
            Some(path) => crate::pop3::store::fetch_body(state, path)
                .await
                .map(|raw| extract_plain_body(&raw)),
            None => None,
        };
        let text = full.or_else(|| it.preview.clone()).unwrap_or_default();
        if text.len() > max_bytes {
            // Truncate on a char boundary at or below max_bytes.
            let mut end = max_bytes;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            it.body_text = text[..end].to_string();
            it.body_truncated = true;
        } else {
            it.body_text = text;
            it.body_truncated = false;
        }
    }
}

/// Extract the decoded plain-text body from a raw RFC822 message. Parses MIME
/// via `mail-parser` and returns the first `text/plain` part (transfer-decoded,
/// charset-normalised). Falls back to the raw bytes after the header separator
/// when the message doesn't parse or has no text part (e.g. HTML-only — HTML→
/// text conversion is a further refinement).
fn extract_plain_body(raw: &[u8]) -> String {
    if let Some(msg) = mail_parser::MessageParser::default().parse(raw) {
        if let Some(text) = msg.body_text(0) {
            return text.into_owned();
        }
    }
    let body = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(raw, |pos| &raw[pos + 4..]);
    String::from_utf8_lossy(body).into_owned()
}

async fn load_state(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    device_id: &str,
    collection_id: Uuid,
) -> (i64, i64) {
    sqlx::query_as(
        "SELECT sync_key, last_uid FROM eas_sync_state \
         WHERE tenant_id = $1 AND user_id = $2 AND device_id = $3 AND collection_id = $4",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(device_id)
    .bind(collection_id)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten()
    .unwrap_or((1, 0))
}

async fn save_state(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    device_id: &str,
    collection_id: Uuid,
    sync_key: i64,
    last_uid: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO eas_sync_state \
            (tenant_id, user_id, device_id, collection_id, sync_key, last_uid, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now()) \
         ON CONFLICT (tenant_id, user_id, device_id, collection_id) DO UPDATE SET \
            sync_key = EXCLUDED.sync_key, last_uid = EXCLUDED.last_uid, updated_at = now()",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(device_id)
    .bind(collection_id)
    .bind(sync_key)
    .bind(last_uid)
    .execute(state.db())
    .await?;
    Ok(())
}

async fn reset_state(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    device_id: &str,
    collection_id: Uuid,
) -> Result<(), sqlx::Error> {
    save_state(state, tenant_id, user_id, device_id, collection_id, 1, 0).await
}

/// Extract the message UID from an EAS ServerId of the form `mboxid:uid`.
fn uid_from_server_id(server_id: &str) -> Option<i64> {
    server_id.rsplit_once(':')?.1.parse().ok()
}

/// Apply client→server changes to the messages in `collection_id`. Read toggles
/// add/remove the `\Seen` flag; deletes remove the row. Each is scoped by
/// mailbox + tenant + uid. Best-effort: errors are logged, not propagated.
async fn apply_client_changes(
    state: &AppState,
    tenant_id: Uuid,
    collection_id: Uuid,
    changes: &[ClientChange],
) {
    for ch in changes {
        let result = match ch {
            ClientChange::SetRead { server_id, read } => {
                let Some(uid) = uid_from_server_id(server_id) else {
                    continue;
                };
                let sql = if *read {
                    "UPDATE messages SET flags = \
                        (SELECT array_agg(DISTINCT f) FROM unnest(flags || ARRAY['\\Seen']) f) \
                     WHERE mailbox_id = $1 AND tenant_id = $2 AND uid = $3"
                } else {
                    "UPDATE messages SET flags = array_remove(flags, '\\Seen') \
                     WHERE mailbox_id = $1 AND tenant_id = $2 AND uid = $3"
                };
                sqlx::query(sql)
                    .bind(collection_id)
                    .bind(tenant_id)
                    .bind(uid)
                    .execute(state.db())
                    .await
            }
            ClientChange::Delete { server_id } => {
                let Some(uid) = uid_from_server_id(server_id) else {
                    continue;
                };
                sqlx::query(
                    "DELETE FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND uid = $3",
                )
                .bind(collection_id)
                .bind(tenant_id)
                .bind(uid)
                .execute(state.db())
                .await
            }
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "EAS client change failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn parse_sync_request_reads_key_and_collection() {
        let a = page::AIR_SYNC;
        let req = encode(&[
            Event::start(a, air_sync::SYNC),
            Event::start(a, air_sync::COLLECTIONS),
            Event::start(a, air_sync::COLLECTION),
            Event::start(a, air_sync::SYNC_KEY),
            Event::Text("5".into()),
            Event::EndElement,
            Event::start(a, air_sync::COLLECTION_ID),
            Event::Text("abc-123".into()),
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
        ]);
        let parsed = parse_sync_request(&req);
        assert_eq!(parsed.sync_key, "5");
        assert_eq!(parsed.collection_id, "abc-123");
    }

    #[test]
    fn parse_sync_request_garbage_is_default() {
        assert_eq!(parse_sync_request(&[0xFF, 0x00]), SyncRequest::default());
    }

    #[test]
    fn display_to_joins_addrs() {
        let v = serde_json::json!([{"addr":"a@x.com","name":"A"},{"addr":"b@x.com"}]);
        assert_eq!(display_to(&v), "a@x.com, b@x.com");
    }

    #[test]
    fn display_to_empty_for_non_array() {
        assert_eq!(display_to(&serde_json::json!(null)), "");
        assert_eq!(display_to(&serde_json::json!({})), "");
    }

    #[test]
    fn format_eas_date_is_iso_utc() {
        let d = datetime!(2026-06-02 14:30:05 UTC);
        assert_eq!(format_eas_date(d), "2026-06-02T14:30:05.000Z");
    }

    #[test]
    fn sync_error_carries_status() {
        let bytes = sync_error("8");
        let events = decode(&bytes).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::Text(t) if t == "8")));
    }

    #[test]
    fn sync_ok_priming_has_no_commands() {
        let bytes = sync_ok("col-1", 1, &[]);
        let events = decode(&bytes).unwrap();
        // No Commands element when there are no items.
        assert!(!events.iter().any(
            |e| matches!(e, Event::StartElement { token, .. } if *token == air_sync::COMMANDS)
        ));
    }

    #[test]
    fn sync_ok_round_trips() {
        let bytes = sync_ok("col-1", 2, &[]);
        let events = decode(&bytes).unwrap();
        assert_eq!(encode(&events), bytes);
    }

    #[test]
    fn uid_from_server_id_parses_suffix() {
        assert_eq!(uid_from_server_id("mbox-uuid:42"), Some(42));
        assert_eq!(uid_from_server_id("42"), None);
        assert_eq!(uid_from_server_id("mbox:notanum"), None);
    }

    #[test]
    fn parse_changes_reads_delete() {
        let a = page::AIR_SYNC;
        let body = encode(&[
            Event::start(a, air_sync::SYNC),
            Event::start(a, air_sync::COMMANDS),
            Event::start(a, air_sync::DELETE),
            Event::start(a, air_sync::SERVER_ID),
            Event::Text("m:7".into()),
            Event::EndElement,
            Event::EndElement, // Delete
            Event::EndElement, // Commands
            Event::EndElement, // Sync
        ]);
        let changes = parse_changes(&body);
        assert_eq!(
            changes,
            vec![ClientChange::Delete {
                server_id: "m:7".into()
            }]
        );
    }

    #[test]
    fn parse_changes_reads_set_read() {
        let a = page::AIR_SYNC;
        let e = page::EMAIL;
        let body = encode(&[
            Event::start(a, air_sync::SYNC),
            Event::start(a, air_sync::COMMANDS),
            Event::start(a, air_sync::CHANGE),
            Event::start(a, air_sync::SERVER_ID),
            Event::Text("m:9".into()),
            Event::EndElement,
            Event::start(a, air_sync::APPLICATION_DATA),
            Event::start(e, email::READ),
            Event::Text("1".into()),
            Event::EndElement,
            Event::EndElement, // ApplicationData
            Event::EndElement, // Change
            Event::EndElement, // Commands
            Event::EndElement, // Sync
        ]);
        let changes = parse_changes(&body);
        assert_eq!(
            changes,
            vec![ClientChange::SetRead {
                server_id: "m:9".into(),
                read: true
            }]
        );
    }

    #[test]
    fn parse_sync_request_reads_truncation_size() {
        let a = page::AIR_SYNC;
        let b = page::AIR_SYNC_BASE;
        let body = encode(&[
            Event::start(a, air_sync::SYNC),
            Event::start(a, air_sync::COLLECTION_ID),
            Event::Text("m-1".into()),
            Event::EndElement,
            Event::start(b, air_sync_base::BODY_PREFERENCE),
            Event::start(b, air_sync_base::TRUNCATION_SIZE),
            Event::Text("5120".into()),
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
        ]);
        let req = parse_sync_request(&body);
        assert_eq!(req.truncation_size, Some(5120));
        assert_eq!(req.collection_id, "m-1");
    }

    #[test]
    fn extract_plain_body_simple_message() {
        let raw = b"Subject: hi\r\nContent-Type: text/plain\r\n\r\nthe body here";
        assert_eq!(extract_plain_body(raw).trim(), "the body here");
    }

    #[test]
    fn extract_plain_body_multipart_picks_text_part() {
        let raw = b"Subject: hi\r\n\
Content-Type: multipart/alternative; boundary=\"B\"\r\n\r\n\
--B\r\nContent-Type: text/plain\r\n\r\nplain version\r\n\
--B\r\nContent-Type: text/html\r\n\r\n<p>html version</p>\r\n--B--\r\n";
        let body = extract_plain_body(raw);
        assert!(body.contains("plain version"));
        assert!(!body.contains("<p>"));
    }

    #[test]
    fn extract_plain_body_quoted_printable_decoded() {
        let raw = b"Content-Type: text/plain\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\r\nca=C3=A9"; // "caé"
        assert_eq!(extract_plain_body(raw).trim(), "caé");
    }

    #[test]
    fn parse_changes_empty_when_no_commands() {
        let a = page::AIR_SYNC;
        let body = encode(&[
            Event::start(a, air_sync::SYNC),
            Event::start(a, air_sync::SYNC_KEY),
            Event::Text("3".into()),
            Event::EndElement,
            Event::EndElement,
        ]);
        assert!(parse_changes(&body).is_empty());
    }
}

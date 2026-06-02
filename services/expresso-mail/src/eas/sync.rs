//! EAS Sync command (MS-ASCMD §2.2.2.20) — mail item sync, read direction.
//!
//! The client sends `<Sync><Collections><Collection><SyncKey>K</SyncKey>
//! <CollectionId>id</CollectionId>…`. On K="0" we reset the collection's state
//! and return an empty Add set with a fresh key (EAS priming round); on a
//! non-zero key we emit an Add per message newer than the high-water UID we last
//! sent that device, then advance the stored key + UID. Read direction only —
//! client→server changes (\Seen, delete, move) land in sprint 5.
//!
//! Envelope fields come from the `messages` row; the body uses `preview_text`
//! (cheap, no object-store fetch) as a truncated plain-text body. Full-body
//! fetch honoring the client TruncationSize is a later refinement.

use expresso_wbxml::{
    decode, encode,
    tokens::{air_sync, air_sync_base, email, page},
    Event,
};
use uuid::Uuid;

use crate::state::AppState;

/// How many messages to emit in one Sync response (EAS WindowSize default 100).
const WINDOW_SIZE: i64 = 100;

/// Parsed fields from a Sync request collection (only what the MVP needs).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncRequest {
    pub sync_key: String,
    pub collection_id: String,
}

/// Parse the first `<Collection>`'s SyncKey + CollectionId from a Sync request.
/// Missing fields default to empty; the caller treats an empty/zero key as the
/// priming round.
pub fn parse_sync_request(body: &[u8]) -> SyncRequest {
    let Ok(events) = decode(body) else {
        return SyncRequest::default();
    };
    let mut req = SyncRequest::default();
    let mut field: Option<u8> = None;
    for ev in events {
        match ev {
            Event::StartElement { page: p, token, .. } if p == page::AIR_SYNC => {
                field = Some(token);
            }
            Event::Text(t) => match field {
                Some(air_sync::SYNC_KEY) if req.sync_key.is_empty() => req.sync_key = t,
                Some(air_sync::COLLECTION_ID) if req.collection_id.is_empty() => {
                    req.collection_id = t;
                }
                _ => {}
            },
            _ => field = None,
        }
    }
    req
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
    flags: Vec<String>,
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

    let (key, last_uid) = load_state(state, tenant_id, user_id, device_id, collection_id).await;
    let items = load_new_items(state, tenant_id, collection_id, last_uid).await;
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

    // Body (AirSyncBase): plain-text preview, marked truncated.
    let body = it.preview.clone().unwrap_or_default();
    doc.push(Event::start(b, air_sync_base::BODY));
    push_text(doc, b, air_sync_base::TYPE, "1"); // 1 = plain text
    push_text(
        doc,
        b,
        air_sync_base::ESTIMATED_DATA_SIZE,
        &body.len().to_string(),
    );
    push_text(doc, b, air_sync_base::TRUNCATED, "1");
    push_text(doc, b, air_sync_base::DATA, &body);
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
    Vec<String>,
);

async fn load_new_items(
    state: &AppState,
    tenant_id: Uuid,
    mailbox_id: Uuid,
    last_uid: i64,
) -> Vec<MailItem> {
    let rows: Vec<MailRow> = sqlx::query_as(
        "SELECT uid, id, subject, from_addr, to_addrs, date, preview_text, flags \
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
            |(uid, server_id, subject, from_addr, to_addrs, date, preview, flags)| MailItem {
                uid,
                server_id,
                subject,
                from_addr,
                to_addrs,
                date,
                preview,
                flags,
            },
        )
        .collect()
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
}

//! EAS Sync for the Calendar class (MS-ASCAL).
//!
//! Routed here when the Sync CollectionId carries the `cal:` prefix FolderSync
//! assigned. Server→client: maps `calendar_events` rows to EAS Calendar items
//! (page 4). Client→server: Add/Change/Delete are translated to a VEVENT and
//! applied THROUGH the calendar service's internal API (`/internal/calendar/
//! events`) — never writing `calendar_events` from mail, so the calendar
//! service's iCal/etag/ctag/scheduling logic stays authoritative.

use expresso_wbxml::{
    decode, encode,
    tokens::{air_sync, calendar, page},
    Event,
};
use uuid::Uuid;

use crate::eas::sync::SyncRequest;
use crate::state::AppState;

const WINDOW_SIZE: i64 = 100;

/// Build the Sync response for a `cal:` collection. `raw_collection_id` is the
/// full `cal:<uuid>` ServerId from the request; the UUID is the calendar id.
/// `body` is the raw request WBXML — needed to read client `<Commands>` (event
/// create/edit/delete) which carry Calendar-page fields not in `SyncRequest`.
pub async fn calendar_sync_response(
    state: &AppState,
    tenant_id: Uuid,
    req: &SyncRequest,
    body: &[u8],
) -> Vec<u8> {
    let Some(uuid_str) = req.collection_id.strip_prefix("cal:") else {
        return status_only("8");
    };
    let Ok(calendar_id) = Uuid::parse_str(uuid_str) else {
        return status_only("8");
    };

    // Priming round resets to key 1 with no items (EAS handshake).
    if req.sync_key == "0" || req.sync_key.is_empty() {
        return ok(&req.collection_id, 1, &[]);
    }

    // Apply client→server event changes (Add/Change/Delete) THROUGH the calendar
    // service's internal API — never writing calendar_events directly from mail.
    apply_calendar_changes(state, tenant_id, calendar_id, body).await;

    let key: i64 = req.sync_key.parse().unwrap_or(1);
    let items = load_events(state, tenant_id, calendar_id).await;
    ok(&req.collection_id, key + 1, &items)
}

/// A client-originated calendar command parsed from the Sync request.
enum CalCommand {
    /// Add or Change → upsert. Carries the EAS fields to build a VEVENT.
    Upsert {
        uid: String,
        summary: Option<String>,
        location: Option<String>,
        start: Option<String>,
        end: Option<String>,
    },
    /// Delete by event id (ServerId).
    Delete { event_id: String },
}

/// Parse client `<Commands>` from a calendar Sync request: Add/Change collect
/// Calendar-page fields (UID/Subject/Location/Start/End) into an Upsert; Delete
/// collects its ServerId. UID falls back to the ServerId when the client omits
/// it (Change of an existing event).
fn parse_calendar_commands(body: &[u8]) -> Vec<CalCommand> {
    let Ok(events) = decode(body) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut mode: Option<bool> = None; // Some(true)=delete, Some(false)=upsert
    let mut cmd_depth: i32 = 0; // element depth at which the current command opened
    let mut depth: i32 = 0;
    let mut server_id: Option<String> = None;
    let (mut uid, mut summary, mut location, mut start, mut end) = (None, None, None, None, None);
    let mut want: Option<(u8, u8)> = None;
    for ev in events {
        match &ev {
            Event::StartElement {
                page: p,
                token,
                has_content,
            } => {
                if *p == page::AIR_SYNC
                    && matches!(*token, air_sync::ADD | air_sync::CHANGE | air_sync::DELETE)
                {
                    mode = Some(*token == air_sync::DELETE);
                    cmd_depth = depth;
                    server_id = None;
                    uid = None;
                    summary = None;
                    location = None;
                    start = None;
                    end = None;
                } else {
                    want = Some((*p, *token));
                }
                if *has_content {
                    depth += 1;
                }
            }
            Event::Text(t) => {
                match want {
                    Some((page::AIR_SYNC, air_sync::SERVER_ID)) => server_id = Some(t.clone()),
                    Some((page::CALENDAR, calendar::UID)) => uid = Some(t.clone()),
                    Some((page::CALENDAR, calendar::SUBJECT)) => summary = Some(t.clone()),
                    Some((page::CALENDAR, calendar::LOCATION)) => location = Some(t.clone()),
                    Some((page::CALENDAR, calendar::START_TIME)) => start = Some(t.clone()),
                    Some((page::CALENDAR, calendar::END_TIME)) => end = Some(t.clone()),
                    _ => {}
                }
                want = None;
            }
            Event::EndElement => {
                depth -= 1;
                want = None;
                // Flush only when this EndElement closes the command element.
                if mode.is_some() && depth == cmd_depth {
                    match mode {
                        Some(true) if server_id.is_some() => out.push(CalCommand::Delete {
                            event_id: server_id.take().unwrap(),
                        }),
                        Some(false) if uid.is_some() || server_id.is_some() => {
                            out.push(CalCommand::Upsert {
                                uid: uid.clone().or_else(|| server_id.clone()).unwrap(),
                                summary: summary.clone(),
                                location: location.clone(),
                                start: start.clone(),
                                end: end.clone(),
                            });
                        }
                        _ => {}
                    }
                    mode = None;
                }
            }
            _ => want = None,
        }
    }
    out
}

/// Apply parsed calendar commands by calling the calendar service's internal
/// API (`calendar_url/internal/calendar/events`). Best-effort: failures are
/// logged, never aborting the Sync. No-op when `calendar_url` is unset.
async fn apply_calendar_changes(state: &AppState, tenant_id: Uuid, calendar_id: Uuid, body: &[u8]) {
    let base = state.cfg().calendar_url.clone();
    if base.is_empty() {
        return;
    }
    let http = reqwest::Client::new();
    for cmd in parse_calendar_commands(body) {
        let result = match cmd {
            CalCommand::Upsert {
                uid,
                summary,
                location,
                start,
                end,
            } => {
                let ical = build_vevent(
                    &uid,
                    summary.as_deref(),
                    location.as_deref(),
                    start.as_deref(),
                    end.as_deref(),
                );
                http.post(format!("{base}/internal/calendar/events"))
                    .json(&serde_json::json!({
                        "tenant_id": tenant_id,
                        "calendar_id": calendar_id,
                        "ical_raw": ical,
                    }))
                    .send()
                    .await
                    .map(|_| ())
            }
            CalCommand::Delete { event_id } => http
                .delete(format!("{base}/internal/calendar/events/{event_id}"))
                .query(&[("tenant_id", tenant_id.to_string())])
                .send()
                .await
                .map(|_| ()),
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "EAS calendar change forward failed");
        }
    }
}

/// Build a minimal VCALENDAR/VEVENT from EAS Calendar fields. EAS compact dates
/// (`YYYYMMDDTHHMMSSZ`) are already valid iCalendar UTC date-times.
fn build_vevent(
    uid: &str,
    summary: Option<&str>,
    location: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
) -> String {
    let mut s = String::from(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Expresso//EAS//EN\r\nBEGIN:VEVENT\r\n",
    );
    s.push_str(&format!("UID:{uid}\r\n"));
    if let Some(v) = start {
        s.push_str(&format!("DTSTART:{v}\r\n"));
    }
    if let Some(v) = end {
        s.push_str(&format!("DTEND:{v}\r\n"));
    }
    if let Some(v) = summary {
        s.push_str(&format!("SUMMARY:{}\r\n", ical_escape(v)));
    }
    if let Some(v) = location {
        s.push_str(&format!("LOCATION:{}\r\n", ical_escape(v)));
    }
    s.push_str("END:VEVENT\r\nEND:VCALENDAR\r\n");
    s
}

/// Escape iCalendar TEXT special chars (RFC 5545 §3.3.11).
fn ical_escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

struct CalItem {
    id: Uuid,
    uid: String,
    summary: Option<String>,
    location: Option<String>,
    dtstart: Option<time::OffsetDateTime>,
    dtend: Option<time::OffsetDateTime>,
}

fn status_only(status: &str) -> Vec<u8> {
    let a = page::AIR_SYNC;
    encode(&[
        Event::start(a, air_sync::SYNC),
        Event::start(a, air_sync::STATUS),
        Event::Text(status.into()),
        Event::EndElement,
        Event::EndElement,
    ])
}

fn ok(collection_id: &str, key: i64, items: &[CalItem]) -> Vec<u8> {
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
            push_event(&mut doc, it);
        }
        doc.push(Event::EndElement);
    }
    doc.push(Event::EndElement); // Collection
    doc.push(Event::EndElement); // Collections
    doc.push(Event::EndElement); // Sync
    encode(&doc)
}

fn push_event(doc: &mut Vec<Event>, it: &CalItem) {
    let a = page::AIR_SYNC;
    let c = page::CALENDAR;
    doc.push(Event::start(a, air_sync::ADD));
    doc.push(Event::start(a, air_sync::SERVER_ID));
    doc.push(Event::Text(it.id.to_string()));
    doc.push(Event::EndElement);
    doc.push(Event::start(a, air_sync::APPLICATION_DATA));
    push_text(doc, c, calendar::UID, &it.uid);
    if let Some(s) = &it.summary {
        push_text(doc, c, calendar::SUBJECT, s);
    }
    if let Some(l) = &it.location {
        push_text(doc, c, calendar::LOCATION, l);
    }
    if let Some(d) = it.dtstart {
        push_text(doc, c, calendar::START_TIME, &eas_dt(d));
    }
    if let Some(d) = it.dtend {
        push_text(doc, c, calendar::END_TIME, &eas_dt(d));
    }
    doc.push(Event::EndElement); // ApplicationData
    doc.push(Event::EndElement); // Add
}

fn push_text(doc: &mut Vec<Event>, page: u8, token: u8, text: &str) {
    doc.push(Event::start(page, token));
    doc.push(Event::Text(text.into()));
    doc.push(Event::EndElement);
}

/// EAS compact date-time `YYYYMMDDTHHMMSSZ` (MS-ASCAL StartTime/EndTime).
fn eas_dt(d: time::OffsetDateTime) -> String {
    let u = d.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        u.year(),
        u.month() as u8,
        u.day(),
        u.hour(),
        u.minute(),
        u.second()
    )
}

type CalRow = (
    Uuid,
    String,
    Option<String>,
    Option<String>,
    Option<time::OffsetDateTime>,
    Option<time::OffsetDateTime>,
);

async fn load_events(state: &AppState, tenant_id: Uuid, calendar_id: Uuid) -> Vec<CalItem> {
    let rows: Vec<CalRow> = sqlx::query_as(
        "SELECT id, uid, summary, location, dtstart, dtend FROM calendar_events \
         WHERE calendar_id = $1 AND tenant_id = $2 ORDER BY dtstart NULLS LAST LIMIT $3",
    )
    .bind(calendar_id)
    .bind(tenant_id)
    .bind(WINDOW_SIZE)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(id, uid, summary, location, dtstart, dtend)| CalItem {
            id,
            uid,
            summary,
            location,
            dtstart,
            dtend,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn eas_dt_is_compact_utc() {
        assert_eq!(
            eas_dt(datetime!(2026-06-02 14:30:05 UTC)),
            "20260602T143005Z"
        );
    }

    #[test]
    fn build_vevent_has_required_fields() {
        let v = build_vevent(
            "evt-1@x",
            Some("Standup"),
            Some("Room A"),
            Some("20260602T140000Z"),
            Some("20260602T143000Z"),
        );
        assert!(v.contains("BEGIN:VEVENT"));
        assert!(v.contains("UID:evt-1@x"));
        assert!(v.contains("SUMMARY:Standup"));
        assert!(v.contains("DTSTART:20260602T140000Z"));
        assert!(v.contains("END:VCALENDAR"));
    }

    #[test]
    fn build_vevent_omits_absent_fields() {
        let v = build_vevent("u", None, None, None, None);
        assert!(v.contains("UID:u"));
        assert!(!v.contains("SUMMARY:"));
        assert!(!v.contains("DTSTART:"));
    }

    #[test]
    fn ical_escape_handles_specials() {
        assert_eq!(ical_escape("a;b,c\\d"), "a\\;b\\,c\\\\d");
    }

    #[test]
    fn parse_calendar_commands_upsert_and_delete() {
        let a = page::AIR_SYNC;
        let c = page::CALENDAR;
        let body = encode(&[
            Event::start(a, air_sync::SYNC),
            Event::start(a, air_sync::COMMANDS),
            Event::start(a, air_sync::ADD),
            Event::start(a, air_sync::SERVER_ID),
            Event::Text("new-1".into()),
            Event::EndElement,
            Event::start(a, air_sync::APPLICATION_DATA),
            Event::start(c, calendar::UID),
            Event::Text("uid-1@x".into()),
            Event::EndElement,
            Event::start(c, calendar::SUBJECT),
            Event::Text("Lunch".into()),
            Event::EndElement,
            Event::EndElement, // ApplicationData
            Event::EndElement, // Add
            Event::start(a, air_sync::DELETE),
            Event::start(a, air_sync::SERVER_ID),
            Event::Text("evt-del".into()),
            Event::EndElement,
            Event::EndElement, // Delete
            Event::EndElement, // Commands
            Event::EndElement, // Sync
        ]);
        let cmds = parse_calendar_commands(&body);
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            CalCommand::Upsert { uid, summary, .. } => {
                assert_eq!(uid, "uid-1@x");
                assert_eq!(summary.as_deref(), Some("Lunch"));
            }
            CalCommand::Delete { .. } => panic!("expected upsert first"),
        }
        match &cmds[1] {
            CalCommand::Delete { event_id } => assert_eq!(event_id, "evt-del"),
            CalCommand::Upsert { .. } => panic!("expected delete second"),
        }
    }

    #[test]
    fn status_only_carries_status() {
        let events = decode(&status_only("8")).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::Text(t) if t == "8")));
    }

    #[test]
    fn ok_priming_has_no_commands() {
        let events = decode(&ok("cal:x", 1, &[])).unwrap();
        assert!(!events.iter().any(
            |e| matches!(e, Event::StartElement { token, .. } if *token == air_sync::COMMANDS)
        ));
    }

    #[test]
    fn ok_round_trips() {
        let bytes = ok("cal:x", 2, &[]);
        assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
    }
}

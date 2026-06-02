//! EAS Sync for the Calendar class (MS-ASCAL).
//!
//! Routed here when the Sync CollectionId carries the `cal:` prefix FolderSync
//! assigned. Maps `calendar_events` rows to EAS Calendar items (page 4) inside
//! the AirSync envelope. Read direction only for now (server→client adds), with
//! the same per-device rolling SyncKey as mail; client-side event creation is a
//! later refinement. Events come from the calendar service's tables (shared DB).

use expresso_wbxml::{
    encode,
    tokens::{air_sync, calendar, page},
    Event,
};
use uuid::Uuid;

use crate::eas::sync::SyncRequest;
use crate::state::AppState;

const WINDOW_SIZE: i64 = 100;

/// Build the Sync response for a `cal:` collection. `raw_collection_id` is the
/// full `cal:<uuid>` ServerId from the request; the UUID is the calendar id.
pub async fn calendar_sync_response(
    state: &AppState,
    tenant_id: Uuid,
    req: &SyncRequest,
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

    let key: i64 = req.sync_key.parse().unwrap_or(1);
    let items = load_events(state, tenant_id, calendar_id).await;
    ok(&req.collection_id, key + 1, &items)
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
    use expresso_wbxml::decode;
    use time::macros::datetime;

    #[test]
    fn eas_dt_is_compact_utc() {
        assert_eq!(
            eas_dt(datetime!(2026-06-02 14:30:05 UTC)),
            "20260602T143005Z"
        );
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

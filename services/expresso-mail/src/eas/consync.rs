//! EAS Sync for the Contacts class (MS-ASCNTC).
//!
//! Routed here when the Sync CollectionId carries the `con:` prefix FolderSync
//! assigned. Maps `contacts` rows to EAS Contact items (page 1) inside the
//! AirSync envelope. Read direction only for now; client-side contact creation
//! is a later refinement. Contacts come from the contacts service's shared
//! tables. Mirrors the calendar Sync shape.

use expresso_wbxml::{
    encode,
    tokens::{air_sync, contacts, page},
    Event,
};
use uuid::Uuid;

use crate::eas::sync::SyncRequest;
use crate::state::AppState;

const WINDOW_SIZE: i64 = 100;

/// Build the Sync response for a `con:` collection. The UUID after the prefix is
/// the address-book id.
pub async fn contacts_sync_response(
    state: &AppState,
    tenant_id: Uuid,
    req: &SyncRequest,
) -> Vec<u8> {
    let Some(uuid_str) = req.collection_id.strip_prefix("con:") else {
        return status_only("8");
    };
    let Ok(book_id) = Uuid::parse_str(uuid_str) else {
        return status_only("8");
    };

    if req.sync_key == "0" || req.sync_key.is_empty() {
        return ok(&req.collection_id, 1, &[]);
    }

    let key: i64 = req.sync_key.parse().unwrap_or(1);
    let items = load_contacts(state, tenant_id, book_id).await;
    ok(&req.collection_id, key + 1, &items)
}

struct ConItem {
    id: Uuid,
    full_name: Option<String>,
    given_name: Option<String>,
    family_name: Option<String>,
    organization: Option<String>,
    email_primary: Option<String>,
    phone_primary: Option<String>,
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

fn ok(collection_id: &str, key: i64, items: &[ConItem]) -> Vec<u8> {
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
            push_contact(&mut doc, it);
        }
        doc.push(Event::EndElement);
    }
    doc.push(Event::EndElement); // Collection
    doc.push(Event::EndElement); // Collections
    doc.push(Event::EndElement); // Sync
    encode(&doc)
}

fn push_contact(doc: &mut Vec<Event>, it: &ConItem) {
    let a = page::AIR_SYNC;
    let c = page::CONTACTS;
    doc.push(Event::start(a, air_sync::ADD));
    doc.push(Event::start(a, air_sync::SERVER_ID));
    doc.push(Event::Text(it.id.to_string()));
    doc.push(Event::EndElement);
    doc.push(Event::start(a, air_sync::APPLICATION_DATA));
    push_opt(doc, c, contacts::FILE_AS, it.full_name.as_deref());
    push_opt(doc, c, contacts::FIRST_NAME, it.given_name.as_deref());
    push_opt(doc, c, contacts::LAST_NAME, it.family_name.as_deref());
    push_opt(doc, c, contacts::COMPANY_NAME, it.organization.as_deref());
    push_opt(
        doc,
        c,
        contacts::EMAIL1_ADDRESS,
        it.email_primary.as_deref(),
    );
    push_opt(doc, c, contacts::MOBILE_PHONE, it.phone_primary.as_deref());
    doc.push(Event::EndElement); // ApplicationData
    doc.push(Event::EndElement); // Add
}

/// Append `<token>text</token>` only when `text` is present and non-empty.
fn push_opt(doc: &mut Vec<Event>, page: u8, token: u8, text: Option<&str>) {
    let Some(t) = text.filter(|s| !s.is_empty()) else {
        return;
    };
    doc.push(Event::start(page, token));
    doc.push(Event::Text(t.into()));
    doc.push(Event::EndElement);
}

type ConRow = (
    Uuid,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

async fn load_contacts(state: &AppState, tenant_id: Uuid, book_id: Uuid) -> Vec<ConItem> {
    let rows: Vec<ConRow> = sqlx::query_as(
        "SELECT id, full_name, given_name, family_name, organization, email_primary, phone_primary \
         FROM contacts WHERE addressbook_id = $1 AND tenant_id = $2 \
         ORDER BY COALESCE(full_name, '') LIMIT $3",
    )
    .bind(book_id)
    .bind(tenant_id)
    .bind(WINDOW_SIZE)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(
            |(
                id,
                full_name,
                given_name,
                family_name,
                organization,
                email_primary,
                phone_primary,
            )| {
                ConItem {
                    id,
                    full_name,
                    given_name,
                    family_name,
                    organization,
                    email_primary,
                    phone_primary,
                }
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use expresso_wbxml::decode;

    fn item() -> ConItem {
        ConItem {
            id: Uuid::nil(),
            full_name: Some("Alice Smith".into()),
            given_name: Some("Alice".into()),
            family_name: Some("Smith".into()),
            organization: None,
            email_primary: Some("alice@x.com".into()),
            phone_primary: None,
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
    fn contact_add_omits_empty_fields() {
        let events = decode(&ok("con:x", 2, &[item()])).unwrap();
        let texts: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"Alice Smith"));
        assert!(texts.contains(&"alice@x.com"));
        // organization + phone are None → no empty elements emitted.
        assert!(!texts.contains(&""));
    }

    #[test]
    fn ok_priming_has_no_commands() {
        let events = decode(&ok("con:x", 1, &[])).unwrap();
        assert!(!events.iter().any(
            |e| matches!(e, Event::StartElement { token, .. } if *token == air_sync::COMMANDS)
        ));
    }

    #[test]
    fn ok_round_trips() {
        let bytes = ok("con:x", 2, &[item()]);
        assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
    }
}

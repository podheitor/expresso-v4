//! EAS Sync for the Contacts class (MS-ASCNTC).
//!
//! Routed here when the Sync CollectionId carries the `con:` prefix FolderSync
//! assigned. Server→client: maps `contacts` rows to EAS Contact items (page 1).
//! Client→server: Add/Change/Delete are translated to a vCard and applied
//! THROUGH the contacts service's internal API (`/internal/contacts/items`) —
//! never writing the `contacts` table from mail. Mirrors the calendar bridge.

use expresso_wbxml::{
    decode, encode,
    tokens::{air_sync, contacts, page},
    Event,
};
use uuid::Uuid;

use crate::eas::sync::SyncRequest;
use crate::state::AppState;

const WINDOW_SIZE: i64 = 100;

/// Build the Sync response for a `con:` collection. `body` is the raw request
/// WBXML — needed to read client `<Commands>` (contact create/edit/delete).
pub async fn contacts_sync_response(
    state: &AppState,
    tenant_id: Uuid,
    req: &SyncRequest,
    body: &[u8],
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

    // Apply client→server contact changes THROUGH the contacts service's internal
    // API — never writing the contacts table directly from mail.
    apply_contact_changes(state, tenant_id, book_id, body).await;

    let key: i64 = req.sync_key.parse().unwrap_or(1);
    let items = load_contacts(state, tenant_id, book_id).await;
    ok(&req.collection_id, key + 1, &items)
}

/// A client-originated contact command parsed from the Sync request.
enum ConCommand {
    Upsert {
        uid: String,
        first: Option<String>,
        last: Option<String>,
        file_as: Option<String>,
        email: Option<String>,
        phone: Option<String>,
        company: Option<String>,
    },
    Delete {
        contact_id: String,
    },
}

/// Parse client `<Commands>` from a contacts Sync request. Add/Change collect
/// Contacts-page fields into an Upsert (UID falls back to ServerId); Delete
/// collects its ServerId. Depth-tracked so nested field closes don't flush early.
fn parse_contact_commands(body: &[u8]) -> Vec<ConCommand> {
    let Ok(events) = decode(body) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut mode: Option<bool> = None; // Some(true)=delete
    let mut cmd_depth: i32 = 0;
    let mut depth: i32 = 0;
    let mut server_id: Option<String> = None;
    let (mut first, mut last, mut file_as, mut email, mut phone, mut company) =
        (None, None, None, None, None, None);
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
                    first = None;
                    last = None;
                    file_as = None;
                    email = None;
                    phone = None;
                    company = None;
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
                    Some((page::CONTACTS, contacts::FIRST_NAME)) => first = Some(t.clone()),
                    Some((page::CONTACTS, contacts::LAST_NAME)) => last = Some(t.clone()),
                    Some((page::CONTACTS, contacts::FILE_AS)) => file_as = Some(t.clone()),
                    Some((page::CONTACTS, contacts::EMAIL1_ADDRESS)) => email = Some(t.clone()),
                    Some((page::CONTACTS, contacts::MOBILE_PHONE)) => phone = Some(t.clone()),
                    Some((page::CONTACTS, contacts::COMPANY_NAME)) => company = Some(t.clone()),
                    _ => {}
                }
                want = None;
            }
            Event::EndElement => {
                depth -= 1;
                want = None;
                if mode.is_some() && depth == cmd_depth {
                    match mode {
                        Some(true) if server_id.is_some() => out.push(ConCommand::Delete {
                            contact_id: server_id.take().unwrap(),
                        }),
                        Some(false) if server_id.is_some() || email.is_some() => {
                            out.push(ConCommand::Upsert {
                                uid: server_id.clone().unwrap_or_else(new_uid),
                                first: first.clone(),
                                last: last.clone(),
                                file_as: file_as.clone(),
                                email: email.clone(),
                                phone: phone.clone(),
                                company: company.clone(),
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

/// A synthetic UID for a brand-new contact the device created without one.
/// Deterministic-enough (no rand dep): derived from the field set is overkill —
/// the contacts service treats UID as the dedup key, and a new contact simply
/// needs a unique-ish value; the ServerId path covers edits.
fn new_uid() -> String {
    format!("eas-{}", Uuid::new_v4())
}

/// Apply parsed contact commands via the contacts service internal API.
/// Best-effort; no-op when `contacts_url` is unset.
async fn apply_contact_changes(state: &AppState, tenant_id: Uuid, book_id: Uuid, body: &[u8]) {
    let base = state.cfg().contacts_url.clone();
    if base.is_empty() {
        return;
    }
    let http = reqwest::Client::new();
    for cmd in parse_contact_commands(body) {
        let result = match cmd {
            ConCommand::Upsert {
                uid,
                first,
                last,
                file_as,
                email,
                phone,
                company,
            } => {
                let vcard = build_vcard(
                    &uid,
                    first.as_deref(),
                    last.as_deref(),
                    file_as.as_deref(),
                    email.as_deref(),
                    phone.as_deref(),
                    company.as_deref(),
                );
                http.post(format!("{base}/internal/contacts/items"))
                    .json(&serde_json::json!({
                        "tenant_id": tenant_id,
                        "addressbook_id": book_id,
                        "vcard_raw": vcard,
                    }))
                    .send()
                    .await
                    .map(|_| ())
            }
            ConCommand::Delete { contact_id } => http
                .delete(format!("{base}/internal/contacts/items/{contact_id}"))
                .query(&[("tenant_id", tenant_id.to_string())])
                .send()
                .await
                .map(|_| ()),
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, "EAS contact change forward failed");
        }
    }
}

/// Build a minimal vCard 3.0 from EAS Contact fields.
#[allow(clippy::too_many_arguments)]
fn build_vcard(
    uid: &str,
    first: Option<&str>,
    last: Option<&str>,
    file_as: Option<&str>,
    email: Option<&str>,
    phone: Option<&str>,
    company: Option<&str>,
) -> String {
    let fn_display = file_as
        .map(str::to_string)
        .or_else(|| match (first, last) {
            (Some(f), Some(l)) => Some(format!("{f} {l}")),
            (Some(f), None) => Some(f.to_string()),
            (None, Some(l)) => Some(l.to_string()),
            (None, None) => None,
        })
        .unwrap_or_else(|| "Unnamed".to_string());
    let mut s = String::from("BEGIN:VCARD\r\nVERSION:3.0\r\n");
    s.push_str(&format!("UID:{uid}\r\n"));
    s.push_str(&format!(
        "N:{};{};;;\r\n",
        vcard_escape(last.unwrap_or("")),
        vcard_escape(first.unwrap_or(""))
    ));
    s.push_str(&format!("FN:{}\r\n", vcard_escape(&fn_display)));
    if let Some(o) = company {
        s.push_str(&format!("ORG:{}\r\n", vcard_escape(o)));
    }
    if let Some(e) = email {
        s.push_str(&format!("EMAIL;TYPE=INTERNET:{e}\r\n"));
    }
    if let Some(p) = phone {
        s.push_str(&format!("TEL;TYPE=CELL:{p}\r\n"));
    }
    s.push_str("END:VCARD\r\n");
    s
}

/// Escape vCard TEXT special chars (RFC 6350 §3.4).
fn vcard_escape(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
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

    #[test]
    fn build_vcard_has_fields() {
        let v = build_vcard(
            "c-1",
            Some("Alice"),
            Some("Smith"),
            None,
            Some("a@x.com"),
            Some("+551199999"),
            Some("Acme"),
        );
        assert!(v.contains("BEGIN:VCARD"));
        assert!(v.contains("UID:c-1"));
        assert!(v.contains("FN:Alice Smith"));
        assert!(v.contains("N:Smith;Alice;;;"));
        assert!(v.contains("EMAIL;TYPE=INTERNET:a@x.com"));
        assert!(v.contains("ORG:Acme"));
    }

    #[test]
    fn build_vcard_file_as_overrides_fn() {
        let v = build_vcard(
            "u",
            Some("A"),
            Some("B"),
            Some("Custom Name"),
            None,
            None,
            None,
        );
        assert!(v.contains("FN:Custom Name"));
    }

    #[test]
    fn vcard_escape_handles_specials() {
        assert_eq!(vcard_escape("a;b,c"), "a\\;b\\,c");
    }

    #[test]
    fn parse_contact_commands_upsert_and_delete() {
        let a = page::AIR_SYNC;
        let c = page::CONTACTS;
        let body = encode(&[
            Event::start(a, air_sync::SYNC),
            Event::start(a, air_sync::COMMANDS),
            Event::start(a, air_sync::ADD),
            Event::start(a, air_sync::SERVER_ID),
            Event::Text("new-1".into()),
            Event::EndElement,
            Event::start(a, air_sync::APPLICATION_DATA),
            Event::start(c, contacts::FIRST_NAME),
            Event::Text("Bob".into()),
            Event::EndElement,
            Event::start(c, contacts::EMAIL1_ADDRESS),
            Event::Text("bob@x.com".into()),
            Event::EndElement,
            Event::EndElement, // ApplicationData
            Event::EndElement, // Add
            Event::start(a, air_sync::DELETE),
            Event::start(a, air_sync::SERVER_ID),
            Event::Text("con-del".into()),
            Event::EndElement,
            Event::EndElement, // Delete
            Event::EndElement, // Commands
            Event::EndElement, // Sync
        ]);
        let cmds = parse_contact_commands(&body);
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            ConCommand::Upsert {
                uid, email, first, ..
            } => {
                assert_eq!(uid, "new-1");
                assert_eq!(first.as_deref(), Some("Bob"));
                assert_eq!(email.as_deref(), Some("bob@x.com"));
            }
            ConCommand::Delete { .. } => panic!("expected upsert first"),
        }
        match &cmds[1] {
            ConCommand::Delete { contact_id } => assert_eq!(contact_id, "con-del"),
            ConCommand::Upsert { .. } => panic!("expected delete second"),
        }
    }

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

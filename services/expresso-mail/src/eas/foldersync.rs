//! EAS FolderSync command (MS-ASCMD §2.2.2.4) — mail folder hierarchy.
//!
//! The client sends `<FolderSync><SyncKey>K</SyncKey></FolderSync>`. On K="0"
//! (initial) we return the full hierarchy with a fresh key; on a non-zero key we
//! return an empty change set (the folder hierarchy is treated as static in the
//! MVP — no creates/renames over EAS), echoing the next key. Folders come from
//! the same `mailboxes` rows IMAP serves; `special_use` maps to EAS folder
//! types. Per-device delta state is deferred until folder mutations over EAS
//! land.

use expresso_wbxml::{
    decode, encode,
    tokens::{folder, page},
    Event,
};
use uuid::Uuid;

use crate::state::AppState;

/// The synckey we hand back after the initial sync. A static non-zero value is
/// sufficient while the hierarchy is read-only over EAS.
const NEXT_SYNC_KEY: &str = "1";

/// Map an IMAP `special_use` attribute to an EAS folder type (MS-ASCMD §2.2.3.170).
/// 1=Generic, 2=Inbox, 3=Drafts, 4=DeletedItems, 5=SentItems, 12=User-created mail.
fn folder_type(special_use: Option<&str>) -> &'static str {
    match special_use {
        Some(s) if s.eq_ignore_ascii_case("\\Inbox") => "2",
        Some(s) if s.eq_ignore_ascii_case("\\Drafts") => "3",
        Some(s) if s.eq_ignore_ascii_case("\\Trash") => "4",
        Some(s) if s.eq_ignore_ascii_case("\\Sent") => "5",
        _ => "12", // user-created mail folder
    }
}

/// Extract the `<SyncKey>` text from a FolderSync request body. Returns "0" when
/// the body is absent/unparseable (treat as initial sync) so a confused client
/// re-bootstraps rather than erroring.
pub fn parse_sync_key(body: &[u8]) -> String {
    let Ok(events) = decode(body) else {
        return "0".into();
    };
    let mut in_key = false;
    for ev in events {
        match ev {
            Event::StartElement { page: p, token, .. }
                if p == page::FOLDER_HIERARCHY && token == folder::SYNC_KEY =>
            {
                in_key = true;
            }
            Event::Text(t) if in_key => return t,
            _ => in_key = false,
        }
    }
    "0".into()
}

/// Build the FolderSync response for a user's subscribed mailboxes.
pub async fn foldersync_response(
    state: &AppState,
    user_id: Uuid,
    tenant_id: Uuid,
    sync_key: &str,
) -> Vec<u8> {
    let p = page::FOLDER_HIERARCHY;
    let mut doc = vec![
        Event::start(p, folder::FOLDER_SYNC),
        Event::start(p, folder::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
        Event::start(p, folder::SYNC_KEY),
        Event::Text(NEXT_SYNC_KEY.into()),
        Event::EndElement,
    ];

    // Initial sync (key "0") emits every folder as an Add; a non-zero key gets an
    // empty change set (static hierarchy in the MVP).
    let folders = if sync_key == "0" {
        load_folders(state, user_id, tenant_id).await
    } else {
        Vec::new()
    };

    doc.push(Event::start(p, folder::CHANGES));
    doc.push(Event::start(p, folder::COUNT));
    doc.push(Event::Text(folders.len().to_string()));
    doc.push(Event::EndElement);
    for f in &folders {
        doc.push(Event::start(p, folder::ADD));
        doc.push(Event::start(p, folder::SERVER_ID));
        doc.push(Event::Text(f.server_id.clone()));
        doc.push(Event::EndElement);
        doc.push(Event::start(p, folder::PARENT_ID));
        doc.push(Event::Text("0".into())); // flat hierarchy: all folders under root
        doc.push(Event::EndElement);
        doc.push(Event::start(p, folder::DISPLAY_NAME));
        doc.push(Event::Text(f.display_name.clone()));
        doc.push(Event::EndElement);
        doc.push(Event::start(p, folder::TYPE));
        doc.push(Event::Text(f.folder_type.into()));
        doc.push(Event::EndElement);
        doc.push(Event::EndElement); // Add
    }
    doc.push(Event::EndElement); // Changes
    doc.push(Event::EndElement); // FolderSync

    encode(&doc)
}

/// An EAS folder to advertise. `server_id` is the opaque id echoed back on Sync;
/// mail uses the bare mailbox UUID (so the mail Sync parses it directly) while
/// calendar/contacts carry a `cal:`/`con:` prefix the Sync router dispatches on.
struct FolderRow {
    server_id: String,
    display_name: String,
    folder_type: &'static str,
}

async fn load_folders(state: &AppState, user_id: Uuid, tenant_id: Uuid) -> Vec<FolderRow> {
    let mut out = Vec::new();

    // Mail folders (from mailboxes; ServerId = bare mailbox UUID).
    let mail: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, folder_name, special_use FROM mailboxes \
         WHERE user_id = $1 AND tenant_id = $2 AND subscribed = TRUE \
         ORDER BY folder_name",
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    for (id, name, special_use) in mail {
        out.push(FolderRow {
            server_id: id.to_string(),
            display_name: name,
            folder_type: folder_type(special_use.as_deref()),
        });
    }

    // Calendar collections (EAS type 8), prefixed `cal:`.
    let cals: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM calendars WHERE owner_user_id = $1 AND tenant_id = $2 ORDER BY name",
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    for (id, name) in cals {
        out.push(FolderRow {
            server_id: format!("cal:{id}"),
            display_name: name,
            folder_type: "8",
        });
    }

    // Contacts collections (EAS type 9), prefixed `con:`.
    let books: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, name FROM addressbooks WHERE owner_user_id = $1 AND tenant_id = $2 ORDER BY name",
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    for (id, name) in books {
        out.push(FolderRow {
            server_id: format!("con:{id}"),
            display_name: name,
            folder_type: "9",
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_type_maps_special_use() {
        assert_eq!(folder_type(Some("\\Inbox")), "2");
        assert_eq!(folder_type(Some("\\drafts")), "3");
        assert_eq!(folder_type(Some("\\Trash")), "4");
        assert_eq!(folder_type(Some("\\Sent")), "5");
        assert_eq!(folder_type(None), "12");
        assert_eq!(folder_type(Some("\\Junk")), "12");
    }

    #[test]
    fn parse_sync_key_reads_zero() {
        let req = encode(&[
            Event::start(page::FOLDER_HIERARCHY, folder::FOLDER_SYNC),
            Event::start(page::FOLDER_HIERARCHY, folder::SYNC_KEY),
            Event::Text("0".into()),
            Event::EndElement,
            Event::EndElement,
        ]);
        assert_eq!(parse_sync_key(&req), "0");
    }

    #[test]
    fn parse_sync_key_reads_nonzero() {
        let req = encode(&[
            Event::start(page::FOLDER_HIERARCHY, folder::FOLDER_SYNC),
            Event::start(page::FOLDER_HIERARCHY, folder::SYNC_KEY),
            Event::Text("7".into()),
            Event::EndElement,
            Event::EndElement,
        ]);
        assert_eq!(parse_sync_key(&req), "7");
    }

    #[test]
    fn parse_sync_key_defaults_zero_on_garbage() {
        assert_eq!(parse_sync_key(&[0xFF, 0x00, 0x01]), "0");
        assert_eq!(parse_sync_key(&[]), "0");
    }
}

//! EAS Ping command (MS-ASCMD §2.2.2.13) — Direct Push.
//!
//! The client sends `<Ping><HeartbeatInterval>N</HeartbeatInterval><Folders>
//! <Folder><Id>…</Id>…`. We snapshot each watched folder's `MAX(mod_sequence)`
//! and poll on a short interval until one advances (→ Status 2 + the changed
//! folder ids, telling the client to Sync) or the heartbeat elapses (→ Status 1,
//! no changes). `mod_sequence` is the same monotonic counter IMAP CONDSTORE uses,
//! so any add/flag/delete bumps it.

use std::time::Duration;

use expresso_wbxml::{
    decode, encode,
    tokens::{page, ping},
    Event,
};
use uuid::Uuid;

use crate::state::AppState;

/// Poll cadence while waiting for a change. Short enough to feel like push,
/// long enough not to hammer the DB.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Clamp the client heartbeat to a sane range (seconds).
const MIN_HEARTBEAT: u64 = 60;
const MAX_HEARTBEAT: u64 = 3540; // just under the common 1h proxy timeout

/// Parsed Ping request: heartbeat + the folder ids to watch.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PingRequest {
    pub heartbeat: Option<u64>,
    pub folder_ids: Vec<String>,
}

/// Parse `<HeartbeatInterval>` + `<Folder><Id>` values from a Ping request.
pub fn parse_ping(body: &[u8]) -> PingRequest {
    let Ok(events) = decode(body) else {
        return PingRequest::default();
    };
    let mut req = PingRequest::default();
    let mut field: Option<u8> = None;
    for ev in events {
        match ev {
            Event::StartElement { page: p, token, .. } if p == page::PING => field = Some(token),
            Event::Text(t) => {
                match field {
                    Some(ping::HEARTBEAT_INTERVAL) => req.heartbeat = t.parse().ok(),
                    Some(ping::ID) => req.folder_ids.push(t),
                    _ => {}
                }
                field = None;
            }
            _ => field = None,
        }
    }
    req
}

/// Run the Ping: long-poll the watched folders, returning the WBXML response.
/// `sleep` is injected so tests can drive it without real time.
pub async fn ping_response(state: &AppState, tenant_id: Uuid, req: &PingRequest) -> Vec<u8> {
    let folders: Vec<Uuid> = req
        .folder_ids
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    if folders.is_empty() {
        // Status 3 = missing/invalid parameters (no valid folders to watch).
        return ping_status(3, &[]);
    }
    let heartbeat = req
        .heartbeat
        .unwrap_or(MIN_HEARTBEAT)
        .clamp(MIN_HEARTBEAT, MAX_HEARTBEAT);

    let baseline: Vec<i64> = {
        let mut v = Vec::with_capacity(folders.len());
        for f in &folders {
            v.push(max_modseq(state, tenant_id, *f).await);
        }
        v
    };

    let deadline = heartbeat / POLL_INTERVAL.as_secs();
    for _ in 0..deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        let mut changed = Vec::new();
        for (i, f) in folders.iter().enumerate() {
            if max_modseq(state, tenant_id, *f).await > baseline[i] {
                changed.push(f.to_string());
            }
        }
        if !changed.is_empty() {
            // Status 2 = changes occurred; the client should issue Sync.
            return ping_status(2, &changed);
        }
    }
    // Status 1 = heartbeat expired with no changes.
    ping_status(1, &[])
}

/// Build a Ping response with `status` and any changed folder ids.
fn ping_status(status: u8, changed: &[String]) -> Vec<u8> {
    let p = page::PING;
    let mut doc = vec![
        Event::start(p, ping::PING),
        Event::start(p, ping::STATUS),
        Event::Text(status.to_string()),
        Event::EndElement,
    ];
    if !changed.is_empty() {
        doc.push(Event::start(p, ping::FOLDERS));
        for id in changed {
            doc.push(Event::start(p, ping::FOLDER));
            doc.push(Event::Text(id.clone()));
            doc.push(Event::EndElement);
        }
        doc.push(Event::EndElement); // Folders
    }
    doc.push(Event::EndElement); // Ping
    encode(&doc)
}

async fn max_modseq(state: &AppState, tenant_id: Uuid, mailbox_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(MAX(mod_sequence), 0) FROM messages \
         WHERE mailbox_id = $1 AND tenant_id = $2",
    )
    .bind(mailbox_id)
    .bind(tenant_id)
    .fetch_one(state.db())
    .await
    .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ping_reads_heartbeat_and_folders() {
        let p = page::PING;
        let body = encode(&[
            Event::start(p, ping::PING),
            Event::start(p, ping::HEARTBEAT_INTERVAL),
            Event::Text("120".into()),
            Event::EndElement,
            Event::start(p, ping::FOLDERS),
            Event::start(p, ping::FOLDER),
            Event::start(p, ping::ID),
            Event::Text("folder-a".into()),
            Event::EndElement,
            Event::EndElement,
            Event::start(p, ping::FOLDER),
            Event::start(p, ping::ID),
            Event::Text("folder-b".into()),
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
        ]);
        let req = parse_ping(&body);
        assert_eq!(req.heartbeat, Some(120));
        assert_eq!(req.folder_ids, vec!["folder-a", "folder-b"]);
    }

    #[test]
    fn parse_ping_garbage_is_default() {
        assert_eq!(parse_ping(&[0xFF, 0x00]), PingRequest::default());
    }

    #[test]
    fn ping_status_no_changes_omits_folders() {
        let events = decode(&ping_status(1, &[])).unwrap();
        assert!(!events
            .iter()
            .any(|e| matches!(e, Event::StartElement { token, .. } if *token == ping::FOLDERS)));
    }

    #[test]
    fn ping_status_with_changes_lists_folders() {
        let events = decode(&ping_status(2, &["f1".into(), "f2".into()])).unwrap();
        let texts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"2"));
        assert!(texts.contains(&"f1"));
        assert!(texts.contains(&"f2"));
    }

    #[test]
    fn ping_status_round_trips() {
        let bytes = ping_status(2, &["x".into()]);
        assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
    }
}

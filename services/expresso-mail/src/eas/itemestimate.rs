//! EAS GetItemEstimate command (MS-ASCMD §2.2.2.9).
//!
//! Clients call this before Sync to learn how many items a collection will
//! return, so they can show progress. We parse the CollectionId(s) and return an
//! estimate per collection: for mail, the count of messages newer than the
//! high-water UID already synced to that device; for calendar/contacts, the
//! collection's row count (advisory — the client re-syncs regardless). The
//! estimate need not be exact per the spec.

use expresso_wbxml::{
    decode, encode,
    tokens::{air_sync, item_estimate, page},
    Event,
};
use uuid::Uuid;

use crate::state::AppState;

/// Parse the CollectionId values from a GetItemEstimate request. The request
/// nests them under GetItemEstimate>Collections>Collection>CollectionId, but the
/// CollectionId tag lives on the AirSync page — scanning for it across pages is
/// enough for the MVP.
pub fn parse_collection_ids(body: &[u8]) -> Vec<String> {
    let Ok(events) = decode(body) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    let mut want = false;
    for ev in events {
        match ev {
            Event::StartElement { page: p, token, .. }
                if (p == page::AIR_SYNC && token == air_sync::COLLECTION_ID)
                    || (p == page::GET_ITEM_ESTIMATE && token == item_estimate::COLLECTION_ID) =>
            {
                want = true;
            }
            Event::Text(t) if want => {
                ids.push(t);
                want = false;
            }
            _ => want = false,
        }
    }
    ids
}

/// Build the GetItemEstimate response: one Response>Collection>Estimate per
/// requested collection. `device_id` scopes the mail high-water lookup.
pub async fn item_estimate_response(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    device_id: &str,
    collection_ids: &[String],
) -> Vec<u8> {
    let g = page::GET_ITEM_ESTIMATE;
    let mut doc = vec![Event::start(g, item_estimate::GET_ITEM_ESTIMATE)];
    for cid in collection_ids {
        let estimate = estimate_for(state, tenant_id, user_id, device_id, cid).await;
        doc.push(Event::start(g, item_estimate::RESPONSE));
        doc.push(Event::start(g, item_estimate::STATUS));
        doc.push(Event::Text("1".into()));
        doc.push(Event::EndElement);
        doc.push(Event::start(g, item_estimate::COLLECTION));
        doc.push(Event::start(g, item_estimate::COLLECTION_ID));
        doc.push(Event::Text(cid.clone()));
        doc.push(Event::EndElement);
        doc.push(Event::start(g, item_estimate::ESTIMATE));
        doc.push(Event::Text(estimate.to_string()));
        doc.push(Event::EndElement);
        doc.push(Event::EndElement); // Collection
        doc.push(Event::EndElement); // Response
    }
    doc.push(Event::EndElement); // GetItemEstimate
    encode(&doc)
}

/// Estimate the pending item count for one collection id, dispatching on the
/// same `cal:`/`con:`/bare-UUID prefix Sync uses.
async fn estimate_for(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    device_id: &str,
    cid: &str,
) -> i64 {
    if let Some(uuid) = cid
        .strip_prefix("cal:")
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        return count(
            state,
            "SELECT COUNT(*) FROM calendar_events WHERE calendar_id = $1 AND tenant_id = $2",
            uuid,
            tenant_id,
        )
        .await;
    }
    if let Some(uuid) = cid
        .strip_prefix("con:")
        .and_then(|s| Uuid::parse_str(s).ok())
    {
        return count(
            state,
            "SELECT COUNT(*) FROM contacts WHERE addressbook_id = $1 AND tenant_id = $2",
            uuid,
            tenant_id,
        )
        .await;
    }
    let Ok(mailbox_id) = Uuid::parse_str(cid) else {
        return 0;
    };
    // Mail: messages newer than the device's high-water UID for this folder.
    let last_uid: i64 = sqlx::query_scalar(
        "SELECT last_uid FROM eas_sync_state \
         WHERE tenant_id = $1 AND user_id = $2 AND device_id = $3 AND collection_id = $4",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(device_id)
    .bind(mailbox_id)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten()
    .unwrap_or(0);
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND uid > $3",
    )
    .bind(mailbox_id)
    .bind(tenant_id)
    .bind(last_uid)
    .fetch_one(state.db())
    .await
    .unwrap_or(0)
}

async fn count(state: &AppState, sql: &str, id: Uuid, tenant_id: Uuid) -> i64 {
    sqlx::query_scalar(sql)
        .bind(id)
        .bind(tenant_id)
        .fetch_one(state.db())
        .await
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_collection_ids_reads_airsync_id() {
        let a = page::AIR_SYNC;
        let g = page::GET_ITEM_ESTIMATE;
        let body = encode(&[
            Event::start(g, item_estimate::GET_ITEM_ESTIMATE),
            Event::start(g, item_estimate::COLLECTION),
            Event::start(a, air_sync::COLLECTION_ID),
            Event::Text("cal:abc".into()),
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
        ]);
        assert_eq!(parse_collection_ids(&body), vec!["cal:abc"]);
    }

    #[test]
    fn parse_collection_ids_reads_estimate_page_id() {
        let g = page::GET_ITEM_ESTIMATE;
        let body = encode(&[
            Event::start(g, item_estimate::GET_ITEM_ESTIMATE),
            Event::start(g, item_estimate::COLLECTION_ID),
            Event::Text("mbox-1".into()),
            Event::EndElement,
            Event::EndElement,
        ]);
        assert_eq!(parse_collection_ids(&body), vec!["mbox-1"]);
    }

    #[test]
    fn parse_collection_ids_empty_on_garbage() {
        assert!(parse_collection_ids(&[0xFF, 0x00]).is_empty());
    }
}

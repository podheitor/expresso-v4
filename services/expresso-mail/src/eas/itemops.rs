//! EAS ItemOperations command (MS-ASCMD §2.2.2.8) — Fetch a specific item.
//!
//! Clients use ItemOperations>Fetch to pull one item's full content on demand
//! (e.g. the untruncated body after a truncated Sync). We support the mail
//! Fetch: given a CollectionId (mailbox UUID) + ServerId (`mboxid:uid`), return
//! the full plain-text body. GetAttachment / Documents-store fetches are later
//! refinements.

use base64::Engine;
use expresso_wbxml::{
    decode, encode,
    tokens::{air_sync_base, item_operations as iop, page},
    Event,
};
use mail_parser::{MessageParser, MimeHeaders};
use uuid::Uuid;

use crate::state::AppState;

/// One parsed Fetch request entry. A Fetch targets either a whole item
/// (CollectionId + ServerId → body) or an attachment (FileReference → bytes).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FetchReq {
    pub collection_id: Option<String>,
    pub server_id: Option<String>,
    /// AirSyncBase FileReference (`mboxid:uid:attidx`) for an attachment fetch.
    pub file_reference: Option<String>,
}

impl FetchReq {
    /// A Fetch is usable when it targets an attachment (FileReference) or a whole
    /// item (CollectionId + ServerId).
    fn is_complete(&self) -> bool {
        self.file_reference.is_some() || (self.collection_id.is_some() && self.server_id.is_some())
    }
}

/// Parse the Fetch entries from an ItemOperations request. Each `<Fetch>` may
/// carry CollectionId+ServerId (item) and/or a FileReference (attachment).
pub fn parse_fetches(body: &[u8]) -> Vec<FetchReq> {
    let Ok(events) = decode(body) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur: Option<FetchReq> = None;
    let mut want: Option<(u8, u8)> = None; // (page, token) of the leaf text
    for ev in events {
        match &ev {
            Event::StartElement { page: p, token, .. }
                if *p == page::ITEM_OPERATIONS && *token == iop::FETCH =>
            {
                cur = Some(FetchReq::default());
                want = None;
            }
            Event::StartElement { page: p, token, .. } if cur.is_some() => {
                want = Some((*p, *token));
            }
            Event::Text(t) => {
                if let Some(f) = cur.as_mut() {
                    match want {
                        Some((page::ITEM_OPERATIONS, iop::COLLECTION_ID)) => {
                            f.collection_id = Some(t.clone());
                        }
                        Some((page::ITEM_OPERATIONS, iop::SERVER_ID)) => {
                            f.server_id = Some(t.clone());
                        }
                        Some((page::AIR_SYNC_BASE, air_sync_base::FILE_REFERENCE)) => {
                            f.file_reference = Some(t.clone());
                        }
                        _ => {}
                    }
                }
                want = None;
            }
            Event::EndElement => {
                // Close of <Fetch>: flush when it carried a usable target.
                if cur.as_ref().is_some_and(FetchReq::is_complete) {
                    out.push(cur.take().unwrap());
                }
                want = None;
            }
            _ => want = None,
        }
    }
    out
}

/// Run the fetches and build the ItemOperations response.
pub async fn item_operations_response(
    state: &AppState,
    tenant_id: Uuid,
    fetches: &[FetchReq],
) -> Vec<u8> {
    let i = page::ITEM_OPERATIONS;
    let mut doc = vec![
        Event::start(i, iop::ITEM_OPERATIONS),
        Event::start(i, iop::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
    ];
    for f in fetches {
        if let Some(fref) = &f.file_reference {
            push_attachment_fetch(&mut doc, state, tenant_id, fref).await;
        } else {
            push_item_fetch(&mut doc, state, tenant_id, f).await;
        }
    }
    doc.push(Event::EndElement); // ItemOperations
    encode(&doc)
}

/// Emit a `<Response><Fetch>` for a whole-item (body) fetch.
async fn push_item_fetch(doc: &mut Vec<Event>, state: &AppState, tenant_id: Uuid, f: &FetchReq) {
    let i = page::ITEM_OPERATIONS;
    let b = page::AIR_SYNC_BASE;
    let body = fetch_body_text(state, tenant_id, f).await;
    doc.push(Event::start(i, iop::RESPONSE));
    doc.push(Event::start(i, iop::FETCH));
    push_text(doc, i, iop::STATUS, if body.is_some() { "1" } else { "15" });
    if let Some(c) = &f.collection_id {
        push_text(doc, i, iop::COLLECTION_ID, c);
    }
    if let Some(s) = &f.server_id {
        push_text(doc, i, iop::SERVER_ID, s);
    }
    if let Some(text) = body {
        doc.push(Event::start(i, iop::PROPERTIES));
        doc.push(Event::start(b, air_sync_base::BODY));
        push_text(doc, b, air_sync_base::TYPE, "1");
        push_text(doc, b, air_sync_base::TRUNCATED, "0");
        push_text(doc, b, air_sync_base::DATA, &text);
        doc.push(Event::EndElement); // Body
        doc.push(Event::EndElement); // Properties
    }
    doc.push(Event::EndElement); // Fetch
    doc.push(Event::EndElement); // Response
}

/// Emit a `<Response><Fetch>` for an attachment fetch (FileReference). The
/// bytes are base64-encoded into the Data element (EAS carries attachment data
/// base64 in ItemOperations).
async fn push_attachment_fetch(
    doc: &mut Vec<Event>,
    state: &AppState,
    tenant_id: Uuid,
    file_reference: &str,
) {
    let i = page::ITEM_OPERATIONS;
    let b = page::AIR_SYNC_BASE;
    let att = fetch_attachment(state, tenant_id, file_reference).await;
    doc.push(Event::start(i, iop::RESPONSE));
    doc.push(Event::start(i, iop::FETCH));
    push_text(doc, i, iop::STATUS, if att.is_some() { "1" } else { "15" });
    push_text(doc, b, air_sync_base::FILE_REFERENCE, file_reference);
    if let Some((content_type, bytes)) = att {
        doc.push(Event::start(i, iop::PROPERTIES));
        push_text(doc, b, air_sync_base::CONTENT_TYPE, &content_type);
        push_text(
            doc,
            b,
            air_sync_base::DATA,
            &base64::engine::general_purpose::STANDARD.encode(&bytes),
        );
        doc.push(Event::EndElement); // Properties
    }
    doc.push(Event::EndElement); // Fetch
    doc.push(Event::EndElement); // Response
}

/// Resolve a FileReference `mboxid:uid:attidx` to (content_type, bytes) of the
/// attachment, or `None` when any segment is unparseable or the part is missing.
async fn fetch_attachment(
    state: &AppState,
    tenant_id: Uuid,
    file_reference: &str,
) -> Option<(String, Vec<u8>)> {
    let mut parts = file_reference.split(':');
    let mailbox_id = Uuid::parse_str(parts.next()?).ok()?;
    let uid: i64 = parts.next()?.parse().ok()?;
    let att_idx: usize = parts.next()?.parse().ok()?;
    let body_path: String = sqlx::query_scalar(
        "SELECT body_path FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND uid = $3",
    )
    .bind(mailbox_id)
    .bind(tenant_id)
    .bind(uid)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten()?;
    let raw = crate::pop3::store::fetch_body(state, &body_path).await?;
    let msg = MessageParser::default().parse(&raw)?;
    let part = msg.attachments().nth(att_idx)?;
    let content_type = part
        .content_type()
        .map(|ct| match &ct.c_subtype {
            Some(sub) => format!("{}/{sub}", ct.c_type),
            None => ct.c_type.to_string(),
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());
    Some((content_type, part.contents().to_vec()))
}

fn push_text(doc: &mut Vec<Event>, page: u8, token: u8, text: &str) {
    doc.push(Event::start(page, token));
    doc.push(Event::Text(text.into()));
    doc.push(Event::EndElement);
}

/// Resolve a Fetch to the message's full plain-text body, or `None` if the
/// ServerId/mailbox is unparseable or the message/body is missing.
async fn fetch_body_text(state: &AppState, tenant_id: Uuid, f: &FetchReq) -> Option<String> {
    let uid: i64 = f.server_id.as_ref()?.rsplit_once(':')?.1.parse().ok()?;
    let mailbox_id = Uuid::parse_str(f.collection_id.as_ref()?).ok()?;
    let body_path: String = sqlx::query_scalar(
        "SELECT body_path FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND uid = $3",
    )
    .bind(mailbox_id)
    .bind(tenant_id)
    .bind(uid)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten()?;
    let raw = crate::pop3::store::fetch_body(state, &body_path).await?;
    Some(body_after_headers(&raw))
}

/// Plain-text body section (after the `\r\n\r\n` separator), lossy UTF-8.
fn body_after_headers(raw: &[u8]) -> String {
    let body = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(raw, |pos| &raw[pos + 4..]);
    String::from_utf8_lossy(body).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fetches_reads_pair() {
        let i = page::ITEM_OPERATIONS;
        let body = encode(&[
            Event::start(i, iop::ITEM_OPERATIONS),
            Event::start(i, iop::FETCH),
            Event::start(i, iop::COLLECTION_ID),
            Event::Text("mbox-1".into()),
            Event::EndElement,
            Event::start(i, iop::SERVER_ID),
            Event::Text("mbox-1:42".into()),
            Event::EndElement,
            Event::EndElement, // Fetch
            Event::EndElement, // ItemOperations
        ]);
        assert_eq!(
            parse_fetches(&body),
            vec![FetchReq {
                collection_id: Some("mbox-1".into()),
                server_id: Some("mbox-1:42".into()),
                file_reference: None,
            }]
        );
    }

    #[test]
    fn parse_fetches_reads_file_reference() {
        let i = page::ITEM_OPERATIONS;
        let b = page::AIR_SYNC_BASE;
        let body = encode(&[
            Event::start(i, iop::ITEM_OPERATIONS),
            Event::start(i, iop::FETCH),
            Event::start(b, air_sync_base::FILE_REFERENCE),
            Event::Text("mbox-1:42:0".into()),
            Event::EndElement,
            Event::EndElement, // Fetch
            Event::EndElement, // ItemOperations
        ]);
        assert_eq!(
            parse_fetches(&body),
            vec![FetchReq {
                collection_id: None,
                server_id: None,
                file_reference: Some("mbox-1:42:0".into()),
            }]
        );
    }

    #[test]
    fn parse_fetches_empty_on_garbage() {
        assert!(parse_fetches(&[0xFF, 0x00]).is_empty());
    }

    #[test]
    fn fetch_req_is_complete_rules() {
        assert!(FetchReq {
            file_reference: Some("x".into()),
            ..Default::default()
        }
        .is_complete());
        assert!(FetchReq {
            collection_id: Some("c".into()),
            server_id: Some("s".into()),
            ..Default::default()
        }
        .is_complete());
        assert!(!FetchReq {
            collection_id: Some("c".into()),
            ..Default::default()
        }
        .is_complete());
    }

    #[test]
    fn body_after_headers_strips_headers() {
        assert_eq!(body_after_headers(b"H: v\r\n\r\nbody"), "body");
        assert_eq!(body_after_headers(b"nosep"), "nosep");
    }

    #[test]
    fn parse_fetches_two_entries() {
        let i = page::ITEM_OPERATIONS;
        let entry = |c: &str, s: &str| {
            [
                Event::start(i, iop::FETCH),
                Event::start(i, iop::COLLECTION_ID),
                Event::Text(c.into()),
                Event::EndElement,
                Event::start(i, iop::SERVER_ID),
                Event::Text(s.into()),
                Event::EndElement,
                Event::EndElement,
            ]
        };
        let mut evs = vec![Event::start(i, iop::ITEM_OPERATIONS)];
        evs.extend(entry("m1", "m1:1"));
        evs.extend(entry("m2", "m2:2"));
        evs.push(Event::EndElement);
        let got = parse_fetches(&encode(&evs));
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].server_id.as_deref(), Some("m2:2"));
    }
}

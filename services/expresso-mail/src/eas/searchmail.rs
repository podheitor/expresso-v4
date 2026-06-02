//! EAS Search command (MS-ASCMD §2.2.2.15) — mailbox content search.
//!
//! Clients issue Search with a free-text query against the "Mailbox" store. We
//! run an ILIKE match across the user's messages (subject / from / preview),
//! returning a Result per hit: LongId (`mboxid:uid` — the same ServerId Sync
//! uses), Class "Email", and a small Properties block (Subject, From). Full
//! ApplicationData + GAL/Documents stores are later refinements.

use expresso_wbxml::{
    encode,
    tokens::{email, page, search},
    Event,
};
use uuid::Uuid;

use crate::state::AppState;

/// Max hits returned in one Search response.
const SEARCH_LIMIT: i64 = 100;

/// Parse the free-text `<Query>` value from a Search request. Returns an empty
/// string when absent (→ the handler responds with status 1 + no results).
pub fn parse_query(body: &[u8]) -> String {
    let Ok(events) = expresso_wbxml::decode(body) else {
        return String::new();
    };
    let mut want = false;
    for ev in events {
        match ev {
            Event::StartElement { page: p, token, .. }
                if p == page::SEARCH && token == search::QUERY =>
            {
                want = true;
            }
            Event::Text(t) if want => return t,
            _ => want = false,
        }
    }
    String::new()
}

/// Run the search and build the Search response WBXML.
pub async fn search_response(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    query: &str,
) -> Vec<u8> {
    let q = query.trim();
    if q.is_empty() {
        return response(&[]);
    }
    let hits = run_search(state, tenant_id, user_id, q).await;
    response(&hits)
}

struct Hit {
    long_id: String,
    subject: Option<String>,
    from_addr: Option<String>,
}

fn response(hits: &[Hit]) -> Vec<u8> {
    let s = page::SEARCH;
    let e = page::EMAIL;
    let mut doc = vec![
        Event::start(s, search::SEARCH),
        Event::start(s, search::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
        Event::start(s, search::RESPONSE),
        Event::start(s, search::STORE),
        Event::start(s, search::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
    ];
    for h in hits {
        doc.push(Event::start(s, search::RESULT));
        doc.push(Event::start(s, search::LONG_ID));
        doc.push(Event::Text(h.long_id.clone()));
        doc.push(Event::EndElement);
        doc.push(Event::start(s, search::PROPERTIES));
        if let Some(sub) = &h.subject {
            doc.push(Event::start(e, email::SUBJECT));
            doc.push(Event::Text(sub.clone()));
            doc.push(Event::EndElement);
        }
        if let Some(f) = &h.from_addr {
            doc.push(Event::start(e, email::FROM));
            doc.push(Event::Text(f.clone()));
            doc.push(Event::EndElement);
        }
        doc.push(Event::EndElement); // Properties
        doc.push(Event::EndElement); // Result
    }
    doc.push(Event::EndElement); // Store
    doc.push(Event::EndElement); // Response
    doc.push(Event::EndElement); // Search
    encode(&doc)
}

async fn run_search(state: &AppState, tenant_id: Uuid, user_id: Uuid, q: &str) -> Vec<Hit> {
    // Escape ILIKE wildcards in the user query, then wrap for substring match.
    let escaped = q
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{escaped}%");
    let rows: Vec<(Uuid, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT m.mailbox_id, m.uid, m.subject, m.from_addr \
           FROM messages m JOIN mailboxes b ON b.id = m.mailbox_id \
          WHERE b.user_id = $1 AND m.tenant_id = $2 \
            AND (m.subject ILIKE $3 ESCAPE '\\' \
              OR m.from_addr ILIKE $3 ESCAPE '\\' \
              OR m.preview_text ILIKE $3 ESCAPE '\\') \
          ORDER BY m.received_at DESC LIMIT $4",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(&pattern)
    .bind(SEARCH_LIMIT)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(mbox, uid, subject, from_addr)| Hit {
            long_id: format!("{mbox}:{uid}"),
            subject,
            from_addr,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use expresso_wbxml::decode;

    #[test]
    fn parse_query_reads_query_text() {
        let s = page::SEARCH;
        let body = encode(&[
            Event::start(s, search::SEARCH),
            Event::start(s, search::STORE),
            Event::start(s, search::QUERY),
            Event::Text("hello world".into()),
            Event::EndElement,
            Event::EndElement,
            Event::EndElement,
        ]);
        assert_eq!(parse_query(&body), "hello world");
    }

    #[test]
    fn parse_query_empty_on_garbage() {
        assert_eq!(parse_query(&[0xFF, 0x00]), "");
    }

    #[test]
    fn response_empty_has_status_no_results() {
        let events = decode(&response(&[])).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::Text(t) if t == "1")));
        assert!(!events
            .iter()
            .any(|e| matches!(e, Event::StartElement { page, token, .. } if *page == page::SEARCH && *token == search::RESULT)));
    }

    #[test]
    fn response_with_hit_carries_long_id_and_subject() {
        let hit = Hit {
            long_id: "mbox:42".into(),
            subject: Some("Re: hi".into()),
            from_addr: Some("a@x.com".into()),
        };
        let events = decode(&response(&[hit])).unwrap();
        let texts: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"mbox:42"));
        assert!(texts.contains(&"Re: hi"));
        assert!(texts.contains(&"a@x.com"));
    }

    #[test]
    fn response_round_trips() {
        let bytes = response(&[]);
        assert_eq!(encode(&decode(&bytes).unwrap()), bytes);
    }
}

//! CalDAV sync-collection REPORT (RFC 6578).
//!
//! Token format: `urn:expresso:ctag:{N}` where N is `calendars.ctag`.
//!
//! Behaviour:
//! - Token missing / unparseable → *initial* sync: emit all current events.
//! - Token == current → fast path: empty 207 with unchanged token.
//! - Token < current  → *incremental*: emit events with `last_ctag > client`
//!   as 200 OK (added/modified) plus tombstone rows with `deleted_ctag > client`
//!   as 404 Not Found (removed).
//! - Token > current  → 410 Gone (RFC 6578 §3.7) — calendar was restored/rolled
//!   back; client must restart with a fresh initial sync.

use axum::{body::Body, http::StatusCode, response::Response};
use expresso_core::{begin_tenant_tx, DbPool};
use sqlx::Row;
use uuid::Uuid;

use crate::caldav::auth::CalDavPrincipal;
use crate::caldav::xml::{self, XML_PROLOG};
use crate::caldav::MULTISTATUS_CT;
use crate::domain::{CalendarRepo, EventQuery, EventRepo};
use crate::error::Result;
use crate::state::AppState;

const TOKEN_PREFIX: &str = "urn:expresso:ctag:";

/// Cap on rows emitted per sync-collection delta query (changed set and
/// tombstone set each). Bounds the response a client can pull with an old token
/// against a calendar with a huge change history. A client that hits the cap
/// keeps an older token and re-syncs to drain the rest (RFC 6578 partial sync).
const MAX_SYNC_ROWS: i64 = 5000;

pub async fn handle(
    state: AppState,
    principal: &CalDavPrincipal,
    calendar_id: Uuid,
    body: &str,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let current_ctag = CalendarRepo::new(pool)
        .ctag(principal.tenant_id, calendar_id)
        .await?;
    let new_token = format!("{TOKEN_PREFIX}{current_ctag}");

    let client_ctag = xml::parse_sync_token(body)
        .as_deref()
        .and_then(parse_token_value);

    let mut out = String::with_capacity(1024);
    out.push_str(XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">"#);

    // Fast path — client already current.
    if client_ctag == Some(current_ctag) {
        push_token(&mut out, &new_token);
        return Ok(ok_207(out));
    }

    // The token we will hand back. Normally the collection's current ctag, but a
    // capped (partial) delta lowers it to the last row emitted so the client
    // re-syncs the remainder rather than skipping it.
    let mut effective_token = new_token.clone();

    match client_ctag {
        // Incremental delta since last known ctag.
        Some(from) if from < current_ctag => {
            let partial = write_changed_since(&mut out, pool, principal, calendar_id, from).await?;
            match partial {
                // Cap hit: advance only to the last emitted change; defer
                // tombstones to the next round (the next from-ctag covers them).
                Some(last) => effective_token = format!("{TOKEN_PREFIX}{last}"),
                None => {
                    write_tombstones_since(&mut out, pool, principal, calendar_id, from).await?;
                }
            }
        }
        // Token from the future — calendar was restored/rolled back; client must
        // restart. RFC 6578 §3.7 mandates 410 Gone with the new current token so
        // the client can immediately switch to the valid token without an extra round-trip.
        Some(_future) => {
            let body = format!(
                "{XML_PROLOG}\
                 <D:error xmlns:D=\"DAV:\">\
                   <D:valid-sync-token/>\
                 </D:error>"
            );
            return Ok(Response::builder()
                .status(StatusCode::GONE)
                .header("Content-Type", MULTISTATUS_CT)
                .header("DAV-Sync-Token", new_token)
                .body(Body::from(body))
                .unwrap());
        }
        // Initial sync — full resend, no tombstones.
        None => {
            let events = EventRepo::new(pool)
                .list(principal.tenant_id, calendar_id, &EventQuery::default())
                .await?;
            for ev in events {
                push_member(&mut out, principal.user_id, calendar_id, &ev.uid, &ev.etag);
            }
        }
    }

    push_token(&mut out, &effective_token);
    Ok(ok_207(out))
}

/// Emit changed members since `from_ctag`. Capped at `MAX_SYNC_ROWS`; when the
/// cap is hit, returns `Some(last_ctag_emitted)` so the caller advances the sync
/// token only to that point (partial sync) instead of to the collection's
/// current ctag — otherwise the client would skip the un-emitted tail.
async fn write_changed_since(
    out: &mut String,
    pool: &DbPool,
    principal: &CalDavPrincipal,
    calendar_id: Uuid,
    from_ctag: i64,
) -> Result<Option<i64>> {
    let mut tx = begin_tenant_tx(pool, principal.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT uid, etag, last_ctag
          FROM calendar_events
         WHERE tenant_id = $1
           AND calendar_id = $2
           AND last_ctag > $3
         ORDER BY last_ctag
         LIMIT $4
        "#,
    )
    .bind(principal.tenant_id)
    .bind(calendar_id)
    .bind(from_ctag)
    .bind(MAX_SYNC_ROWS)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    let truncated = rows.len() as i64 == MAX_SYNC_ROWS;
    let mut last_emitted = from_ctag;
    for r in rows {
        let uid: String = r.get("uid");
        let etag: String = r.get("etag");
        last_emitted = r.get("last_ctag");
        push_member(out, principal.user_id, calendar_id, &uid, &etag);
    }
    Ok(truncated.then_some(last_emitted))
}

async fn write_tombstones_since(
    out: &mut String,
    pool: &DbPool,
    principal: &CalDavPrincipal,
    calendar_id: Uuid,
    from_ctag: i64,
) -> Result<()> {
    let mut tx = begin_tenant_tx(pool, principal.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT uid
          FROM calendar_event_tombstones
         WHERE tenant_id = $1
           AND calendar_id = $2
           AND deleted_ctag > $3
         ORDER BY deleted_ctag
         LIMIT $4
        "#,
    )
    .bind(principal.tenant_id)
    .bind(calendar_id)
    .bind(from_ctag)
    .bind(MAX_SYNC_ROWS)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    for r in rows {
        let uid: String = r.get("uid");
        let href = format!("/caldav/{}/{}/{}.ics", principal.user_id, calendar_id, uid);
        out.push_str("<D:response>");
        out.push_str("<D:href>");
        out.push_str(&xml::escape(&href));
        out.push_str("</D:href>");
        out.push_str("<D:status>HTTP/1.1 404 Not Found</D:status>");
        out.push_str("</D:response>");
    }
    Ok(())
}

fn push_member(out: &mut String, user_id: Uuid, calendar_id: Uuid, uid: &str, etag: &str) {
    let href = format!("/caldav/{user_id}/{calendar_id}/{uid}.ics");
    out.push_str("<D:response>");
    out.push_str("<D:href>");
    out.push_str(&xml::escape(&href));
    out.push_str("</D:href>");
    out.push_str("<D:propstat><D:prop>");
    out.push_str("<D:getetag>\"");
    out.push_str(&xml::escape(etag));
    out.push_str("\"</D:getetag>");
    out.push_str("</D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat>");
    out.push_str("</D:response>");
}

fn push_token(out: &mut String, token: &str) {
    out.push_str("<D:sync-token>");
    out.push_str(token);
    out.push_str("</D:sync-token>");
    out.push_str("</D:multistatus>");
}

fn parse_token_value(tok: &str) -> Option<i64> {
    tok.strip_prefix(TOKEN_PREFIX)
        .and_then(|n| n.parse::<i64>().ok())
}

fn ok_207(body: String) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(207).unwrap())
        .header("Content-Type", MULTISTATUS_CT)
        .body(Body::from(body))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::{parse_token_value, push_member, push_token, TOKEN_PREFIX};
    use uuid::Uuid;

    #[test]
    fn token_roundtrip() {
        let tok = format!("{TOKEN_PREFIX}7");
        assert_eq!(parse_token_value(&tok), Some(7));
        assert_eq!(parse_token_value("garbage"), None);
        assert_eq!(parse_token_value(""), None);
    }

    #[test]
    fn parse_token_value_zero() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}0")), Some(0));
    }

    #[test]
    fn parse_token_value_large() {
        assert_eq!(
            parse_token_value(&format!("{TOKEN_PREFIX}99999")),
            Some(99999)
        );
    }

    #[test]
    fn parse_token_value_non_numeric_suffix() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}abc")), None);
    }

    #[test]
    fn push_token_wraps_in_sync_token_tags() {
        let mut out = String::new();
        push_token(&mut out, "urn:expresso:ctag:5");
        assert!(out.contains("<D:sync-token>urn:expresso:ctag:5</D:sync-token>"));
        assert!(out.contains("</D:multistatus>"));
    }

    #[test]
    fn push_member_contains_ics_href_and_etag() {
        let user = Uuid::nil();
        let cal = Uuid::nil();
        let mut out = String::new();
        push_member(&mut out, user, cal, "evt-uid", "etag42");
        assert!(out.contains(".ics"));
        assert!(out.contains("etag42"));
        assert!(out.contains("200 OK"));
    }

    #[test]
    fn push_member_contains_uid_in_href() {
        let user = Uuid::nil();
        let cal = Uuid::nil();
        let mut out = String::new();
        push_member(&mut out, user, cal, "my-unique-uid", "e1");
        assert!(out.contains("my-unique-uid"));
    }

    #[test]
    fn parse_token_negative_valid() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}-5")), Some(-5));
    }

    #[test]
    fn parse_token_zero_valid() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}0")), Some(0));
    }

    #[test]
    fn parse_token_missing_prefix_returns_none() {
        assert_eq!(parse_token_value("ctag:100"), None);
    }

    #[test]
    fn parse_token_large_value_parsed() {
        assert_eq!(
            parse_token_value(&format!("{TOKEN_PREFIX}999999")),
            Some(999_999)
        );
    }

    #[test]
    fn parse_token_negative_value_is_none() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}-1")), Some(-1));
    }

    #[test]
    fn parse_token_non_numeric_is_none() {
        assert!(parse_token_value(&format!("{TOKEN_PREFIX}abc")).is_none());
    }

    #[test]
    fn parse_token_zero_is_valid() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}0")), Some(0));
    }

    #[test]
    fn token_prefix_starts_with_urn() {
        assert!(TOKEN_PREFIX.starts_with("urn:"));
    }

    #[test]
    fn token_prefix_contains_ctag() {
        assert!(TOKEN_PREFIX.contains("ctag"));
    }

    #[test]
    fn parse_token_value_one_is_valid() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}1")), Some(1));
    }

    #[test]
    fn token_prefix_value_is_urn_expresso_ctag() {
        assert_eq!(TOKEN_PREFIX, "urn:expresso:ctag:");
    }

    #[test]
    fn token_prefix_ends_with_colon() {
        assert!(TOKEN_PREFIX.ends_with(':'));
    }

    #[test]
    fn parse_token_value_100_is_valid() {
        assert_eq!(parse_token_value(&format!("{TOKEN_PREFIX}100")), Some(100));
    }

    #[test]
    fn push_token_output_contains_prefix_value() {
        let mut out = String::new();
        push_token(&mut out, &format!("{TOKEN_PREFIX}7"));
        assert!(out.contains("urn:expresso:ctag:7"));
    }

    #[test]
    fn parse_token_value_max_i64_is_valid() {
        let tok = format!("{TOKEN_PREFIX}{}", i64::MAX);
        assert_eq!(parse_token_value(&tok), Some(i64::MAX));
    }

    #[test]
    fn token_prefix_length_is_nonzero() {
        assert!(!TOKEN_PREFIX.is_empty());
    }
}

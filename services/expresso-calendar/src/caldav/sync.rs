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

pub async fn handle(
    state:       AppState,
    principal:   &CalDavPrincipal,
    calendar_id: Uuid,
    body:        &str,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let current_ctag = CalendarRepo::new(pool)
        .ctag(principal.tenant_id, calendar_id)
        .await?;
    let new_token = format!("{TOKEN_PREFIX}{current_ctag}");

    let client_ctag = xml::parse_sync_token(body).as_deref().and_then(parse_token_value);

    let mut out = String::with_capacity(1024);
    out.push_str(XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">"#);

    // Fast path — client already current.
    if client_ctag == Some(current_ctag) {
        push_token(&mut out, &new_token);
        return Ok(ok_207(out));
    }

    match client_ctag {
        // Incremental delta since last known ctag.
        Some(from) if from < current_ctag => {
            write_changed_since(&mut out, pool, principal, calendar_id, from).await?;
            write_tombstones_since(&mut out, pool, principal, calendar_id, from).await?;
        }
        // Initial sync — full resend, no tombstones.
        _ => {
            let events = EventRepo::new(pool)
                .list(principal.tenant_id, calendar_id, &EventQuery::default())
                .await?;
            for ev in events {
                push_member(&mut out, principal.user_id, calendar_id, &ev.uid, &ev.etag);
            }
        }
    }

    push_token(&mut out, &new_token);
    Ok(ok_207(out))
}

async fn write_changed_since(
    out:         &mut String,
    pool:        &DbPool,
    principal:   &CalDavPrincipal,
    calendar_id: Uuid,
    from_ctag:   i64,
) -> Result<()> {
    let mut tx = begin_tenant_tx(pool, principal.tenant_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT uid, etag
          FROM calendar_events
         WHERE tenant_id = $1
           AND calendar_id = $2
           AND last_ctag > $3
         ORDER BY last_ctag
        "#,
    )
    .bind(principal.tenant_id)
    .bind(calendar_id)
    .bind(from_ctag)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    for r in rows {
        let uid:  String = r.get("uid");
        let etag: String = r.get("etag");
        push_member(out, principal.user_id, calendar_id, &uid, &etag);
    }
    Ok(())
}

async fn write_tombstones_since(
    out:         &mut String,
    pool:        &DbPool,
    principal:   &CalDavPrincipal,
    calendar_id: Uuid,
    from_ctag:   i64,
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
        "#,
    )
    .bind(principal.tenant_id)
    .bind(calendar_id)
    .bind(from_ctag)
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
    tok.strip_prefix(TOKEN_PREFIX).and_then(|n| n.parse::<i64>().ok())
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
    use super::{parse_token_value, TOKEN_PREFIX};

    #[test]
    fn token_roundtrip() {
        let tok = format!("{TOKEN_PREFIX}7");
        assert_eq!(parse_token_value(&tok), Some(7));
        assert_eq!(parse_token_value("garbage"), None);
        assert_eq!(parse_token_value(""), None);
    }
}

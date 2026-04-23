//! CardDAV sync-collection REPORT (RFC 6578).
//!
//! Token format: `urn:expresso:ctag:{N}` where N = addressbook.ctag.
//!
//! Behaviour:
//! - Token missing / unparseable → *initial* sync: emit all current contacts.
//! - Token == current → fast path: empty 207 with unchanged token.
//! - Token < current  → *incremental*: contacts with `last_ctag > client` as
//!   200 OK, plus `contact_tombstones` with `deleted_ctag > client` as 404.

use axum::{body::Body, http::StatusCode, response::Response};
use expresso_core::DbPool;
use sqlx::Row;
use uuid::Uuid;

use crate::carddav::auth::CardDavPrincipal;
use crate::carddav::xml::{self, XML_PROLOG};
use crate::carddav::MULTISTATUS_CT;
use crate::domain::{AddressbookRepo, ContactRepo};
use crate::error::Result;
use crate::state::AppState;

const TOKEN_PREFIX: &str = "urn:expresso:ctag:";

pub async fn handle(
    state:          AppState,
    principal:      &CardDavPrincipal,
    addressbook_id: Uuid,
    body:           &str,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let current_ctag = AddressbookRepo::new(pool)
        .ctag(principal.tenant_id, addressbook_id)
        .await?;
    let new_token = format!("{TOKEN_PREFIX}{current_ctag}");

    let client_ctag = xml::parse_sync_token(body).as_deref().and_then(parse_token_value);

    let mut out = String::with_capacity(1024);
    out.push_str(XML_PROLOG);
    out.push_str(r#"<D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:carddav">"#);

    if client_ctag == Some(current_ctag) {
        push_token(&mut out, &new_token);
        return Ok(ok_207(out));
    }

    match client_ctag {
        Some(from) if from < current_ctag => {
            write_changed_since(&mut out, pool, principal, addressbook_id, from).await?;
            write_tombstones_since(&mut out, pool, principal, addressbook_id, from).await?;
        }
        _ => {
            let contacts = ContactRepo::new(pool)
                .list(principal.tenant_id, addressbook_id)
                .await?;
            for c in contacts {
                push_member(&mut out, principal.user_id, addressbook_id, &c.uid, &c.etag);
            }
        }
    }

    push_token(&mut out, &new_token);
    Ok(ok_207(out))
}

async fn write_changed_since(
    out:            &mut String,
    pool:           &DbPool,
    principal:      &CardDavPrincipal,
    addressbook_id: Uuid,
    from_ctag:      i64,
) -> Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT uid, etag
          FROM contacts
         WHERE tenant_id = $1
           AND addressbook_id = $2
           AND last_ctag > $3
         ORDER BY last_ctag
        "#,
    )
    .bind(principal.tenant_id)
    .bind(addressbook_id)
    .bind(from_ctag)
    .fetch_all(pool)
    .await?;

    for r in rows {
        let uid:  String = r.get("uid");
        let etag: String = r.get("etag");
        push_member(out, principal.user_id, addressbook_id, &uid, &etag);
    }
    Ok(())
}

async fn write_tombstones_since(
    out:            &mut String,
    pool:           &DbPool,
    principal:      &CardDavPrincipal,
    addressbook_id: Uuid,
    from_ctag:      i64,
) -> Result<()> {
    let rows = sqlx::query(
        r#"
        SELECT uid
          FROM contact_tombstones
         WHERE tenant_id = $1
           AND addressbook_id = $2
           AND deleted_ctag > $3
         ORDER BY deleted_ctag
        "#,
    )
    .bind(principal.tenant_id)
    .bind(addressbook_id)
    .bind(from_ctag)
    .fetch_all(pool)
    .await?;

    for r in rows {
        let uid: String = r.get("uid");
        let href = format!("/carddav/{}/{}/{}.vcf", principal.user_id, addressbook_id, uid);
        out.push_str("<D:response>");
        out.push_str("<D:href>");
        out.push_str(&xml::escape(&href));
        out.push_str("</D:href>");
        out.push_str("<D:status>HTTP/1.1 404 Not Found</D:status>");
        out.push_str("</D:response>");
    }
    Ok(())
}

fn push_member(out: &mut String, user_id: Uuid, addressbook_id: Uuid, uid: &str, etag: &str) {
    let href = format!("/carddav/{user_id}/{addressbook_id}/{uid}.vcf");
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
        let tok = format!("{TOKEN_PREFIX}5");
        assert_eq!(parse_token_value(&tok), Some(5));
        assert_eq!(parse_token_value("junk"), None);
    }
}

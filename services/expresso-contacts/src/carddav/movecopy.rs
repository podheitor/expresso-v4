//! CardDAV MOVE + COPY verbs (RFC 4918 §9.8 / §9.9).
//!
//! Scope: contact resources only (URI `/carddav/<user>/<addressbook>/<uid>.vcf`).
//! - Destination URI must resolve to same authenticated user.
//! - Both source + destination must be Contact targets.
//! - Cross-addressbook allowed (same user, same tenant).
//! - `Overwrite: F` → if destination exists, 412.

use axum::{body::Body, http::{HeaderMap, StatusCode}, response::Response};

use crate::carddav::auth::CardDavPrincipal;
use crate::carddav::uri::{self, Target};
use crate::domain::ContactRepo;
use crate::error::Result;
use crate::state::AppState;

pub async fn copy(
    state:     AppState,
    principal: CardDavPrincipal,
    path:      &str,
    headers:   &HeaderMap,
) -> Result<Response> {
    process(state, principal, path, headers, false).await
}

pub async fn mov(
    state:     AppState,
    principal: CardDavPrincipal,
    path:      &str,
    headers:   &HeaderMap,
) -> Result<Response> {
    process(state, principal, path, headers, true).await
}

async fn process(
    state:     AppState,
    principal: CardDavPrincipal,
    path:      &str,
    headers:   &HeaderMap,
    is_move:   bool,
) -> Result<Response> {
    let (src_ab, src_uid) = match uri::classify(path) {
        Target::Contact { user_id, addressbook_id, uid } if user_id == principal.user_id =>
            (addressbook_id, uid),
        Target::Contact { .. } => return Ok(simple(StatusCode::FORBIDDEN)),
        _ => return Ok(simple(StatusCode::NOT_FOUND)),
    };

    let dest_raw = match headers.get("destination").and_then(|h| h.to_str().ok()) {
        Some(s) => s.trim().to_string(),
        None => return Ok(bad_request("missing Destination header")),
    };
    let dest_path = strip_origin(&dest_raw);

    let (dst_ab, dst_uid) = match uri::classify(&dest_path) {
        Target::Contact { user_id, addressbook_id, uid } if user_id == principal.user_id =>
            (addressbook_id, uid),
        Target::Contact { .. } => return Ok(simple(StatusCode::FORBIDDEN)),
        _ => return Ok(bad_request("destination must resolve to a contact URI")),
    };

    let overwrite = headers.get("overwrite").and_then(|h| h.to_str().ok()).map(|s| s.trim());
    let allow_overwrite = !matches!(overwrite, Some("F") | Some("f"));

    let pool = state.db_or_unavailable()?;
    let repo = ContactRepo::new(pool);

    let src = repo.get_by_uid(principal.tenant_id, src_ab, &src_uid).await?;

    let dst_existed = repo
        .get_by_uid(principal.tenant_id, dst_ab, &dst_uid)
        .await
        .is_ok();
    if dst_existed && !allow_overwrite {
        return Ok(simple(StatusCode::PRECONDITION_FAILED));
    }

    let _ = repo
        .replace_by_uid(principal.tenant_id, dst_ab, &src.vcard_raw)
        .await?;

    let same_row = src_ab == dst_ab && src.uid == dst_uid;
    if is_move && !same_row {
        repo.delete_by_uid(principal.tenant_id, src_ab, &src.uid).await?;
    }

    let status = if dst_existed { StatusCode::NO_CONTENT } else { StatusCode::CREATED };
    Ok(simple(status))
}

fn strip_origin(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) {
        match rest.find('/') {
            Some(i) => rest[i..].to_string(),
            None => "/".to_string(),
        }
    } else {
        url.to_string()
    }
}

fn simple(status: StatusCode) -> Response {
    Response::builder().status(status).body(Body::empty()).unwrap()
}

fn bad_request(msg: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from(msg))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::strip_origin;

    #[test]
    fn strip_absolute() {
        assert_eq!(strip_origin("http://h:8003/carddav/u/a/x.vcf"), "/carddav/u/a/x.vcf");
        assert_eq!(strip_origin("/carddav/u/a/x.vcf"), "/carddav/u/a/x.vcf");
    }
}

//! Extended MKCOL (RFC 5689) — cria addressbook via CardDAV.

use axum::{body::Body, http::StatusCode, response::Response};

use crate::carddav::auth::CardDavPrincipal;
use crate::carddav::uri::{classify, Target};
use crate::domain::{AddressbookRepo, NewAddressbook};
use crate::state::AppState;

pub async fn handle(
    state:     AppState,
    principal: CardDavPrincipal,
    path:      &str,
    body:      &str,
) -> Response {
    let (user_id, ab_id) = match classify(path) {
        Target::Addressbook { user_id, addressbook_id } => (user_id, addressbook_id),
        _ => return bad_request("MKCOL requires /carddav/<user-uuid>/<ab-uuid>/ URL"),
    };
    if user_id != principal.user_id {
        return forbidden("principal mismatch");
    }
    // Verifica resourcetype contém "addressbook" — ≠ MKCOL genérico p/ outra coisa.
    if !body.is_empty() && !body.contains("addressbook") {
        return error(StatusCode::BAD_REQUEST, "resourcetype must include addressbook");
    }
    let Some(pool) = state.db() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "db unavailable");
    };
    let repo = AddressbookRepo::new(pool);
    if repo.get(principal.tenant_id, ab_id).await.is_ok() {
        return error(StatusCode::METHOD_NOT_ALLOWED, "resource already exists");
    }
    let name = extract_prop(body, "displayname")
        .unwrap_or_else(|| format!("Addressbook {}", &ab_id.to_string()[..8]));
    let description = extract_prop(body, "addressbook-description");

    let input = NewAddressbook { name, description, is_default: false };
    match repo.create_with_id(ab_id, principal.tenant_id, principal.user_id, input).await {
        Ok(_) => created(),
        Err(e) => {
            tracing::warn!(error = %e, "MKCOL addressbook insert failed");
            error(StatusCode::CONFLICT, "could not create addressbook")
        }
    }
}

fn extract_prop(body: &str, local_name: &str) -> Option<String> {
    let bytes = body.as_bytes();
    let mut search_start = 0;
    while let Some(rel) = body[search_start..].find(local_name) {
        let name_pos = search_start + rel;
        let after = bytes.get(name_pos + local_name.len()).copied();
        if !matches!(after, Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/')) {
            search_start = name_pos + local_name.len();
            continue;
        }
        let before = if name_pos > 0 { bytes.get(name_pos - 1).copied() } else { None };
        let valid_start = matches!(before, Some(b'<') | Some(b':'));
        if !valid_start { search_start = name_pos + local_name.len(); continue; }
        let rest = &body[name_pos + local_name.len()..];
        let gt = match rest.find('>') { Some(g) => g, None => return None };
        if rest.as_bytes().get(gt.saturating_sub(1)) == Some(&b'/') { return None; }
        let content_start = name_pos + local_name.len() + gt + 1;
        let tail = &body[content_start..];
        let close_plain = format!("</{local_name}>");
        let mut close_pos = tail.find(&close_plain);
        let mut i = 0;
        while let Some(lt) = tail[i..].find("</") {
            let abs = i + lt + 2;
            let seg_end = match tail[abs..].find('>') { Some(e) => e, None => break };
            let seg = &tail[abs..abs + seg_end];
            let is_match = seg == local_name
                || (seg.ends_with(local_name)
                    && seg.len() > local_name.len()
                    && &seg[seg.len() - local_name.len() - 1..seg.len() - local_name.len()] == ":");
            if is_match {
                let found = i + lt;
                if close_pos.map_or(true, |p| found < p) { close_pos = Some(found); }
                break;
            }
            i = abs + seg_end + 1;
        }
        let c = match close_pos { Some(c) => c, None => return None };
        let s = tail[..c].trim()
            .replace("&amp;", "&")
            .replace("&lt;",  "<")
            .replace("&gt;",  ">")
            .replace("&quot;","\"")
            .replace("&apos;","'");
        return if s.is_empty() { None } else { Some(s) };
    }
    None
}

fn created() -> Response {
    Response::builder().status(StatusCode::CREATED).body(Body::empty()).unwrap()
}

fn bad_request(msg: &'static str) -> Response {
    Response::builder().status(StatusCode::BAD_REQUEST).body(Body::from(msg)).unwrap()
}

fn forbidden(msg: &'static str) -> Response {
    Response::builder().status(StatusCode::FORBIDDEN).body(Body::from(msg)).unwrap()
}

fn error(status: StatusCode, msg: &'static str) -> Response {
    Response::builder().status(status).body(Body::from(msg)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::extract_prop;
    #[test]
    fn displayname_prefixed() {
        let b = r#"<D:prop><D:displayname>Amigos</D:displayname></D:prop>"#;
        assert_eq!(extract_prop(b, "displayname").as_deref(), Some("Amigos"));
    }
    #[test]
    fn displayname_plain() {
        let b = r#"<displayname>Equipe</displayname>"#;
        assert_eq!(extract_prop(b, "displayname").as_deref(), Some("Equipe"));
    }
}

//! MKCALENDAR (RFC 4791 §5.3.1) — cria nova coleção calendar.

use axum::{body::Body, http::StatusCode, response::Response};

use crate::caldav::auth::CalDavPrincipal;
use crate::caldav::uri::{classify, Target};
use crate::domain::{CalendarRepo, NewCalendar};
use crate::state::AppState;

/// Extrai displayname / description / color / timezone de body XML MKCALENDAR.
/// Parser minimalista — procura tags por substring. Suficiente p/ Thunderbird,
/// Apple Calendar, DAVx5. Body pode ser vazio (servidor aplica defaults).
fn extract_prop(body: &str, local_name: &str) -> Option<String> {
    // Encontra tag abertura: "<local>" ou "<prefix:local>" (possivelmente com atributos).
    // Estratégia: para cada ocorrência de local_name no body, verifica se forma uma tag de abertura válida.
    let bytes = body.as_bytes();
    let mut search_start = 0;
    while let Some(rel) = body[search_start..].find(local_name) {
        let name_pos = search_start + rel;
        // Verifica char após o nome: deve ser '>' (sem atributos) ou whitespace (com attrs) ou '/' (self-close).
        let after = bytes.get(name_pos + local_name.len()).copied();
        if !matches!(after, Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/')) {
            search_start = name_pos + local_name.len();
            continue;
        }
        // Verifica char antes: deve ser '<' (sem prefixo) ou ':' (com prefixo).
        let before = if name_pos > 0 { bytes.get(name_pos - 1).copied() } else { None };
        let valid_start = match before {
            Some(b'<') => true,
            Some(b':') if name_pos >= 2 => {
                // Rastreia até o '<' — só válido se só houver caracteres de prefixo XML entre '<' e ':'.
                let slice = &bytes[..name_pos];
                matches!(slice.iter().rposition(|&c| c == b'<'), Some(_))
            },
            _ => false,
        };
        if !valid_start {
            search_start = name_pos + local_name.len();
            continue;
        }
        // Avança até '>' que fecha a tag de abertura.
        let rest = &body[name_pos + local_name.len()..];
        let gt = match rest.find('>') { Some(g) => g, None => return None };
        // Se a tag é self-closing "/>" → sem conteúdo.
        if rest.as_bytes().get(gt.saturating_sub(1)) == Some(&b'/') {
            return None;
        }
        let content_start = name_pos + local_name.len() + gt + 1;
        // Procura tag de fechamento: "</local_name>" ou "</prefix:local_name>".
        let tail = &body[content_start..];
        // Primeiro cenário: </local_name>
        let close_plain = format!("</{local_name}>");
        let mut close_pos: Option<usize> = None;
        if let Some(c) = tail.find(&close_plain) {
            close_pos = Some(c);
        }
        // Tenta "</X:local_name>" — deve começar em "</" e conter ":local_name>"
        let mut i = 0;
        while let Some(lt) = tail[i..].find("</") {
            let abs = i + lt + 2; // após "</"
            // lê até '>' ou '/'
            let seg_end = match tail[abs..].find('>') { Some(e) => e, None => break };
            let segment = &tail[abs..abs + seg_end];
            // segment = "prefix:local_name" ou "local_name"
            let matches_close = segment == local_name
                || (segment.ends_with(local_name)
                    && segment.len() > local_name.len()
                    && &segment[segment.len() - local_name.len() - 1..segment.len() - local_name.len()] == ":");
            if matches_close {
                let found = i + lt;
                if close_pos.map_or(true, |p| found < p) {
                    close_pos = Some(found);
                }
                break;
            }
            i = abs + seg_end + 1;
        }
        let c = match close_pos { Some(c) => c, None => return None };
        let raw = &tail[..c].trim();
        let s = unescape_xml(raw);
        return if s.is_empty() { None } else { Some(s) };
    }
    None
}

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&")
     .replace("&lt;", "<")
     .replace("&gt;", ">")
     .replace("&quot;", "\"")
     .replace("&apos;", "'")
}

pub async fn handle(
    state:     AppState,
    principal: CalDavPrincipal,
    path:      &str,
    body:      &str,
) -> Response {
    // Target deve ser Calendar (URL completa c/ user+cal UUID).
    let (user_id, calendar_id) = match classify(path) {
        Target::Calendar { user_id, calendar_id } => (user_id, calendar_id),
        _ => return bad_request("MKCALENDAR requires /caldav/<user-uuid>/<cal-uuid>/ URL"),
    };
    if user_id != principal.user_id {
        return forbidden("principal mismatch");
    }
    let Some(pool) = state.db() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "db unavailable");
    };
    let repo = CalendarRepo::new(pool);

    // Se já existe → 405 Method Not Allowed (per RFC 4918 §9.3.1 analogia MKCOL).
    if repo.get(principal.tenant_id, calendar_id).await.is_ok() {
        return error(StatusCode::METHOD_NOT_ALLOWED, "resource already exists");
    }

    let name = extract_prop(body, "displayname")
        .unwrap_or_else(|| format!("Calendar {}", &calendar_id.to_string()[..8]));
    let description = extract_prop(body, "calendar-description");
    let color       = extract_prop(body, "calendar-color");
    let timezone    = extract_prop(body, "calendar-timezone")
        .and_then(|tz| extract_tzid(&tz));

    let input = NewCalendar {
        name, description, color, timezone, is_default: false,
    };
    match repo.create_with_id(calendar_id, principal.tenant_id, principal.user_id, &input).await {
        Ok(_) => created(),
        Err(e) => {
            tracing::warn!(error = %e, "MKCALENDAR insert failed");
            error(StatusCode::CONFLICT, "could not create calendar")
        }
    }
}

/// calendar-timezone traz um VCALENDAR completo. Extrai TZID primário.
fn extract_tzid(ics: &str) -> Option<String> {
    for line in ics.lines() {
        if let Some(rest) = line.trim().strip_prefix("TZID:") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn created() -> Response {
    Response::builder()
        .status(StatusCode::CREATED)
        .body(Body::empty())
        .unwrap()
}

fn bad_request(msg: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from(msg))
        .unwrap()
}

fn forbidden(msg: &'static str) -> Response {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Body::from(msg))
        .unwrap()
}

fn error(status: StatusCode, msg: &'static str) -> Response {
    Response::builder()
        .status(status)
        .body(Body::from(msg))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::extract_prop;

    #[test]
    fn parse_plain_displayname() {
        let body = r#"<mkcalendar><set><prop><displayname>Trabalho</displayname></prop></set></mkcalendar>"#;
        assert_eq!(extract_prop(body, "displayname").as_deref(), Some("Trabalho"));
    }

    #[test]
    fn parse_prefixed_displayname() {
        let body = r#"<C:mkcalendar xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
            <D:set><D:prop><D:displayname>Estudos</D:displayname></D:prop></D:set>
        </C:mkcalendar>"#;
        assert_eq!(extract_prop(body, "displayname").as_deref(), Some("Estudos"));
    }

    #[test]
    fn parse_color() {
        let body = r#"<ICAL:calendar-color xmlns:ICAL="http://apple.com/ns/ical/">#3498db</ICAL:calendar-color>"#;
        assert_eq!(extract_prop(body, "calendar-color").as_deref(), Some("#3498db"));
    }

    #[test]
    fn parse_missing_returns_none() {
        assert_eq!(extract_prop("<a/>", "displayname"), None);
    }
}

//! iMIP (RFC 6047) REPLY detection → forward to calendar scheduling inbox.
//!
//! Inbound path: LMTP delivers a message; `ingest::process` inspects the raw
//! MIME for a `text/calendar` part carrying `METHOD:REPLY`. If present, the
//! ICS payload is relayed (best-effort) to the calendar service's scheduling
//! inbox so attendee PARTSTAT updates are applied to the organizer's event.
//!
//! Why here (not in the milter): Postfix milter runs before delivery and
//! cannot authenticate against a specific recipient's tenant/user. At LMTP
//! ingest time we already resolved `(tenant_id, user_id)` for the organizer
//! (= recipient of the REPLY), which the calendar inbox endpoint requires.

use mail_parser::{MessageParser, MimeHeaders, PartType};
use uuid::Uuid;

/// Scan a MIME message for a `text/calendar` part containing `METHOD:REPLY`.
/// Returns the ICS body when found.
pub fn extract_imip_reply(raw: &[u8]) -> Option<String> {
    let msg = MessageParser::default().parse(raw)?;
    for part in msg.parts.iter() {
        let ct = match part.content_type() {
            Some(ct) => ct,
            None => continue,
        };
        let is_calendar = ct.c_type.eq_ignore_ascii_case("text")
            && ct
                .c_subtype
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("calendar"));
        if !is_calendar {
            continue;
        }
        let body = match &part.body {
            PartType::Text(cow) => cow.to_string(),
            PartType::Html(cow) => cow.to_string(),
            PartType::Binary(b) | PartType::InlineBinary(b) => {
                String::from_utf8_lossy(b).into_owned()
            }
            _ => continue,
        };
        if has_method_reply(&body) {
            return Some(body);
        }
    }
    None
}

/// Case-insensitive scan for a `METHOD:REPLY` property (RFC 5545 § 3.1:
/// property names are case-insensitive; values for METHOD are enum tokens).
fn has_method_reply(ics: &str) -> bool {
    ics.lines().any(|l| {
        let u = l.trim().to_ascii_uppercase();
        // Allow METHOD with optional params (METHOD;X-FOO=bar:REPLY).
        match u.split_once(':') {
            Some((name, value)) => {
                let name_base = name.split(';').next().unwrap_or("").trim();
                name_base == "METHOD" && value.trim() == "REPLY"
            }
            None => false,
        }
    })
}

/// POST the ICS to `{calendar_url}/api/v1/scheduling/inbox` as
/// `(tenant_id, user_id)`. Errors propagate; caller decides whether to log
/// and swallow (we want mail delivery to succeed regardless).
pub async fn forward_reply(
    calendar_url: &str,
    tenant_id: Uuid,
    user_id: Uuid,
    ics: &str,
) -> anyhow::Result<()> {
    let url = format!(
        "{}/api/v1/scheduling/inbox",
        calendar_url.trim_end_matches('/')
    );
    let resp = reqwest::Client::new()
        .post(&url)
        .header("x-tenant-id", tenant_id.to_string())
        .header("x-user-id", user_id.to_string())
        .header("content-type", "text/calendar")
        .body(ics.to_owned())
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("calendar inbox returned {status}: {body}");
    }
    tracing::info!(%status, %tenant_id, %user_id, "iMIP REPLY forwarded to calendar");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY_INLINE: &[u8] = b"From: a@b\r\nTo: c@d\r\nSubject: reply\r\n\
Content-Type: text/calendar; method=REPLY; charset=utf-8\r\n\
\r\n\
BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:abc\r\n\
ATTENDEE;PARTSTAT=ACCEPTED:mailto:a@b\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const REPLY_MULTIPART: &[u8] = b"From: a@b\r\nTo: c@d\r\nSubject: reply\r\n\
Content-Type: multipart/mixed; boundary=BB\r\n\
\r\n\
--BB\r\nContent-Type: text/plain\r\n\r\nhello\r\n\
--BB\r\nContent-Type: text/calendar; charset=utf-8; method=REPLY\r\n\r\n\
BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REPLY\r\nBEGIN:VEVENT\r\nUID:xyz\r\n\
ATTENDEE;PARTSTAT=DECLINED:mailto:a@b\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n\
--BB--\r\n";

    const REQUEST_ONLY: &[u8] = b"From: a@b\r\nTo: c@d\r\nSubject: request\r\n\
Content-Type: text/calendar; method=REQUEST; charset=utf-8\r\n\
\r\n\
BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const PLAIN: &[u8] = b"From: a@b\r\nTo: c@d\r\nSubject: plain\r\n\
Content-Type: text/plain\r\n\r\nhello world\r\n";

    #[test]
    fn detects_inline_reply() {
        let ics = extract_imip_reply(REPLY_INLINE).expect("should find");
        assert!(ics.contains("METHOD:REPLY"));
        assert!(ics.contains("UID:abc"));
    }

    #[test]
    fn detects_multipart_reply() {
        let ics = extract_imip_reply(REPLY_MULTIPART).expect("should find");
        assert!(ics.contains("METHOD:REPLY"));
        assert!(ics.contains("UID:xyz"));
    }

    #[test]
    fn ignores_request() {
        assert!(extract_imip_reply(REQUEST_ONLY).is_none());
    }

    #[test]
    fn ignores_plain() {
        assert!(extract_imip_reply(PLAIN).is_none());
    }

    #[test]
    fn method_matcher_case_insensitive() {
        assert!(has_method_reply("method:reply\r\n"));
        assert!(has_method_reply("METHOD:REPLY"));
        assert!(has_method_reply("BEGIN:VCALENDAR\r\nMETHOD: REPLY \r\n"));
        assert!(!has_method_reply("METHOD:REQUEST"));
        assert!(!has_method_reply("X-METHOD:REPLY"));
    }
}

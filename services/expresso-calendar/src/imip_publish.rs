//! iMIP envelope publisher — bridge entre calendar events armazenados
//! (`domain::event::Event`) e o consumer `expresso-imip-dispatch`.
//!
//! Subject: `expresso.imip.request` (stream EXPRESSO_CALENDAR).
//! Fire-and-forget: spawna task tokio; handler HTTP não bloqueia.
//! Skip silencioso quando evento não tem attendees ou dtstart/dtend.

use crate::domain::event::Event as StoredEvent;
use crate::domain::itip;
use async_nats::jetstream::Context as JsCtx;
use once_cell::sync::Lazy;
use prometheus::IntCounterVec;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;

static IMIP_PUBLISH_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "calendar_imip_publish_total",
            "iMIP envelope publish attempts per method and result",
        ),
        &["method", "result"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

pub fn init_metrics() {
    Lazy::force(&IMIP_PUBLISH_TOTAL);
    for m in ["REQUEST", "CANCEL"] {
        for r in ["ok", "err", "serialize_err", "skipped_no_attendees", "skipped_no_times"] {
            IMIP_PUBLISH_TOTAL.with_label_values(&[m, r]).inc_by(0);
        }
    }
}

#[derive(Serialize)]
struct AttendeeWire<'a> {
    email: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    common_name: Option<&'a str>,
    rsvp: bool,
}

#[derive(Serialize)]
struct InviteWire<'a> {
    uid: &'a str,
    sequence: u32,
    summary: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'a str>,
    dtstart: String,
    dtend: String,
    organizer_email: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    organizer_cn: Option<&'a str>,
    attendees: Vec<AttendeeWire<'a>>,
}

#[derive(Serialize)]
struct Envelope<'a> {
    method: &'static str,
    invite: InviteWire<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject_hint: Option<String>,
}

/// Build envelope JSON bytes from stored event + method.
/// Returns `Ok(None)` when the event lacks required fields (no attendees
/// or no dtstart/dtend) — caller should skip publish.
pub fn build_envelope_bytes(
    ev: &StoredEvent,
    method: &'static str,
) -> Result<Option<Vec<u8>>, serde_json::Error> {
    let attendees = itip::parse_attendees(&ev.ical_raw);
    if attendees.is_empty() {
        IMIP_PUBLISH_TOTAL.with_label_values(&[method, "skipped_no_attendees"]).inc();
        return Ok(None);
    }
    let (Some(dtstart), Some(dtend)) = (ev.dtstart, ev.dtend) else {
        IMIP_PUBLISH_TOTAL.with_label_values(&[method, "skipped_no_times"]).inc();
        return Ok(None);
    };
    let Some(organizer_email) = ev.organizer_email.as_deref() else {
        IMIP_PUBLISH_TOTAL.with_label_values(&[method, "skipped_no_times"]).inc();
        return Ok(None);
    };

    let summary = ev.summary.as_deref().unwrap_or("(sem título)");
    let subject_hint = Some(match method {
        "CANCEL" => format!("Cancelado: {summary}"),
        _        => format!("Convite: {summary}"),
    });

    let attendees_wire: Vec<AttendeeWire> = attendees.iter().map(|a| AttendeeWire {
        email: &a.email,
        common_name: a.cn.as_deref(),
        rsvp: a.rsvp.unwrap_or(true),
    }).collect();

    let invite = InviteWire {
        uid: &ev.uid,
        sequence: ev.sequence.max(0) as u32,
        summary,
        description: ev.description.as_deref(),
        location: ev.location.as_deref(),
        dtstart: dtstart.format(&Rfc3339).unwrap_or_default(),
        dtend:   dtend.format(&Rfc3339).unwrap_or_default(),
        organizer_email,
        organizer_cn: None,
        attendees: attendees_wire,
    };
    let envelope = Envelope { method, invite, subject_hint };
    Ok(Some(serde_json::to_vec(&envelope)?))
}

/// Fire-and-forget publish to `expresso.imip.request`. No-op when event
/// lacks required fields. Increments metrics for every outcome.
pub fn publish_imip(js: JsCtx, ev: StoredEvent, method: &'static str) {
    tokio::spawn(async move {
        let bytes = match build_envelope_bytes(&ev, method) {
            Ok(Some(b)) => b,
            Ok(None) => return,
            Err(e) => {
                IMIP_PUBLISH_TOTAL.with_label_values(&[method, "serialize_err"]).inc();
                tracing::warn!(error=%e, "imip envelope serialize failed");
                return;
            }
        };
        match js.publish("expresso.imip.request", bytes.into()).await {
            Ok(_) => {
                IMIP_PUBLISH_TOTAL.with_label_values(&[method, "ok"]).inc();
                tracing::debug!(method, uid=%ev.uid, "imip envelope published");
            }
            Err(e) => {
                IMIP_PUBLISH_TOTAL.with_label_values(&[method, "err"]).inc();
                tracing::warn!(error=%e, method, "imip publish failed");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;
    use uuid::Uuid;

    fn sample_event(ical: &str) -> StoredEvent {
        StoredEvent {
            id: Uuid::nil(),
            calendar_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            uid: "uid-1@x".into(),
            etag: "e".into(),
            ical_raw: ical.into(),
            summary: Some("Reunião".into()),
            description: Some("Alinhamento trimestral".into()),
            location: Some("Sala A".into()),
            dtstart: Some(datetime!(2026-05-10 13:00 UTC)),
            dtend:   Some(datetime!(2026-05-10 14:00 UTC)),
            rrule: None,
            status: None,
            sequence: 2,
            organizer_email: Some("alice@ex.local".into()),
            created_at: datetime!(2026-01-01 0:00 UTC),
            updated_at: datetime!(2026-01-01 0:00 UTC),
        }
    }

    const ICAL_WITH_ATTENDEES: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:uid-1@x\r\nSUMMARY:Reunião\r\nATTENDEE;CN=Bob;RSVP=TRUE:mailto:bob@ex.local\r\nATTENDEE;CN=\"Carol Doe\";RSVP=FALSE:mailto:carol@ex.local\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    const ICAL_NO_ATTENDEES: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:uid-2@x\r\nSUMMARY:Solo\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn envelope_contains_method_and_attendees() {
        let ev = sample_event(ICAL_WITH_ATTENDEES);
        let bytes = build_envelope_bytes(&ev, "REQUEST").unwrap().expect("should build");
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["method"], "REQUEST");
        assert_eq!(v["invite"]["uid"], "uid-1@x");
        assert_eq!(v["invite"]["sequence"], 2);
        assert_eq!(v["invite"]["organizer_email"], "alice@ex.local");
        assert_eq!(v["invite"]["attendees"].as_array().unwrap().len(), 2);
        assert_eq!(v["invite"]["attendees"][0]["email"], "bob@ex.local");
        assert_eq!(v["invite"]["attendees"][0]["common_name"], "Bob");
        assert_eq!(v["invite"]["attendees"][0]["rsvp"], true);
        assert_eq!(v["invite"]["attendees"][1]["rsvp"], false);
        assert!(v["subject_hint"].as_str().unwrap().starts_with("Convite:"));
    }

    #[test]
    fn cancel_subject_hint() {
        let ev = sample_event(ICAL_WITH_ATTENDEES);
        let bytes = build_envelope_bytes(&ev, "CANCEL").unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["method"], "CANCEL");
        assert!(v["subject_hint"].as_str().unwrap().starts_with("Cancelado:"));
    }

    #[test]
    fn skip_when_no_attendees() {
        let ev = sample_event(ICAL_NO_ATTENDEES);
        assert!(build_envelope_bytes(&ev, "REQUEST").unwrap().is_none());
    }

    #[test]
    fn skip_when_missing_times() {
        let mut ev = sample_event(ICAL_WITH_ATTENDEES);
        ev.dtstart = None;
        assert!(build_envelope_bytes(&ev, "REQUEST").unwrap().is_none());
    }

    #[test]
    fn rfc3339_datetimes() {
        let ev = sample_event(ICAL_WITH_ATTENDEES);
        let bytes = build_envelope_bytes(&ev, "REQUEST").unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["invite"]["dtstart"], "2026-05-10T13:00:00Z");
        assert_eq!(v["invite"]["dtend"],   "2026-05-10T14:00:00Z");
    }
}

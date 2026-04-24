//! expresso-imip-dispatch
//!
//! Consumer JetStream que recebe envelopes iMIP e envia e-mails aos
//! attendees usando `lettre` (SMTP) + `expresso-imip` (iCal + MIME).
//!
//! Subject consumido: `expresso.imip.request` (stream: `EXPRESSO_CALENDAR`
//! temporariamente; em #40+ pode migrar p/ stream próprio).
//!
//! Envelope JSON:
//! ```json
//! {
//!   "method": "REQUEST" | "CANCEL",
//!   "invite": { ... EventInvite serializado ... },
//!   "subject_hint": "Convite: Reunião X"
//! }
//! ```
//!
//! Env:
//!   NATS_URL        = nats://host:4222                    (required)
//!   NATS_DURABLE    = consumer name (default imip-dispatch)
//!   NATS_SUBJECT    = filter (default expresso.imip.request)
//!   NATS_STREAM     = stream name (default EXPRESSO_CALENDAR)
//!   IMIP_ENABLED    = true|false (default false = dry-run, logs only)
//!   SMTP_HOST       = smtp host                           (if enabled)
//!   SMTP_PORT       = smtp port (default 587)
//!   SMTP_USER       = smtp user (optional)
//!   SMTP_PASSWORD   = smtp password (optional)
//!   SMTP_FROM       = email remetente (default noreply@expresso.local)
//!   SMTP_STARTTLS   = true|false (default true)
//!   METRICS_ADDR    = bind ops http (default 0.0.0.0:9192)

use anyhow::{Context, Result};
use async_nats::jetstream::{
    self,
    consumer::{pull::Config as PullConfig, DeliverPolicy},
};
use axum::{response::IntoResponse, routing::get, Router};
use expresso_imip::{build_mime_multipart, Attendee, EventInvite, ImipError, Method};
use futures::StreamExt;
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, Encoder, IntCounterVec, TextEncoder};
use serde::{Deserialize, Serialize};
use std::env;
use time::OffsetDateTime;
use tracing::{error, info, warn};

static DISPATCH_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "imip_dispatch_total",
        "iMIP dispatch attempts by method and result.",
        &["method", "result"]
    )
    .expect("register")
});

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "UPPERCASE")]
enum EnvelopeMethod {
    Request,
    Cancel,
}

impl From<EnvelopeMethod> for Method {
    fn from(m: EnvelopeMethod) -> Self {
        match m {
            EnvelopeMethod::Request => Method::Request,
            EnvelopeMethod::Cancel => Method::Cancel,
        }
    }
}

#[derive(Debug, Deserialize)]
struct AttendeeWire {
    email: String,
    #[serde(default)]
    common_name: Option<String>,
    #[serde(default = "default_true")]
    rsvp: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, Deserialize)]
struct InviteWire {
    uid: String,
    #[serde(default)]
    sequence: u32,
    summary: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    dtstart: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    dtend: OffsetDateTime,
    organizer_email: String,
    #[serde(default)]
    organizer_cn: Option<String>,
    attendees: Vec<AttendeeWire>,
}

impl From<InviteWire> for EventInvite {
    fn from(w: InviteWire) -> Self {
        EventInvite {
            uid: w.uid,
            sequence: w.sequence,
            summary: w.summary,
            description: w.description,
            location: w.location,
            dtstart: w.dtstart,
            dtend: w.dtend,
            organizer_email: w.organizer_email,
            organizer_cn: w.organizer_cn,
            attendees: w.attendees.into_iter().map(|a| Attendee {
                email: a.email,
                common_name: a.common_name,
                rsvp: a.rsvp,
            }).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Envelope {
    method: EnvelopeMethod,
    invite: InviteWire,
    #[serde(default)]
    subject_hint: Option<String>,
}

#[derive(Clone)]
struct SmtpConfig {
    enabled: bool,
    host: String,
    port: u16,
    user: Option<String>,
    password: Option<String>,
    from: String,
    starttls: bool,
}

impl SmtpConfig {
    fn from_env() -> Self {
        let enabled = env::var("IMIP_ENABLED").map(|v| v == "true").unwrap_or(false);
        Self {
            enabled,
            host: env::var("SMTP_HOST").unwrap_or_default(),
            port: env::var("SMTP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(587),
            user: env::var("SMTP_USER").ok().filter(|s| !s.is_empty()),
            password: env::var("SMTP_PASSWORD").ok().filter(|s| !s.is_empty()),
            from: env::var("SMTP_FROM").unwrap_or_else(|_| "noreply@expresso.local".into()),
            starttls: env::var("SMTP_STARTTLS").map(|v| v != "false").unwrap_or(true),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let nats_url = env::var("NATS_URL").context("NATS_URL required")?;
    let durable = env::var("NATS_DURABLE").unwrap_or_else(|_| "imip-dispatch".into());
    let subject = env::var("NATS_SUBJECT").unwrap_or_else(|_| "expresso.imip.request".into());
    let stream_name = env::var("NATS_STREAM").unwrap_or_else(|_| "EXPRESSO_CALENDAR".into());
    let metrics_addr = env::var("METRICS_ADDR").unwrap_or_else(|_| "0.0.0.0:9192".into());

    let smtp = SmtpConfig::from_env();
    pre_populate_metrics();

    tokio::spawn(run_ops_http(metrics_addr.clone()));

    info!(
        %nats_url, %durable, %subject, %stream_name, %metrics_addr,
        imip_enabled = smtp.enabled,
        smtp_host = %smtp.host,
        "starting"
    );

    let client = async_nats::connect(&nats_url).await?;
    let js = jetstream::new(client);

    let stream = js.get_stream(&stream_name).await
        .with_context(|| format!("get stream {stream_name}"))?;

    let consumer = stream.get_or_create_consumer(&durable, PullConfig {
        durable_name: Some(durable.clone()),
        filter_subject: subject.clone(),
        deliver_policy: DeliverPolicy::New,
        ..Default::default()
    }).await.context("create consumer")?;

    info!("consumer ready, waiting for messages");

    let mut messages = consumer.messages().await?;
    while let Some(msg) = messages.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => { warn!(error=%e, "recv error"); continue; }
        };
        let smtp = smtp.clone();
        tokio::spawn(async move {
            match process_message(&msg.payload, &smtp).await {
                Ok((method, _)) => {
                    DISPATCH_TOTAL.with_label_values(&[method_label(method), "ok"]).inc();
                }
                Err(e) => {
                    warn!(error=%e, "dispatch failed");
                    DISPATCH_TOTAL.with_label_values(&["unknown", "err"]).inc();
                }
            }
            if let Err(e) = msg.ack().await {
                warn!(error=%e, "ack failed");
            }
        });
    }
    Ok(())
}

fn method_label(m: Method) -> &'static str {
    match m { Method::Request => "REQUEST", Method::Cancel => "CANCEL" }
}

fn pre_populate_metrics() {
    Lazy::force(&DISPATCH_TOTAL);
    for m in ["REQUEST", "CANCEL", "unknown"] {
        for r in ["ok", "err", "parse_err", "send_err", "dry_run"] {
            DISPATCH_TOTAL.with_label_values(&[m, r]).inc_by(0);
        }
    }
}

async fn process_message(payload: &[u8], smtp: &SmtpConfig) -> Result<(Method, usize)> {
    let env: Envelope = serde_json::from_slice(payload)
        .map_err(|e| {
            DISPATCH_TOTAL.with_label_values(&["unknown", "parse_err"]).inc();
            anyhow::anyhow!("parse envelope: {e}")
        })?;
    let method: Method = env.method.into();
    let invite: EventInvite = env.invite.into();
    let subject_hint = env.subject_hint.unwrap_or_else(|| match method {
        Method::Request => format!("Convite: {}", invite.summary),
        Method::Cancel => format!("Cancelado: {}", invite.summary),
    });

    let human = human_summary(&invite, method);
    let (ct, body) = build_mime_multipart(&invite, method, &human)
        .map_err(|e: ImipError| anyhow::anyhow!("mime build: {e}"))?;

    let recipients: Vec<String> = invite.attendees.iter().map(|a| a.email.clone()).collect();

    if !smtp.enabled {
        info!(
            uid = %invite.uid, method = method_label(method),
            attendees = recipients.len(), subject = %subject_hint,
            "dry-run (IMIP_ENABLED=false)"
        );
        DISPATCH_TOTAL.with_label_values(&[method_label(method), "dry_run"]).inc();
        return Ok((method, recipients.len()));
    }

    // Actual SMTP send
    send_via_smtp(smtp, &invite.organizer_email, &subject_hint, &ct, body, &recipients).await
        .map_err(|e| {
            DISPATCH_TOTAL.with_label_values(&[method_label(method), "send_err"]).inc();
            e
        })?;
    info!(uid=%invite.uid, method=method_label(method), attendees=recipients.len(), "sent");
    Ok((method, recipients.len()))
}

async fn send_via_smtp(
    cfg: &SmtpConfig,
    _organizer: &str,
    subject: &str,
    content_type: &str,
    body: String,
    recipients: &[String],
) -> Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    if recipients.is_empty() {
        return Err(anyhow::anyhow!("no recipients"));
    }
    let from_mbox: lettre::message::Mailbox = cfg.from.parse()
        .map_err(|e| anyhow::anyhow!("parse SMTP_FROM: {e}"))?;

    let mut builder = Message::builder()
        .from(from_mbox)
        .subject(subject)
        .header(ContentType::parse(content_type)
            .map_err(|e| anyhow::anyhow!("content_type: {e}"))?);
    for r in recipients {
        let mbox: lettre::message::Mailbox = r.parse()
            .map_err(|e| anyhow::anyhow!("parse recipient {r}: {e}"))?;
        builder = builder.to(mbox);
    }
    let email = builder.body(body)
        .map_err(|e| anyhow::anyhow!("build email: {e}"))?;

    let mut transport = if cfg.starttls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host)
            .map_err(|e| anyhow::anyhow!("smtp starttls: {e}"))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.host)
    }
    .port(cfg.port);
    if let (Some(u), Some(p)) = (&cfg.user, &cfg.password) {
        transport = transport.credentials(Credentials::new(u.clone(), p.clone()));
    }
    transport.build().send(email).await
        .map_err(|e| anyhow::anyhow!("smtp send: {e}"))?;
    Ok(())
}

fn human_summary(invite: &EventInvite, method: Method) -> String {
    let verb = match method {
        Method::Request => "Você foi convidado(a) para",
        Method::Cancel => "Evento cancelado:",
    };
    format!(
        "{verb} \"{}\"\n\nQuando: {}\nLocal: {}\nOrganizador: {}\n\n\
         Anexo: invite.ics (aceite/recuse pelo seu cliente de calendário).",
        invite.summary,
        invite.dtstart,
        invite.location.as_deref().unwrap_or("—"),
        invite.organizer_email,
    )
}

async fn run_ops_http(addr: String) {
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(|| async { "ok" }))
        .route("/metrics", get(metrics_handler));
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => { error!(error=%e, %addr, "ops http bind failed"); return; }
    };
    info!(%addr, "ops http ready");
    if let Err(e) = axum::serve(listener, app).await {
        error!(error=%e, "ops http exited");
    }
}

async fn metrics_handler() -> impl IntoResponse {
    let mut buf = Vec::new();
    let enc = TextEncoder::new();
    if let Err(e) = enc.encode(&prometheus::gather(), &mut buf) {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, format!("encode: {e}")).into_response();
    }
    ([(axum::http::header::CONTENT_TYPE, enc.format_type())], buf).into_response()
}

// --- tests ---------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_envelope() {
        let j = r#"{
            "method": "REQUEST",
            "invite": {
                "uid": "e1@host",
                "sequence": 0,
                "summary": "Sync",
                "description": null,
                "location": "Sala A",
                "dtstart": "2026-05-10T13:00:00Z",
                "dtend":   "2026-05-10T13:30:00Z",
                "organizer_email": "alice@x",
                "organizer_cn": "Alice",
                "attendees": [{"email":"bob@x","common_name":"Bob","rsvp":true}]
            },
            "subject_hint": "Convite: Sync"
        }"#;
        let env: Envelope = serde_json::from_str(j).unwrap();
        let invite: EventInvite = env.invite.into();
        assert_eq!(invite.uid, "e1@host");
        assert_eq!(invite.attendees.len(), 1);
    }

    #[test]
    fn parse_cancel_with_defaults() {
        let j = r#"{
            "method": "CANCEL",
            "invite": {
                "uid": "e2@host",
                "summary": "Reunião",
                "dtstart": "2026-05-10T13:00:00Z",
                "dtend":   "2026-05-10T14:00:00Z",
                "organizer_email": "a@x",
                "attendees": [{"email":"b@x"}]
            }
        }"#;
        let env: Envelope = serde_json::from_str(j).unwrap();
        let invite: EventInvite = env.invite.into();
        assert_eq!(invite.sequence, 0);
        assert!(invite.organizer_cn.is_none());
        assert!(invite.attendees[0].rsvp); // default true
    }

    #[test]
    fn envelope_method_serializes_upper() {
        let m: EnvelopeMethod = serde_json::from_str("\"REQUEST\"").unwrap();
        assert!(matches!(m, EnvelopeMethod::Request));
    }

    #[test]
    fn human_summary_varies_by_method() {
        let inv = EventInvite {
            uid: "u".into(), sequence: 0, summary: "Foo".into(),
            description: None, location: Some("L".into()),
            dtstart: time::macros::datetime!(2026-05-10 13:00 UTC),
            dtend:   time::macros::datetime!(2026-05-10 14:00 UTC),
            organizer_email: "o@x".into(), organizer_cn: None,
            attendees: vec![Attendee{email:"a@x".into(),common_name:None,rsvp:true}],
        };
        let r = human_summary(&inv, Method::Request);
        let c = human_summary(&inv, Method::Cancel);
        assert!(r.contains("convidado"));
        assert!(c.contains("cancelado"));
    }
}

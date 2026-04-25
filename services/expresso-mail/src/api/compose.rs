//! Send email endpoint.
//!
//! From-spoof guard: os endpoints aceitavam `from` arbitrário do cliente e
//! submetiam direto ao relay SMTP — qualquer usuário autenticado podia
//! enviar mail como qualquer outro (inclusive cross-tenant). Agora
//! `assert_from_is_authenticated_user` verifica que `req.from` bate com o
//! email do usuário autenticado (case-insensitive) antes de enviar.

use axum::{Router, routing::post, extract::State, Json, http::StatusCode};
use expresso_core::begin_tenant_tx;
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
    message::{header::ContentType, Mailbox, Message, MultiPart, SinglePart},
    Address,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{api::context::RequestCtx, error::{MailError, Result}, state::AppState};

/// Limites duros pro endpoint de envio.
///
/// Sem isso, qualquer usuário autenticado vira spam-cannon: enviar pra
/// 100k recipients num shot, body de 50 MiB, etc. — usando o relay SMTP
/// do tenant e custando reputação de IP/domínio.
///
/// 100 recipients combina to+cc+bcc; cobre listas internas legítimas
/// sem virar BCC-bomb. 1 MiB de body cobre HTML rich + assinatura;
/// anexos grandes vão por outro fluxo. 998 bytes de subject = limite
/// de linha do RFC 5322.
pub const MAX_RECIPIENTS_PER_MESSAGE: usize = 100;
pub const MAX_BODY_BYTES:             usize = 1024 * 1024;
pub const MAX_SUBJECT_BYTES:          usize = 998;

/// VCALENDAR payload cap pro send_itip. Convites reais (com participantes,
/// VALARM, recurrence rules) ficam em poucos KiB; 256 KiB cobre até
/// agendas insanas. Acima disso é abuso — ICS gigante engasga MUAs e
/// vira amplificador via mailing-list de calendário.
pub const MAX_ICS_BYTES: usize = 256 * 1024;

/// Rejeita com 403 se `claimed_from` não bater com o email do usuário
/// autenticado (case-insensitive, trim). A consulta usa `begin_tenant_tx`
/// + `WHERE tenant_id = $1 AND id = $2` — defense-in-depth contra RLS
/// NULL-bypass em `users`.
async fn assert_from_is_authenticated_user(
    state:       &AppState,
    ctx:         &RequestCtx,
    claimed_from: &str,
) -> Result<()> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row: Option<String> = sqlx::query_scalar(
        r#"SELECT email FROM users
            WHERE tenant_id = $1 AND id = $2
            LIMIT 1"#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;

    let actual = row.ok_or(MailError::Forbidden)?;
    if actual.trim().eq_ignore_ascii_case(claimed_from.trim()) {
        Ok(())
    } else {
        Err(MailError::Forbidden)
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/send",      post(send_message))
        .route("/mail/send-itip", post(send_itip))
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SendRequest {
    pub from:        String,
    pub to:          Vec<String>,
    pub cc:          Option<Vec<String>>,
    pub bcc:         Option<Vec<String>>,
    pub subject:     String,
    pub body_text:   Option<String>,
    pub body_html:   Option<String>,
    pub reply_to_id: Option<Uuid>,
}

/// POST /api/v1/mail/send
pub async fn send_message(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(req):    Json<SendRequest>,
) -> Result<StatusCode> {
    validate_send_request(&req)?;
    assert_from_is_authenticated_user(&state, &ctx, &req.from).await?;

    let from_addr: Address = req.from.parse()
        .map_err(|_| MailError::InvalidMessage(format!("invalid from: {}", req.from)))?;

    let mut builder = Message::builder()
        .from(Mailbox::new(None, from_addr))
        .subject(&req.subject);

    for addr_str in &req.to {
        let a: Address = addr_str.parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid to: {addr_str}")))?;
        builder = builder.to(Mailbox::new(None, a));
    }

    // Build body — prefer multipart when both variants present
    let email = match (req.body_html.as_deref(), req.body_text.as_deref()) {
        (Some(html), Some(plain)) => builder.multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(plain.to_string()))
                .singlepart(SinglePart::builder().header(ContentType::TEXT_HTML).body(html.to_string())),
        ),
        (Some(html), None) => builder.singlepart(
            SinglePart::builder().header(ContentType::TEXT_HTML).body(html.to_string()),
        ),
        (None, plain_opt) => builder.singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(plain_opt.unwrap_or("").to_string()),
        ),
    }
    .map_err(|e| MailError::InvalidMessage(e.to_string()))?;

    let smtp_host = &state.cfg().mail_server.relay_host;
    let smtp_port = state.cfg().mail_server.relay_port;

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_host)
        .port(smtp_port)
        .build();

    // DKIM signing: se configurado, serializa → assina → prepend header → envia raw.
    // Senão, fluxo normal via lettre send() (≠ signed).
    if let Some(signer) = state.dkim() {
        let envelope = email.envelope().clone();
        let raw = email.formatted();
        match signer.sign(&raw) {
            Ok(sig_header) => {
                let mut signed = Vec::with_capacity(sig_header.len() + raw.len());
                signed.extend_from_slice(sig_header.as_bytes());
                signed.extend_from_slice(&raw);
                mailer.send_raw(&envelope, &signed).await
                    .map_err(|e| MailError::SendFailed(e.to_string()))?;
                tracing::info!(from = %req.from, to = ?req.to, subject = %req.subject, dkim = true, "message sent");
                return Ok(StatusCode::ACCEPTED);
            }
            Err(e) => {
                // Falha DKIM ≠ bloqueia envio — loga e manda sem assinar.
                tracing::warn!(error = %e, "DKIM sign failed — sending unsigned");
            }
        }
    }

    mailer.send(email).await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;

    tracing::info!(from = %req.from, to = ?req.to, subject = %req.subject, dkim = false, "message sent");
    Ok(StatusCode::ACCEPTED)
}


// ─────────────────────────────────────────────────────────────────────────────
// iTIP delivery (RFC 6047): wrap an ICS body as a text/calendar MIME part with
// METHOD parameter and ship through the same SMTP relay used by /mail/send.
// Calendar service (or any client) produces the ICS — this endpoint only
// handles the MIME wrapping + relay hand-off.

use lettre::message::Attachment;

#[derive(Debug, Deserialize)]
pub struct SendItipRequest {
    pub from:     String,
    pub to:       Vec<String>,
    pub subject:  String,
    /// iTIP method: REQUEST, REPLY, CANCEL, REFRESH.
    pub method:   String,
    /// Plain-text fallback body; ICS-only clients still render from the ics part.
    pub body_text: Option<String>,
    /// Raw VCALENDAR payload (CRLF-terminated per RFC 5545).
    pub ics:      String,
}

pub async fn send_itip(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(req):    Json<SendItipRequest>,
) -> Result<StatusCode> {
    validate_itip_request(&req)?;
    assert_from_is_authenticated_user(&state, &ctx, &req.from).await?;

    let method = req.method.trim().to_ascii_uppercase();
    match method.as_str() {
        "REQUEST" | "REPLY" | "CANCEL" | "REFRESH" => {}
        _ => return Err(MailError::InvalidMessage(format!("unsupported iTIP method: {method}"))),
    }

    let from_addr: Address = req.from.parse()
        .map_err(|_| MailError::InvalidMessage(format!("invalid from: {}", req.from)))?;

    let mut builder = Message::builder()
        .from(Mailbox::new(None, from_addr))
        .subject(&req.subject);

    for addr_str in &req.to {
        let a: Address = addr_str.parse()
            .map_err(|_| MailError::InvalidMessage(format!("invalid to: {addr_str}")))?;
        builder = builder.to(Mailbox::new(None, a));
    }

    // RFC 6047 §2.1: text/calendar with method=<METHOD> parameter.
    let calendar_ct: ContentType = format!("text/calendar; method={method}; charset=utf-8")
        .parse()
        .map_err(|e: lettre::message::header::ContentTypeErr| MailError::InvalidMessage(e.to_string()))?;

    // Build multipart/alternative: plain text + text/calendar. Use attachment
    // form so that MUAs that don't render inline still offer the ics as a file.
    let plain = req.body_text.unwrap_or_else(|| format!(
        "This is an iTIP {method} invitation. Your mail client should display it inline."
    ));

    let ics_part = Attachment::new("invite.ics".to_string())
        .body(req.ics.into_bytes(), calendar_ct);

    let email = builder
        .multipart(
            MultiPart::mixed()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(plain),
                )
                .singlepart(ics_part),
        )
        .map_err(|e| MailError::InvalidMessage(e.to_string()))?;

    let smtp_host = &state.cfg().mail_server.relay_host;
    let smtp_port = state.cfg().mail_server.relay_port;

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_host)
        .port(smtp_port)
        .build();

    mailer.send(email).await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;

    tracing::info!(from=%req.from, to=?req.to, method=%method, "itip dispatched");
    Ok(StatusCode::ACCEPTED)
}

/// Gate aplicado em /mail/send antes do bind ao relay SMTP. Ordem dos
/// checks: subject → recipients → body. Mantém o de baixo custo (len)
/// antes do que poderia alocar (Vec scan).
fn validate_send_request(req: &SendRequest) -> Result<()> {
    if req.subject.len() > MAX_SUBJECT_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "subject too large: {} bytes (max {})",
            req.subject.len(), MAX_SUBJECT_BYTES
        )));
    }
    if req.subject.contains('\r') || req.subject.contains('\n') {
        return Err(MailError::InvalidMessage(
            "subject must not contain CR or LF".into()
        ));
    }
    let recipient_count = req.to.len()
        + req.cc.as_ref().map_or(0, Vec::len)
        + req.bcc.as_ref().map_or(0, Vec::len);
    if recipient_count == 0 {
        return Err(MailError::InvalidMessage("no recipients".into()));
    }
    if recipient_count > MAX_RECIPIENTS_PER_MESSAGE {
        return Err(MailError::InvalidMessage(format!(
            "too many recipients: {} (max {})",
            recipient_count, MAX_RECIPIENTS_PER_MESSAGE
        )));
    }
    let body_total = req.body_text.as_deref().map_or(0, str::len)
        + req.body_html.as_deref().map_or(0, str::len);
    if body_total > MAX_BODY_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "body too large: {} bytes (max {})",
            body_total, MAX_BODY_BYTES
        )));
    }
    Ok(())
}

/// Gate aplicado em /mail/send-itip antes do bind ao relay. Reusa os caps
/// de subject/recipients do /mail/send + cap próprio pro VCALENDAR.
fn validate_itip_request(req: &SendItipRequest) -> Result<()> {
    if req.subject.len() > MAX_SUBJECT_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "subject too large: {} bytes (max {})",
            req.subject.len(), MAX_SUBJECT_BYTES
        )));
    }
    if req.subject.contains('\r') || req.subject.contains('\n') {
        return Err(MailError::InvalidMessage(
            "subject must not contain CR or LF".into()
        ));
    }
    if req.to.is_empty() {
        return Err(MailError::InvalidMessage("no recipients".into()));
    }
    if req.to.len() > MAX_RECIPIENTS_PER_MESSAGE {
        return Err(MailError::InvalidMessage(format!(
            "too many recipients: {} (max {})",
            req.to.len(), MAX_RECIPIENTS_PER_MESSAGE
        )));
    }
    if req.ics.len() > MAX_ICS_BYTES {
        return Err(MailError::InvalidMessage(format!(
            "ics payload too large: {} bytes (max {})",
            req.ics.len(), MAX_ICS_BYTES
        )));
    }
    if let Some(b) = req.body_text.as_deref() {
        if b.len() > MAX_BODY_BYTES {
            return Err(MailError::InvalidMessage(format!(
                "body too large: {} bytes (max {})",
                b.len(), MAX_BODY_BYTES
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> SendRequest {
        SendRequest {
            from:        "alice@acme.test".into(),
            to:          vec!["bob@acme.test".into()],
            cc:          None,
            bcc:         None,
            subject:     "hi".into(),
            body_text:   Some("hello".into()),
            body_html:   None,
            reply_to_id: None,
        }
    }

    #[test]
    fn ok_default() {
        assert!(validate_send_request(&req()).is_ok());
    }

    #[test]
    fn rejects_empty_recipients() {
        let mut r = req();
        r.to.clear();
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("no recipients"), "got: {err}");
    }

    #[test]
    fn rejects_excess_recipients() {
        let mut r = req();
        r.to  = vec!["x@y.z".to_string(); 60];
        r.cc  = Some(vec!["x@y.z".to_string(); 30]);
        r.bcc = Some(vec!["x@y.z".to_string(); 20]); // total 110 > 100
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("too many recipients"), "got: {err}");
    }

    #[test]
    fn accepts_max_recipients_exact() {
        let mut r = req();
        r.to  = vec!["x@y.z".to_string(); 50];
        r.cc  = Some(vec!["x@y.z".to_string(); 30]);
        r.bcc = Some(vec!["x@y.z".to_string(); 20]); // total 100
        assert!(validate_send_request(&r).is_ok());
    }

    #[test]
    fn rejects_oversize_subject() {
        let mut r = req();
        r.subject = "x".repeat(MAX_SUBJECT_BYTES + 1);
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("subject too large"), "got: {err}");
    }

    #[test]
    fn rejects_crlf_in_subject() {
        let mut r = req();
        r.subject = "Hi\r\nBcc: evil@x.y".into();
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("CR or LF"), "got: {err}");
    }

    #[test]
    fn rejects_oversize_body_combined() {
        // Sum de text+html. Cada um sozinho passaria, juntos não.
        let mut r = req();
        let half = "x".repeat(MAX_BODY_BYTES * 2 / 3);
        r.body_text = Some(half.clone());
        r.body_html = Some(half);
        let err = format!("{:?}", validate_send_request(&r).unwrap_err());
        assert!(err.contains("body too large"), "got: {err}");
    }

    fn itip_req() -> SendItipRequest {
        SendItipRequest {
            from:      "alice@acme.test".into(),
            to:        vec!["bob@acme.test".into()],
            subject:   "Meeting".into(),
            method:    "REQUEST".into(),
            body_text: None,
            ics:       "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n".into(),
        }
    }

    #[test]
    fn itip_ok_default() {
        assert!(validate_itip_request(&itip_req()).is_ok());
    }

    #[test]
    fn itip_rejects_empty_recipients() {
        let mut r = itip_req();
        r.to.clear();
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("no recipients"), "got: {err}");
    }

    #[test]
    fn itip_rejects_excess_recipients() {
        let mut r = itip_req();
        r.to = vec!["x@y.z".to_string(); MAX_RECIPIENTS_PER_MESSAGE + 1];
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("too many recipients"), "got: {err}");
    }

    #[test]
    fn itip_rejects_oversize_ics() {
        let mut r = itip_req();
        r.ics = "x".repeat(MAX_ICS_BYTES + 1);
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("ics payload too large"), "got: {err}");
    }

    #[test]
    fn itip_rejects_crlf_in_subject() {
        let mut r = itip_req();
        r.subject = "Meet\r\nBcc: evil@x.y".into();
        let err = format!("{:?}", validate_itip_request(&r).unwrap_err());
        assert!(err.contains("CR or LF"), "got: {err}");
    }

    #[test]
    fn itip_accepts_boundary_ics() {
        let mut r = itip_req();
        r.ics = "x".repeat(MAX_ICS_BYTES);
        assert!(validate_itip_request(&r).is_ok());
    }
}

//! Send email endpoint

use axum::{Router, routing::post, extract::State, Json, http::StatusCode};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
    message::{header::ContentType, Mailbox, Message, MultiPart, SinglePart},
    Address,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::{error::{MailError, Result}, state::AppState};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/send", post(send_message))
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
    Json(req): Json<SendRequest>,
) -> Result<StatusCode> {
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

    let smtp_host = &state.cfg().mail_server.domain;
    let smtp_port = state.cfg().mail_server.smtp_port;

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(smtp_host)
        .port(smtp_port)
        .build();

    mailer.send(email).await
        .map_err(|e| MailError::SendFailed(e.to_string()))?;

    tracing::info!(from = %req.from, to = ?req.to, subject = %req.subject, "message sent");
    Ok(StatusCode::ACCEPTED)
}

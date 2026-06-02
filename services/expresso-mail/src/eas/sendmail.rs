//! EAS SendMail command (MS-ASCMD §2.2.2.17).
//!
//! Unlike the WBXML commands, SendMail's body is a complete raw RFC822 MIME
//! message (the client built it). We parse the envelope (From + all
//! recipients) from the MIME, DKIM-sign, relay through the same SMTP submission
//! relay the REST send path uses, and save a copy to the Sent folder. On success
//! EAS expects an empty HTTP 200 (no response body).

use lettre::{address::Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use mail_parser::MessageParser;
use uuid::Uuid;

use crate::state::AppState;

/// Outcome of a SendMail attempt; the handler maps this to an HTTP status.
pub enum SendOutcome {
    Sent,
    /// Could not parse a usable envelope from the MIME (bad request).
    InvalidMessage,
    /// Relay failed (upstream error).
    RelayFailed,
}

/// Relay a raw MIME message on behalf of `user_id`/`tenant_id` and copy it to
/// Sent. Best-effort save (a failed copy doesn't fail the send, mirroring REST).
pub async fn send_raw_mime(
    state: &AppState,
    user_id: Uuid,
    tenant_id: Uuid,
    raw: &[u8],
) -> SendOutcome {
    let Some((from, rcpts)) = parse_envelope(raw) else {
        return SendOutcome::InvalidMessage;
    };
    let envelope = match lettre::address::Envelope::new(Some(from), rcpts) {
        Ok(e) => e,
        Err(_) => return SendOutcome::InvalidMessage,
    };

    // DKIM-sign if configured (non-fatal on failure, same as REST send).
    let to_relay = if let Some(signer) = state.dkim() {
        match signer.sign(raw) {
            Ok(sig) => {
                let mut signed = Vec::with_capacity(sig.len() + raw.len());
                signed.extend_from_slice(sig.as_bytes());
                signed.extend_from_slice(raw);
                signed
            }
            Err(e) => {
                tracing::warn!(error = %e, "EAS SendMail DKIM sign failed — sending unsigned");
                raw.to_vec()
            }
        }
    } else {
        raw.to_vec()
    };

    let mailer = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
        &state.cfg().mail_server.relay_host,
    )
    .port(state.cfg().mail_server.relay_port)
    .build();

    if let Err(e) = mailer.send_raw(&envelope, &to_relay).await {
        tracing::warn!(error = %e, "EAS SendMail relay failed");
        return SendOutcome::RelayFailed;
    }

    if let Err(e) =
        crate::api::compose::save_raw_to_folder(state, user_id, tenant_id, &to_relay, r"\Sent")
            .await
    {
        tracing::warn!(error = %e, "EAS SendMail save-to-sent failed");
    }
    SendOutcome::Sent
}

/// Parse the SMTP envelope (From address + recipient addresses from To/Cc/Bcc)
/// out of a raw MIME message. Returns `None` when there is no parseable From or
/// no recipients.
fn parse_envelope(raw: &[u8]) -> Option<(Address, Vec<Address>)> {
    let msg = MessageParser::default().parse(raw)?;
    let from = msg
        .from()
        .and_then(|a| a.first())
        .and_then(|addr| addr.address())
        .and_then(|s| s.parse::<Address>().ok())?;

    let mut rcpts = Vec::new();
    for group in [msg.to(), msg.cc(), msg.bcc()].into_iter().flatten() {
        for addr in group.iter() {
            if let Some(parsed) = addr.address().and_then(|s| s.parse::<Address>().ok()) {
                if !rcpts.contains(&parsed) {
                    rcpts.push(parsed);
                }
            }
        }
    }
    if rcpts.is_empty() {
        return None;
    }
    Some((from, rcpts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_envelope_extracts_from_and_recipients() {
        let raw = b"From: alice@x.com\r\nTo: bob@y.com, carol@z.com\r\nSubject: hi\r\n\r\nbody";
        let (from, rcpts) = parse_envelope(raw).unwrap();
        assert_eq!(from.to_string(), "alice@x.com");
        assert_eq!(rcpts.len(), 2);
        assert!(rcpts.iter().any(|r| r.to_string() == "bob@y.com"));
        assert!(rcpts.iter().any(|r| r.to_string() == "carol@z.com"));
    }

    #[test]
    fn parse_envelope_includes_cc_and_bcc() {
        let raw =
            b"From: a@x.com\r\nTo: b@y.com\r\nCc: c@y.com\r\nBcc: d@y.com\r\nSubject: s\r\n\r\nx";
        let (_, rcpts) = parse_envelope(raw).unwrap();
        assert_eq!(rcpts.len(), 3);
    }

    #[test]
    fn parse_envelope_dedups_recipients() {
        let raw = b"From: a@x.com\r\nTo: b@y.com\r\nCc: b@y.com\r\nSubject: s\r\n\r\nx";
        let (_, rcpts) = parse_envelope(raw).unwrap();
        assert_eq!(rcpts.len(), 1);
    }

    #[test]
    fn parse_envelope_none_without_recipients() {
        let raw = b"From: a@x.com\r\nSubject: s\r\n\r\nbody";
        assert!(parse_envelope(raw).is_none());
    }

    #[test]
    fn parse_envelope_none_without_from() {
        let raw = b"To: b@y.com\r\nSubject: s\r\n\r\nbody";
        assert!(parse_envelope(raw).is_none());
    }
}

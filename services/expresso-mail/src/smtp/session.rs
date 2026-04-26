//! SMTP session state machine — one per connection.
//! Implements: EHLO, MAIL FROM, RCPT TO, DATA, RSET, QUIT, NOOP, VRFY.
//! Inbound: runs SPF/DKIM/DMARC verification on DATA, prepends Authentication-Results.

use std::net::SocketAddr;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tracing::{debug, error, info, instrument, warn};

use crate::{dkim, ingest, state::AppState};
use crate::smtp::metrics::{command_label, SMTP_COMMANDS_TOTAL, SMTP_SESSIONS_TOTAL};

const MAX_MSG_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const MAX_RCPTS: usize = 100;

#[derive(Default)]
struct Envelope {
    from: Option<String>,
    rcpts: Vec<String>,
    helo: Option<String>,
    /// Client-declared message size via MAIL FROM SIZE= parameter (RFC 1870).
    declared_size: Option<usize>,
}

/// Handle a single SMTP connection.
#[instrument(skip(stream, state), fields(peer = %peer))]
pub async fn handle(stream: TcpStream, peer: SocketAddr, state: AppState) -> anyhow::Result<()> {
    SMTP_SESSIONS_TOTAL.with_label_values(&["smtp25", "accepted"]).inc();

    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let domain = &state.cfg().mail_server.domain;

    writer
        .write_all(format!("220 {domain} ESMTP Expresso\r\n").as_bytes())
        .await?;

    let mut env = Envelope::default();
    let mut data_mode = false;
    let mut data_buf = String::new();

    let result = async {
    loop {
        let line = match lines.next_line().await? {
            Some(l) => l,
            None => break,
        };

        debug!(line = %line, "smtp ←");

        if data_mode {
            if line == "." {
                data_mode = false;
                let raw = data_buf.as_bytes();
                let bytes = raw.len();
                info!(from = ?env.from, rcpts = ?env.rcpts, bytes, "received message");

                let helo = env.helo.as_deref().unwrap_or("unknown");
                let mail_from = env.from.as_deref().unwrap_or("");
                let auth = dkim::verify_inbound(peer.ip(), helo, mail_from, domain, raw).await;
                debug!(spf = %auth.spf, dkim = %auth.dkim, dmarc = %auth.dmarc,
                       policy = ?auth.dmarc_policy, "auth results");

                if auth.should_reject() {
                    warn!(from = %mail_from, spf = %auth.spf, dkim = %auth.dkim,
                          "DMARC fail + p=reject — refusing message");
                    expresso_mail_auth::MAIL_AUTH_ACTIONS_TOTAL
                        .with_label_values(&["reject"]).inc();
                    writer
                        .write_all(b"550 5.7.1 DMARC policy: message rejected (p=reject)\r\n")
                        .await?;
                    SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "smtp25", "reject"]).inc();
                    data_buf.clear();
                    env = Envelope::default();
                    continue;
                }
                expresso_mail_auth::MAIL_AUTH_ACTIONS_TOTAL
                    .with_label_values(&["accept"]).inc();

                let rspf_header = auth.to_received_spf_header(domain);
                let auth_header = auth.to_header(domain);
                let signed_raw = format!("{rspf_header}{auth_header}{data_buf}");

                match ingest::process(&state, env.from.as_deref(), &env.rcpts, signed_raw.as_bytes()).await {
                    Ok(delivered) if delivered > 0 => {
                        writer.write_all(b"250 OK message accepted\r\n").await?;
                        SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "smtp25", "ok"]).inc();
                    }
                    Ok(_) => {
                        writer.write_all(b"550 No valid recipients\r\n").await?;
                        SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "smtp25", "reject"]).inc();
                    }
                    Err(e) => {
                        error!(error = %e, "ingest failed");
                        writer.write_all(b"451 Requested action aborted: local error\r\n").await?;
                        SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "smtp25", "error"]).inc();
                    }
                }
                data_buf.clear();
                env = Envelope::default();
            } else {
                // RFC 5321 §4.5.2 dot-stuffing: strip leading dot.
                let line = line.strip_prefix('.').unwrap_or(&line);
                if data_buf.len() + line.len() + 2 > MAX_MSG_BYTES {
                    writer.write_all(b"552 Message too large\r\n").await?;
                    data_mode = false;
                    data_buf.clear();
                    env = Envelope::default();
                } else {
                    data_buf.push_str(line);
                    data_buf.push_str("\r\n");
                }
            }
            continue;
        }

        let upper = line.to_ascii_uppercase();

        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            let verb = if upper.starts_with("EHLO") { "EHLO" } else { "HELO" };
            env.helo = Some(line[4..].trim().to_string());
            writer.write_all(
                format!("250-{domain} Hello\r\n250-SIZE {MAX_MSG_BYTES}\r\n250-8BITMIME\r\n250-PIPELINING\r\n250 OK\r\n")
                    .as_bytes()
            ).await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&[verb, "smtp25", "ok"]).inc();
        } else if upper.starts_with("MAIL FROM:") {
            let rest = &line[10..];
            let declared = extract_size_param(rest);
            if let Some(sz) = declared {
                if sz > MAX_MSG_BYTES {
                    writer.write_all(b"552 5.3.4 Message size exceeds fixed maximum message size\r\n").await?;
                    SMTP_COMMANDS_TOTAL.with_label_values(&["MAIL", "smtp25", "reject"]).inc();
                    continue;
                }
            }
            env.from = Some(extract_angle(rest));
            env.declared_size = declared;
            env.rcpts.clear();
            writer.write_all(b"250 OK\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["MAIL", "smtp25", "ok"]).inc();
        } else if upper.starts_with("RCPT TO:") {
            if env.from.is_none() {
                writer.write_all(b"503 Bad sequence: MAIL first\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["RCPT", "smtp25", "reject"]).inc();
            } else if env.rcpts.len() >= MAX_RCPTS {
                writer.write_all(b"452 Too many recipients\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["RCPT", "smtp25", "reject"]).inc();
            } else {
                env.rcpts.push(extract_angle(&line[8..]));
                writer.write_all(b"250 OK\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["RCPT", "smtp25", "ok"]).inc();
            }
        } else if upper == "DATA" {
            if env.from.is_none() || env.rcpts.is_empty() {
                writer.write_all(b"503 Bad sequence\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "smtp25", "reject"]).inc();
            } else {
                writer.write_all(b"354 Start input; end with <CRLF>.<CRLF>\r\n").await?;
                data_mode = true;
            }
        } else if upper == "RSET" {
            env = Envelope::default();
            writer.write_all(b"250 OK\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["RSET", "smtp25", "ok"]).inc();
        } else if upper == "NOOP" {
            writer.write_all(b"250 OK\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["NOOP", "smtp25", "ok"]).inc();
        } else if upper == "VRFY" {
            // RFC 5321 §3.5.2: servers MAY return 252 (not verified but will accept).
            writer.write_all(b"252 2.5.2 Cannot VRFY user; try RCPT\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["VRFY", "smtp25", "ok"]).inc();
        } else if upper == "QUIT" {
            writer.write_all(format!("221 {domain} Bye\r\n").as_bytes()).await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["QUIT", "smtp25", "ok"]).inc();
            break;
        } else {
            let verb = command_label(upper.split_whitespace().next().unwrap_or("OTHER"));
            warn!(cmd = %line, "unknown SMTP command");
            writer.write_all(b"500 Command not recognized\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&[verb, "smtp25", "reject"]).inc();
        }
    }
    anyhow::Ok(())
    }.await;

    match &result {
        Ok(()) => SMTP_SESSIONS_TOTAL.with_label_values(&["smtp25", "closed"]).inc(),
        Err(_) => SMTP_SESSIONS_TOTAL.with_label_values(&["smtp25", "error"]).inc(),
    }
    result
}

/// Extract address from `<user@domain>` or `user@domain`
fn extract_angle(s: &str) -> String {
    // Strip any ESMTP parameters after the address (e.g., "SIZE=1234").
    let addr_part = s.trim();
    let addr_part = if let Some(gt) = addr_part.find('>') {
        &addr_part[..=gt]
    } else {
        // No angle bracket — take up to first whitespace (ESMTP param).
        addr_part.split_whitespace().next().unwrap_or(addr_part)
    };
    let addr_part = addr_part.trim();
    if addr_part.starts_with('<') && addr_part.ends_with('>') {
        addr_part[1..addr_part.len() - 1].to_string()
    } else {
        addr_part.to_string()
    }
}

/// Extract the SIZE parameter value from a MAIL FROM parameter string.
/// Accepts `<addr@host> SIZE=12345` or `addr@host SIZE=12345`.
/// Returns None if SIZE is absent or unparseable.
fn extract_size_param(s: &str) -> Option<usize> {
    // Upper-case to find SIZE= case-insensitively.
    let upper = s.to_ascii_uppercase();
    let pos = upper.find("SIZE=")?;
    let after = s[pos + 5..].split_whitespace().next()?;
    after.parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::{extract_angle, extract_size_param};

    #[test]
    fn angle_plain() {
        assert_eq!(extract_angle("<a@b.com>"), "a@b.com");
        assert_eq!(extract_angle("a@b.com"), "a@b.com");
    }

    #[test]
    fn angle_with_size_param() {
        assert_eq!(extract_angle("<a@b.com> SIZE=1234"), "a@b.com");
        assert_eq!(extract_angle("a@b.com SIZE=1234"), "a@b.com");
    }

    #[test]
    fn size_param_extracted() {
        assert_eq!(extract_size_param("<a@b.com> SIZE=99"), Some(99));
        assert_eq!(extract_size_param("a@b SIZE=0"), Some(0));
        assert_eq!(extract_size_param("<a@b.com>"), None);
        assert_eq!(extract_size_param("SIZE=abc"), None);
    }
}

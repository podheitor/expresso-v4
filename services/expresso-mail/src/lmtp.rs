//! LMTP server (RFC 2033) — local delivery protocol.
//!
//! Differences from SMTP:
//!   * Banner + greeting = "LMTP", LHLO instead of EHLO
//!   * After final `.` in DATA → ONE response line per accepted recipient
//!     (not a single 250 for the whole message)
//!
//! Purpose: Postfix on :25/:465/:587 writes to queue, then delivers to this
//! listener on :24 via `virtual_transport = lmtp:inet:expresso-mail:24`.
//! Because Postfix already authenticated the peer and (via milter) verified
//! SPF/DKIM/DMARC + injected `Authentication-Results`, this endpoint trusts
//! the content and ingests directly.

use std::net::SocketAddr;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tracing::{debug, error, info, instrument, warn};

use crate::{ingest, state::AppState};
use crate::smtp::metrics::{command_label, SMTP_COMMANDS_TOTAL, SMTP_SESSIONS_TOTAL};

const MAX_MSG_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const MAX_RCPTS: usize = 100;

#[derive(Default)]
struct Envelope {
    from: Option<String>,
    rcpts: Vec<String>,
    lhlo: Option<String>,
    declared_size: Option<usize>,
}

/// Start LMTP listener; spawn task per connection.
pub async fn serve(state: AppState, addr: SocketAddr) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(addr = %addr, "LMTP listener ready");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, peer, state).await {
                        error!(peer = %peer, error = %e, "LMTP session error");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "LMTP accept error");
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }
    }
}

#[instrument(skip(stream, state), fields(peer = %peer))]
async fn handle(stream: TcpStream, peer: SocketAddr, state: AppState) -> anyhow::Result<()> {
    SMTP_SESSIONS_TOTAL.with_label_values(&["lmtp", "accepted"]).inc();

    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let domain = &state.cfg().mail_server.domain;

    writer
        .write_all(format!("220 {domain} LMTP Expresso\r\n").as_bytes())
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
        debug!(line = %line, "lmtp ←");

        if data_mode {
            if line == "." {
                data_mode = false;
                let bytes = data_buf.len();
                info!(from = ?env.from, rcpts = ?env.rcpts, bytes, "LMTP received");

                let rcpts = std::mem::take(&mut env.rcpts);
                match ingest::process(&state, env.from.as_deref(), &rcpts, data_buf.as_bytes()).await {
                    Ok(_) => {
                        for rcpt in &rcpts {
                            writer
                                .write_all(format!("250 2.0.0 <{rcpt}> delivered\r\n").as_bytes())
                                .await?;
                        }
                        SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "lmtp", "ok"]).inc();
                    }
                    Err(e) => {
                        error!(error = %e, "LMTP ingest failed");
                        for rcpt in &rcpts {
                            writer
                                .write_all(format!("451 4.0.0 <{rcpt}> local error\r\n").as_bytes())
                                .await?;
                        }
                        SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "lmtp", "error"]).inc();
                    }
                }
                data_buf.clear();
                env = Envelope::default();
            } else {
                // RFC 5321 §4.5.2 dot-stuffing: strip leading dot.
                let line = line.strip_prefix('.').unwrap_or(&line);
                if data_buf.len() + line.len() + 2 > MAX_MSG_BYTES {
                    writer.write_all(b"552 5.3.4 Message too large\r\n").await?;
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

        if upper.starts_with("LHLO") {
            env.lhlo = Some(line[4..].trim().to_string());
            writer
                .write_all(
                    format!("250-{domain} Hello\r\n250-SIZE {MAX_MSG_BYTES}\r\n250-8BITMIME\r\n250-ENHANCEDSTATUSCODES\r\n250 PIPELINING\r\n").as_bytes(),
                )
                .await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["LHLO", "lmtp", "ok"]).inc();
        } else if upper.starts_with("MAIL FROM:") {
            let rest = &line[10..];
            let declared = extract_size_param(rest);
            if let Some(sz) = declared {
                if sz > MAX_MSG_BYTES {
                    writer.write_all(b"552 5.3.4 Message size exceeds fixed maximum\r\n").await?;
                    SMTP_COMMANDS_TOTAL.with_label_values(&["MAIL", "lmtp", "reject"]).inc();
                    continue;
                }
            }
            env.from = Some(extract_angle(rest));
            env.declared_size = declared;
            env.rcpts.clear();
            writer.write_all(b"250 2.1.0 Sender OK\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["MAIL", "lmtp", "ok"]).inc();
        } else if upper.starts_with("RCPT TO:") {
            if env.from.is_none() {
                writer.write_all(b"503 5.5.1 MAIL first\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["RCPT", "lmtp", "reject"]).inc();
            } else if env.rcpts.len() >= MAX_RCPTS {
                writer.write_all(b"452 4.5.3 Too many recipients\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["RCPT", "lmtp", "reject"]).inc();
            } else {
                env.rcpts.push(extract_angle(&line[8..]));
                writer.write_all(b"250 2.1.5 Recipient OK\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["RCPT", "lmtp", "ok"]).inc();
            }
        } else if upper == "DATA" {
            if env.from.is_none() || env.rcpts.is_empty() {
                writer.write_all(b"503 5.5.1 Bad sequence\r\n").await?;
                SMTP_COMMANDS_TOTAL.with_label_values(&["DATA", "lmtp", "reject"]).inc();
            } else {
                writer.write_all(b"354 End data with <CRLF>.<CRLF>\r\n").await?;
                data_mode = true;
            }
        } else if upper == "RSET" {
            env = Envelope::default();
            writer.write_all(b"250 2.0.0 Reset\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["RSET", "lmtp", "ok"]).inc();
        } else if upper == "NOOP" {
            writer.write_all(b"250 2.0.0 OK\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["NOOP", "lmtp", "ok"]).inc();
        } else if upper == "QUIT" {
            writer.write_all(format!("221 2.0.0 {domain} Bye\r\n").as_bytes()).await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&["QUIT", "lmtp", "ok"]).inc();
            break;
        } else {
            let verb = command_label(upper.split_whitespace().next().unwrap_or("OTHER"));
            warn!(cmd = %line, "unknown LMTP command");
            writer.write_all(b"500 5.5.2 Command not recognized\r\n").await?;
            SMTP_COMMANDS_TOTAL.with_label_values(&[verb, "lmtp", "reject"]).inc();
        }
    }
    anyhow::Ok(())
    }.await;

    match &result {
        Ok(()) => SMTP_SESSIONS_TOTAL.with_label_values(&["lmtp", "closed"]).inc(),
        Err(_) => SMTP_SESSIONS_TOTAL.with_label_values(&["lmtp", "error"]).inc(),
    }
    result
}

fn extract_angle(s: &str) -> String {
    let addr_part = s.trim();
    let addr_part = if let Some(gt) = addr_part.find('>') {
        &addr_part[..=gt]
    } else {
        addr_part.split_whitespace().next().unwrap_or(addr_part)
    };
    let addr_part = addr_part.trim();
    if addr_part.starts_with('<') && addr_part.ends_with('>') {
        addr_part[1..addr_part.len() - 1].to_string()
    } else {
        addr_part.to_string()
    }
}

fn extract_size_param(s: &str) -> Option<usize> {
    let upper = s.to_ascii_uppercase();
    let pos = upper.find("SIZE=")?;
    let after = s[pos + 5..].split_whitespace().next()?;
    after.parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::{extract_angle, extract_size_param};

    #[test]
    fn angle_brackets() {
        assert_eq!(extract_angle(" <a@b> "), "a@b");
        assert_eq!(extract_angle("a@b"), "a@b");
        assert_eq!(extract_angle("<a@b>"), "a@b");
        assert_eq!(extract_angle("<a@b> SIZE=1234"), "a@b");
    }

    #[test]
    fn size_param_parsed() {
        assert_eq!(extract_size_param("<a@b> SIZE=512"), Some(512));
        assert_eq!(extract_size_param("<a@b>"), None);
        assert_eq!(extract_size_param("SIZE=abc"), None);
    }

    #[test]
    fn angle_empty_input() {
        assert_eq!(extract_angle(""), "");
    }

    #[test]
    fn angle_no_brackets_with_params() {
        assert_eq!(extract_angle("user@example.com BODY=8BITMIME"), "user@example.com");
    }

    #[test]
    fn size_param_case_insensitive() {
        assert_eq!(extract_size_param("<a@b> size=1024"), Some(1024));
    }

    #[test]
    fn size_param_zero() {
        assert_eq!(extract_size_param("SIZE=0"), Some(0));
    }

    #[test]
    fn angle_nested_brackets_outer_stripped() {
        assert_eq!(extract_angle("<<a@b>>"), "<a@b>");
    }

    #[test]
    fn size_param_large_value() {
        assert_eq!(extract_size_param("SIZE=52428800"), Some(52_428_800));
    }

    #[test]
    fn size_param_missing_returns_none() {
        assert_eq!(extract_size_param("<a@b>"), None);
    }
}

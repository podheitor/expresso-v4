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
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
use tracing::{debug, error, info, instrument, warn};

use crate::smtp::metrics::{command_label, SMTP_COMMANDS_TOTAL, SMTP_SESSIONS_TOTAL};
use crate::{ingest, state::AppState};

const MAX_MSG_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const MAX_RCPTS: usize = 100;

#[derive(Default)]
struct Envelope {
    from: Option<String>,
    rcpts: Vec<String>,
    lhlo: Option<String>,
    declared_size: Option<usize>,
}

/// Handle the LMTP `DATA` command: require a prior MAIL FROM + ≥1 RCPT, then
/// write `354` to begin message input. Returns `true` if the session should
/// enter DATA mode. Extracted from `handle`.
async fn handle_lmtp_data_command<W>(writer: &mut W, env: &Envelope) -> anyhow::Result<bool>
where
    W: AsyncWrite + Unpin,
{
    if env.from.is_none() || env.rcpts.is_empty() {
        writer.write_all(b"503 5.5.1 Bad sequence\r\n").await?;
        SMTP_COMMANDS_TOTAL
            .with_label_values(&["DATA", "lmtp", "reject"])
            .inc();
        return Ok(false);
    }
    writer
        .write_all(b"354 End data with <CRLF>.<CRLF>\r\n")
        .await?;
    Ok(true)
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
    SMTP_SESSIONS_TOTAL
        .with_label_values(&["lmtp", "accepted"])
        .inc();

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
            data_mode = handle_lmtp_data_command(&mut writer, &env).await?;
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
        Ok(()) => SMTP_SESSIONS_TOTAL
            .with_label_values(&["lmtp", "closed"])
            .inc(),
        Err(_) => SMTP_SESSIONS_TOTAL
            .with_label_values(&["lmtp", "error"])
            .inc(),
    }
    result
}

fn extract_angle(s: &str) -> String {
    let s = s.trim();
    // Isolate the address token: everything up to the first whitespace, so any
    // ESMTP params after the closing '>' (e.g. "<a@b> SIZE=1234") are dropped.
    let token = s.split_whitespace().next().unwrap_or(s);
    if token.starts_with('<') {
        // Strip exactly one outer <...> pair, matching the LAST '>' in the token
        // so a nested address like "<<a@b>>" yields "<a@b>" rather than "<a@b".
        if let Some(close) = token.rfind('>') {
            return token[1..close].to_string();
        }
    }
    token.to_string()
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
        assert_eq!(
            extract_angle("user@example.com BODY=8BITMIME"),
            "user@example.com"
        );
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

    #[test]
    fn size_param_lowercase_also_parsed() {
        assert_eq!(extract_size_param("<a@b> size=1024"), Some(1024));
    }

    #[test]
    fn size_param_zero_is_parsed() {
        assert_eq!(extract_size_param("<a@b> SIZE=0"), Some(0));
    }

    #[test]
    fn size_param_with_extra_params_extracted() {
        assert_eq!(extract_size_param("<a@b> BODY=7BIT SIZE=4096"), Some(4096));
    }

    #[test]
    fn size_param_non_numeric_returns_none() {
        assert_eq!(extract_size_param("<a@b> SIZE=notanumber"), None);
    }

    #[test]
    fn size_param_no_size_keyword_returns_none() {
        assert_eq!(extract_size_param("<a@b>"), None);
    }

    #[test]
    fn angle_with_uppercase_domain_extracted() {
        assert_eq!(extract_angle("<user@DOMAIN.COM>"), "user@DOMAIN.COM");
    }

    #[test]
    fn max_msg_bytes_is_50_mib() {
        assert_eq!(super::MAX_MSG_BYTES, 50 * 1024 * 1024);
    }

    #[test]
    fn size_param_decimal_not_parsed() {
        // Decimal size values must not parse — only integers are valid.
        assert_eq!(extract_size_param("<a@b> SIZE=10.5"), None);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_msg_bytes_exceeds_one_mib() {
        assert!(super::MAX_MSG_BYTES > 1024 * 1024);
    }

    #[test]
    fn angle_plain_without_params() {
        assert_eq!(extract_angle("<sender@domain.org>"), "sender@domain.org");
    }

    #[test]
    fn size_param_with_other_params_still_parsed() {
        assert_eq!(
            extract_size_param("<a@b> BODY=7BIT SIZE=4096 RET=HDRS"),
            Some(4096)
        );
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_rcpts_is_positive() {
        assert!(super::MAX_RCPTS > 0);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_msg_bytes_exceeds_max_rcpts() {
        assert!(super::MAX_MSG_BYTES > super::MAX_RCPTS);
    }

    #[test]
    fn size_param_equals_zero_parses_as_zero() {
        assert_eq!(extract_size_param("<user@example.com> SIZE=0"), Some(0));
    }
}

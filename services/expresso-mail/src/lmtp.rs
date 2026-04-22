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

const MAX_MSG_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const MAX_RCPTS: usize = 100;

#[derive(Default)]
struct Envelope {
    from: Option<String>,
    rcpts: Vec<String>,
    lhlo: Option<String>,
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
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let domain = &state.cfg().mail_server.domain;

    // LMTP banner
    writer
        .write_all(format!("220 {domain} LMTP Expresso\r\n").as_bytes())
        .await?;

    let mut env = Envelope::default();
    let mut data_mode = false;
    let mut data_buf = String::new();

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

                // Delivery: one response per rcpt
                let rcpts = std::mem::take(&mut env.rcpts);
                match ingest::process(&state, env.from.as_deref(), &rcpts, data_buf.as_bytes()).await {
                    Ok(_) => {
                        // Per RFC 2033: one line per RCPT reply
                        for rcpt in &rcpts {
                            writer
                                .write_all(format!("250 2.0.0 <{rcpt}> delivered\r\n").as_bytes())
                                .await?;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "LMTP ingest failed");
                        for rcpt in &rcpts {
                            writer
                                .write_all(format!("451 4.0.0 <{rcpt}> local error\r\n").as_bytes())
                                .await?;
                        }
                    }
                }
                data_buf.clear();
                env = Envelope::default();
            } else {
                // Dot-stuffing
                let line = line.strip_prefix('.').unwrap_or(&line);
                if data_buf.len() + line.len() > MAX_MSG_BYTES {
                    writer.write_all(b"552 5.3.4 Message too large\r\n").await?;
                    data_mode = false;
                    data_buf.clear();
                    env = Envelope::default();
                } else {
                    data_buf.push_str(line);
                    data_buf.push('\n');
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
        } else if upper.starts_with("MAIL FROM:") {
            env.from = Some(extract_angle(&line[10..]));
            env.rcpts.clear();
            writer.write_all(b"250 2.1.0 Sender OK\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            if env.from.is_none() {
                writer.write_all(b"503 5.5.1 MAIL first\r\n").await?;
            } else if env.rcpts.len() >= MAX_RCPTS {
                writer.write_all(b"452 4.5.3 Too many recipients\r\n").await?;
            } else {
                env.rcpts.push(extract_angle(&line[8..]));
                writer.write_all(b"250 2.1.5 Recipient OK\r\n").await?;
            }
        } else if upper == "DATA" {
            if env.from.is_none() || env.rcpts.is_empty() {
                writer.write_all(b"503 5.5.1 Bad sequence\r\n").await?;
            } else {
                writer
                    .write_all(b"354 End data with <CRLF>.<CRLF>\r\n")
                    .await?;
                data_mode = true;
            }
        } else if upper == "RSET" {
            env = Envelope::default();
            writer.write_all(b"250 2.0.0 Reset\r\n").await?;
        } else if upper == "NOOP" {
            writer.write_all(b"250 2.0.0 OK\r\n").await?;
        } else if upper == "QUIT" {
            writer
                .write_all(format!("221 2.0.0 {domain} Bye\r\n").as_bytes())
                .await?;
            break;
        } else {
            warn!(cmd = %line, "unknown LMTP command");
            writer
                .write_all(b"500 5.5.2 Command not recognized\r\n")
                .await?;
        }
    }
    Ok(())
}

fn extract_angle(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('<') && s.ends_with('>') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::extract_angle;

    #[test]
    fn angle_brackets() {
        assert_eq!(extract_angle(" <a@b> "), "a@b");
        assert_eq!(extract_angle("a@b"), "a@b");
        assert_eq!(extract_angle("<a@b>"), "a@b");
    }
}

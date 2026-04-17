//! SMTP session state machine — one per connection.
//! Implements: EHLO, MAIL FROM, RCPT TO, DATA, RSET, QUIT, NOOP.

use std::net::SocketAddr;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
};
use tracing::{debug, info, instrument, warn};

use crate::state::AppState;

const MAX_MSG_BYTES: usize = 50 * 1024 * 1024; // 50 MiB
const MAX_RCPTS:      usize = 100;

#[derive(Default)]
struct Envelope {
    from:  Option<String>,
    rcpts: Vec<String>,
}

/// Handle a single SMTP connection.
#[instrument(skip(stream, state), fields(peer = %peer))]
pub async fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    state: AppState,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    let domain = &state.cfg().mail_server.domain;

    // Greeting
    writer.write_all(format!("220 {domain} ESMTP Expresso\r\n").as_bytes()).await?;

    let mut env = Envelope::default();
    let mut data_mode = false;
    let mut data_buf  = String::new();

    loop {
        let line = match lines.next_line().await? {
            Some(l) => l,
            None    => break,   // client disconnected
        };

        debug!(line = %line, "smtp ←");

        if data_mode {
            // RFC 5321 §4.5.2: single dot on its own line = end of data
            if line == "." {
                data_mode = false;
                let bytes = data_buf.len();
                info!(from = ?env.from, rcpts = ?env.rcpts, bytes, "received message");

                // TODO: call ingest pipeline (parse + store + index)
                // ingest::process(&state, &env.from, &env.rcpts, data_buf.as_bytes()).await?;

                writer.write_all(b"250 OK message accepted\r\n").await?;
                data_buf.clear();
                env = Envelope::default();
            } else {
                // RFC 5321 §4.5.2: strip leading dot-stuffing
                let line = line.strip_prefix('.').unwrap_or(&line);
                if data_buf.len() + line.len() > MAX_MSG_BYTES {
                    writer.write_all(b"552 Message too large\r\n").await?;
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

        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            writer.write_all(
                format!("250-{domain} Hello\r\n250-SIZE {MAX_MSG_BYTES}\r\n250-8BITMIME\r\n250 OK\r\n")
                    .as_bytes()
            ).await?;
        } else if upper.starts_with("MAIL FROM:") {
            env.from = Some(extract_angle(&line[10..]));
            env.rcpts.clear();
            writer.write_all(b"250 OK\r\n").await?;
        } else if upper.starts_with("RCPT TO:") {
            if env.from.is_none() {
                writer.write_all(b"503 Bad sequence: MAIL first\r\n").await?;
            } else if env.rcpts.len() >= MAX_RCPTS {
                writer.write_all(b"452 Too many recipients\r\n").await?;
            } else {
                env.rcpts.push(extract_angle(&line[8..]));
                writer.write_all(b"250 OK\r\n").await?;
            }
        } else if upper == "DATA" {
            if env.from.is_none() || env.rcpts.is_empty() {
                writer.write_all(b"503 Bad sequence\r\n").await?;
            } else {
                writer.write_all(b"354 Start input; end with <CRLF>.<CRLF>\r\n").await?;
                data_mode = true;
            }
        } else if upper == "RSET" {
            env = Envelope::default();
            writer.write_all(b"250 OK\r\n").await?;
        } else if upper == "NOOP" {
            writer.write_all(b"250 OK\r\n").await?;
        } else if upper == "QUIT" {
            writer.write_all(format!("221 {domain} Bye\r\n").as_bytes()).await?;
            break;
        } else {
            warn!(cmd = %line, "unknown SMTP command");
            writer.write_all(b"500 Command not recognized\r\n").await?;
        }
    }

    Ok(())
}

/// Extract address from `<user@domain>` or `user@domain`
fn extract_angle(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('<') && s.ends_with('>') {
        s[1..s.len()-1].to_string()
    } else {
        s.to_string()
    }
}

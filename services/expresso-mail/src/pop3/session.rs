//! POP3 session state machine (RFC 1939). A session moves through three
//! states: AUTHORIZATION (USER/PASS), TRANSACTION (STAT/LIST/RETR/DELE/…),
//! and UPDATE (QUIT commits deletions). The maildrop is a snapshot of INBOX
//! taken at authentication time; scan numbers are 1-based positions into it
//! and stay stable for the session even as messages are marked deleted.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio_rustls::server::TlsStream;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::imap::lockout::LoginLockout;
use crate::pop3::command::{parse, Pop3Command};
use crate::pop3::metrics::{POP3_COMMANDS_TOTAL, POP3_LOGINS_TOTAL};
use crate::pop3::store::{self, Pop3Message};
use crate::state::AppState;
use once_cell::sync::Lazy;

/// Per-username brute-force throttle, shared with the same profile as IMAP
/// (10 failures / 60s → 5min lockout). Reuses the IMAP lockout type.
static LOGIN_LOCKOUT: Lazy<LoginLockout> = Lazy::new(LoginLockout::default);

/// Idle timeout — RFC 1939 §3 mandates at least a 10-minute autologout timer.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Maximum command-line length accepted (RFC 1939 §3: 255 octets for the
/// keyword+args). We allow a little slack for the CRLF.
const MAX_LINE: usize = 512;

/// A maildrop entry: the underlying message plus its per-session deletion mark.
struct Slot {
    msg: Pop3Message,
    deleted: bool,
}

/// Authenticated session state carried through TRANSACTION.
struct Maildrop {
    user_id: Uuid,
    tenant_id: Uuid,
    slots: Vec<Slot>,
}

impl Maildrop {
    /// Total octets of non-deleted messages (STAT/scan).
    fn total_size(&self) -> i64 {
        self.slots
            .iter()
            .filter(|s| !s.deleted)
            .map(|s| s.msg.size)
            .sum()
    }

    /// Count of non-deleted messages.
    fn count(&self) -> usize {
        self.slots.iter().filter(|s| !s.deleted).count()
    }

    /// Resolve a 1-based scan number to a live (non-deleted) slot index.
    fn live(&self, num: u32) -> Option<usize> {
        let idx = (num as usize).checked_sub(1)?;
        match self.slots.get(idx) {
            Some(s) if !s.deleted => Some(idx),
            _ => None,
        }
    }
}

pub async fn handle(stream: TcpStream, state: AppState) -> anyhow::Result<()> {
    let (r, w) = tokio::io::split(stream);
    run(BufReader::new(r), w, state).await
}

pub async fn handle_tls(stream: TlsStream<TcpStream>, state: AppState) -> anyhow::Result<()> {
    let (r, w) = tokio::io::split(stream);
    run(BufReader::new(r), w, state).await
}

/// Drive the session over already-split read/write halves. Generic so the same
/// code serves plain TCP and implicit-TLS connections.
async fn run<R, W>(mut reader: BufReader<R>, mut writer: W, state: AppState) -> anyhow::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let domain = state.cfg().mail_server.domain.clone();
    writer
        .write_all(format!("+OK {domain} POP3 Expresso ready\r\n").as_bytes())
        .await?;

    let mut auth_user: Option<String> = None;
    let mut drop: Option<Maildrop> = None;
    let mut line = String::new();

    loop {
        line.clear();
        let read = tokio::time::timeout(IDLE_TIMEOUT, reader.read_line(&mut line)).await;
        let n = match read {
            Err(_) => {
                let _ = writer
                    .write_all(b"-ERR autologout; idle too long\r\n")
                    .await;
                break;
            }
            Ok(Ok(0)) => break, // client closed
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(e.into()),
        };
        if n > MAX_LINE {
            writer.write_all(b"-ERR line too long\r\n").await?;
            continue;
        }
        debug!(line = %line.trim_end(), "pop3 ←");
        let cmd = parse(&line);

        if matches!(cmd, Pop3Command::Quit) {
            let removed = commit_quit(&state, &drop).await;
            let _ = writer
                .write_all(format!("+OK {removed} messages deleted; bye\r\n").as_bytes())
                .await;
            count_cmd("QUIT", true);
            break;
        }

        let replies = match drop.as_mut() {
            None => dispatch_auth(&state, cmd, &mut auth_user, &mut drop).await,
            Some(md) => dispatch_txn(&state, cmd, md).await,
        };
        for chunk in replies {
            writer.write_all(&chunk).await?;
        }
    }
    Ok(())
}

/// AUTHORIZATION-state dispatch: only USER/PASS/CAPA/NOOP/RSET are meaningful;
/// everything else is rejected until the client authenticates.
async fn dispatch_auth(
    state: &AppState,
    cmd: Pop3Command,
    auth_user: &mut Option<String>,
    drop: &mut Option<Maildrop>,
) -> Vec<Vec<u8>> {
    match cmd {
        Pop3Command::User(u) => {
            *auth_user = Some(u);
            count_cmd("USER", true);
            ok("send PASS")
        }
        Pop3Command::Pass(pass) => login(state, auth_user.take(), &pass, drop).await,
        Pop3Command::Capa => capa(),
        Pop3Command::Noop => {
            count_cmd("NOOP", true);
            ok("")
        }
        Pop3Command::Rset => {
            count_cmd("RSET", true);
            ok("")
        }
        Pop3Command::Quit => ok("bye"), // handled in run(); defensive
        other => {
            count_cmd(other.name(), false);
            err("command invalid in AUTHORIZATION state")
        }
    }
}

/// Run USER+PASS against the backend, applying the brute-force lockout. On
/// success, load the INBOX snapshot into a `Maildrop` and enter TRANSACTION.
async fn login(
    state: &AppState,
    user: Option<String>,
    pass: &str,
    drop: &mut Option<Maildrop>,
) -> Vec<Vec<u8>> {
    let Some(user) = user else {
        count_cmd("PASS", false);
        return err("USER required before PASS");
    };
    if LOGIN_LOCKOUT.is_locked_out(&user) {
        POP3_LOGINS_TOTAL.with_label_values(&["locked_out"]).inc();
        count_cmd("PASS", false);
        warn!(user = %user, "POP3 login refused (locked out)");
        return err("authentication failed");
    }
    match store::verify_login(state, &user, pass).await {
        Some((user_id, tenant_id)) => {
            LOGIN_LOCKOUT.clear_failures(&user);
            let slots = match store::load_inbox(state, user_id, tenant_id).await {
                Ok(msgs) => msgs
                    .into_iter()
                    .map(|msg| Slot {
                        msg,
                        deleted: false,
                    })
                    .collect(),
                Err(e) => {
                    warn!(user = %user, error = %e, "POP3 INBOX load failed");
                    return err("maildrop unavailable");
                }
            };
            *drop = Some(Maildrop {
                user_id,
                tenant_id,
                slots,
            });
            POP3_LOGINS_TOTAL.with_label_values(&["success"]).inc();
            count_cmd("PASS", true);
            ok("mailbox ready")
        }
        None => {
            LOGIN_LOCKOUT.record_failure(&user);
            POP3_LOGINS_TOTAL.with_label_values(&["failure"]).inc();
            count_cmd("PASS", false);
            warn!(user = %user, "POP3 login failed");
            err("authentication failed")
        }
    }
}

/// TRANSACTION-state dispatch.
async fn dispatch_txn(state: &AppState, cmd: Pop3Command, md: &mut Maildrop) -> Vec<Vec<u8>> {
    match cmd {
        Pop3Command::Stat => {
            count_cmd("STAT", true);
            ok(&format!("{} {}", md.count(), md.total_size()))
        }
        Pop3Command::List(arg) => list(md, arg, false),
        Pop3Command::Uidl(arg) => list(md, arg, true),
        Pop3Command::Retr(num) => retr(state, md, num, None).await,
        Pop3Command::Top(num, lines) => retr(state, md, num, Some(lines)).await,
        Pop3Command::Dele(num) => dele(md, num),
        Pop3Command::Noop => {
            count_cmd("NOOP", true);
            ok("")
        }
        Pop3Command::Rset => {
            for s in &mut md.slots {
                s.deleted = false;
            }
            count_cmd("RSET", true);
            ok("maildrop reset")
        }
        Pop3Command::Capa => capa(),
        Pop3Command::User(_) | Pop3Command::Pass(_) => err("already authenticated"),
        Pop3Command::Invalid(kw) => {
            count_cmd(kw, false);
            err("invalid arguments")
        }
        other @ (Pop3Command::Quit | Pop3Command::Unknown) => {
            count_cmd(other.name(), false);
            err("unknown command")
        }
    }
}

/// LIST/UIDL: either a single scan-line for one message, or a multi-line
/// listing of every live message. `uidl` selects the UUID column vs. octets.
fn list(md: &Maildrop, arg: Option<u32>, uidl: bool) -> Vec<Vec<u8>> {
    let label = if uidl { "UIDL" } else { "LIST" };
    if let Some(num) = arg {
        let Some(idx) = md.live(num) else {
            count_cmd(label, false);
            return err("no such message");
        };
        count_cmd(label, true);
        let val = scan_value(&md.slots[idx], uidl);
        return ok(&format!("{num} {val}"));
    }
    count_cmd(label, true);
    let mut out = format!("+OK {} messages\r\n", md.count()).into_bytes();
    for (i, slot) in md.slots.iter().enumerate() {
        if slot.deleted {
            continue;
        }
        out.extend_from_slice(format!("{} {}\r\n", i + 1, scan_value(slot, uidl)).as_bytes());
    }
    out.extend_from_slice(b".\r\n");
    vec![out]
}

fn scan_value(slot: &Slot, uidl: bool) -> String {
    if uidl {
        slot.msg.id.simple().to_string()
    } else {
        slot.msg.size.to_string()
    }
}

/// RETR (whole message) or TOP (headers + first `top_lines` body lines).
/// Output is byte-stuffed (leading-dot doubled) and dot-terminated per §3.
async fn retr(state: &AppState, md: &Maildrop, num: u32, top_lines: Option<u32>) -> Vec<Vec<u8>> {
    let label = if top_lines.is_some() { "TOP" } else { "RETR" };
    let Some(idx) = md.live(num) else {
        count_cmd(label, false);
        return err("no such message");
    };
    let Some(path) = md.slots[idx].msg.body_path.as_deref() else {
        count_cmd(label, false);
        return err("message body unavailable");
    };
    let Some(raw) = store::fetch_body(state, path).await else {
        count_cmd(label, false);
        return err("message body unavailable");
    };
    let payload = match top_lines {
        Some(n) => top_bytes(&raw, n as usize),
        None => raw,
    };
    count_cmd(label, true);
    let mut out = if top_lines.is_some() {
        b"+OK top of message follows\r\n".to_vec()
    } else {
        format!("+OK {} octets\r\n", payload.len()).into_bytes()
    };
    out.extend_from_slice(&dot_stuff(&payload));
    out.extend_from_slice(b"\r\n.\r\n");
    vec![out]
}

/// DELE: mark a live message for deletion. Idempotent rejection on already-gone.
fn dele(md: &mut Maildrop, num: u32) -> Vec<Vec<u8>> {
    match md.live(num) {
        Some(idx) => {
            md.slots[idx].deleted = true;
            count_cmd("DELE", true);
            ok(&format!("message {num} deleted"))
        }
        None => {
            count_cmd("DELE", false);
            err("no such message")
        }
    }
}

/// Commit the deletions accumulated in the maildrop. Returns the number of
/// messages removed. Errors are logged but do not fail QUIT (client already
/// considers them gone); rows simply remain for the next session.
async fn commit_quit(state: &AppState, drop: &Option<Maildrop>) -> usize {
    let Some(md) = drop else {
        return 0;
    };
    let ids: Vec<Uuid> = md
        .slots
        .iter()
        .filter(|s| s.deleted)
        .map(|s| s.msg.id)
        .collect();
    let removed = ids.len();
    if let Err(e) = store::delete_messages(state, &ids, md.tenant_id).await {
        warn!(user_id = %md.user_id, error = %e, "POP3 QUIT delete failed");
        return 0;
    }
    removed
}

/// Byte-stuff a message body per RFC 1939 §3: any line beginning with '.'
/// gets an extra leading '.' so the terminator octet is unambiguous.
fn dot_stuff(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + raw.len() / 64 + 1);
    let mut at_line_start = true;
    for &b in raw {
        if at_line_start && b == b'.' {
            out.push(b'.');
        }
        out.push(b);
        at_line_start = b == b'\n';
    }
    out
}

/// Return the header block plus the first `body_lines` lines of the body,
/// for the TOP command. The CRLF separating headers from body is included.
fn top_bytes(raw: &[u8], body_lines: usize) -> Vec<u8> {
    let sep = raw.windows(4).position(|w| w == b"\r\n\r\n");
    let Some(pos) = sep else {
        return raw.to_vec(); // no body separator — return whole message
    };
    let header_end = pos + 4;
    let mut out = raw[..header_end].to_vec();
    if body_lines == 0 {
        return out; // headers only
    }
    let body = &raw[header_end..];
    let mut taken = 0usize;
    let mut start = 0usize;
    for i in 0..body.len() {
        if body[i] == b'\n' {
            out.extend_from_slice(&body[start..=i]);
            start = i + 1;
            taken += 1;
            if taken >= body_lines {
                break;
            }
        }
    }
    // Body ends without a trailing newline and we still have budget: include it.
    if taken < body_lines && start < body.len() {
        out.extend_from_slice(&body[start..]);
    }
    out
}

fn capa() -> Vec<Vec<u8>> {
    count_cmd("CAPA", true);
    vec![b"+OK capability list follows\r\nUSER\r\nUIDL\r\nTOP\r\n.\r\n".to_vec()]
}

fn ok(msg: &str) -> Vec<Vec<u8>> {
    let line = if msg.is_empty() {
        "+OK\r\n".to_string()
    } else {
        format!("+OK {msg}\r\n")
    };
    vec![line.into_bytes()]
}

fn err(msg: &str) -> Vec<Vec<u8>> {
    vec![format!("-ERR {msg}\r\n").into_bytes()]
}

fn count_cmd(name: &str, ok: bool) {
    let outcome = if ok { "ok" } else { "err" };
    POP3_COMMANDS_TOTAL
        .with_label_values(&[name, outcome])
        .inc();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(size: i64) -> Slot {
        Slot {
            msg: Pop3Message {
                id: Uuid::nil(),
                size,
                body_path: None,
            },
            deleted: false,
        }
    }

    fn md(sizes: &[i64]) -> Maildrop {
        Maildrop {
            user_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            slots: sizes.iter().map(|&s| slot(s)).collect(),
        }
    }

    #[test]
    fn stat_counts_live_only() {
        let mut m = md(&[100, 200, 300]);
        m.slots[1].deleted = true;
        assert_eq!(m.count(), 2);
        assert_eq!(m.total_size(), 400);
    }

    #[test]
    fn live_resolves_one_based() {
        let m = md(&[10, 20]);
        assert_eq!(m.live(1), Some(0));
        assert_eq!(m.live(2), Some(1));
        assert_eq!(m.live(3), None);
        assert_eq!(m.live(0), None);
    }

    #[test]
    fn live_skips_deleted() {
        let mut m = md(&[10, 20]);
        m.slots[0].deleted = true;
        assert_eq!(m.live(1), None);
        assert_eq!(m.live(2), Some(1));
    }

    #[test]
    fn dot_stuff_doubles_leading_dot() {
        let got = dot_stuff(b".hidden\r\nnormal\r\n");
        assert_eq!(&got, b"..hidden\r\nnormal\r\n");
    }

    #[test]
    fn dot_stuff_only_at_line_start() {
        let got = dot_stuff(b"a.b\r\n.c\r\n");
        assert_eq!(&got, b"a.b\r\n..c\r\n");
    }

    #[test]
    fn dot_stuff_empty() {
        assert!(dot_stuff(b"").is_empty());
    }

    #[test]
    fn top_returns_headers_plus_n_body_lines() {
        let raw = b"Subject: hi\r\n\r\nline1\r\nline2\r\nline3\r\n";
        let got = top_bytes(raw, 2);
        assert_eq!(&got, b"Subject: hi\r\n\r\nline1\r\nline2\r\n");
    }

    #[test]
    fn top_zero_lines_returns_headers_only() {
        let raw = b"Subject: hi\r\n\r\nbody\r\n";
        let got = top_bytes(raw, 0);
        assert_eq!(&got, b"Subject: hi\r\n\r\n");
    }

    #[test]
    fn top_no_separator_returns_whole() {
        let raw = b"no body separator here";
        assert_eq!(top_bytes(raw, 5), raw.to_vec());
    }

    #[test]
    fn top_more_lines_than_body_returns_all() {
        let raw = b"H: v\r\n\r\nonly\r\n";
        let got = top_bytes(raw, 99);
        assert_eq!(&got, b"H: v\r\n\r\nonly\r\n");
    }

    #[test]
    fn scan_value_size_vs_uidl() {
        let s = slot(42);
        assert_eq!(scan_value(&s, false), "42");
        assert_eq!(scan_value(&s, true), "00000000000000000000000000000000");
    }
}

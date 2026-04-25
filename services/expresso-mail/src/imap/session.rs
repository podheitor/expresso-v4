//! IMAP session state machine — one per TCP connection.
//! Handles core IMAP4rev1 commands: CAPABILITY, LOGIN, LIST, SELECT,
//! FETCH, STORE, EXPUNGE, CLOSE, LOGOUT, NOOP.
//!
//! Tenant scoping: após LOGIN, `tenant_id` é propagado para todo handler
//! subsequente e cada query aplica `AND tenant_id = $` explícito. Sem isso,
//! um mailbox_id ou user_id vazado (via misconfig/log/debug endpoint) daria
//! acesso cross-tenant — a RLS de `mailboxes`/`messages` é NULL-bypass e
//! não bloqueia operações IMAP que rodam fora de `begin_tenant_tx`.

use std::num::NonZeroU32;

use imap_codec::{
    CommandCodec, GreetingCodec, ResponseCodec,
    decode::{CommandDecodeError, Decoder},
    encode::Encoder,
    imap_types::{
        command::{Command, CommandBody},
        core::{Atom, AString, IString, NString, Tag, Text, Vec1},
        fetch::{MacroOrMessageDataItemNames, MessageDataItem},
        flag::{Flag, FlagFetch, FlagNameAttribute, FlagPerm},
        mailbox::Mailbox as ImapMailbox,
        response::{
            Bye, Code, Data, Greeting, Response, Status, StatusBody,
            StatusKind, Tagged,
        },
        IntoStatic,
    },
};
use chrono::{DateTime as ChronoDateTime, FixedOffset, Utc};
use imap_codec::imap_types::datetime::DateTime as ImapDateTime;
use sqlx::Row;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, warn};
use uuid::Uuid;

use std::sync::OnceLock;

use crate::state::AppState;
use crate::imap::lockout::LoginLockout;
use crate::imap::metrics::{
    command_label, IMAP_COMMANDS_TOTAL, IMAP_LOGINS_TOTAL, IMAP_SESSIONS_TOTAL,
};

/// Singleton de lockout por-username — cross-connection (defesa contra
/// brute-force distribuído no mesmo username vindo de N clientes).
/// Defaults: 10 falhas/60s → 5min lockout (mesmo perfil do #105).
static LOGIN_LOCKOUT: OnceLock<LoginLockout> = OnceLock::new();

fn login_lockout() -> &'static LoginLockout {
    LOGIN_LOCKOUT.get_or_init(LoginLockout::default)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    NotAuthenticated,
    Authenticated,
    Selected,
}

struct SelectedMailbox {
    mailbox_id: Uuid,
    exists: u32,
}

pub async fn handle(stream: TcpStream, state: AppState) -> anyhow::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let cmd_codec = CommandCodec::default();
    let resp_codec = ResponseCodec::default();
    let greet_codec = GreetingCodec::default();

    // Send greeting via GreetingCodec
    let greeting = Greeting::ok(None, "Expresso IMAP4rev1 ready")
        .expect("valid greeting text");
    writer
        .write_all(&greet_codec.encode(&greeting).dump())
        .await?;

    let mut sess = SessionState::NotAuthenticated;
    let mut user_id: Option<Uuid> = None;
    let mut tenant_id: Option<Uuid> = None;
    let mut selected: Option<SelectedMailbox> = None;
    let mut buf = Vec::with_capacity(64 * 1024);

    let mut tmp = [0u8; 4096];
    loop {
        let n = reader.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);

        loop {
            match cmd_codec.decode(&buf) {
                Ok((remaining, cmd)) => {
                    let consumed = buf.len() - remaining.len();
                    let cmd = cmd.into_static();
                    let tag_s = cmd.tag.as_ref().to_owned();
                    buf.drain(..consumed);

                    let cmd_name = cmd.body.name();
                    debug!(tag = %tag_s, cmd = ?cmd_name, "imap ←");

                    let responses = dispatch(
                        &state, &cmd, &mut sess, &mut user_id, &mut tenant_id, &mut selected,
                    )
                    .await;

                    IMAP_COMMANDS_TOTAL
                        .with_label_values(&[command_label(cmd_name), outcome_of(&responses)])
                        .inc();

                    for resp in &responses {
                        writer.write_all(&resp_codec.encode(resp).dump()).await?;
                    }

                    if matches!(cmd.body, CommandBody::Logout) {
                        return Ok(());
                    }
                }
                Err(CommandDecodeError::Incomplete) => break,
                Err(CommandDecodeError::LiteralFound { length, .. }) => {
                    // Accept literal continuation
                    let cont = format!("+ Ready for {} bytes\r\n", length);
                    writer.write_all(cont.as_bytes()).await?;
                    // Keep reading — literal data will be appended to buf
                    break;
                }
                Err(CommandDecodeError::Failed) => {
                    IMAP_SESSIONS_TOTAL.with_label_values(&["parse_error"]).inc();
                    writer.write_all(b"* BAD parse error\r\n").await?;
                    buf.clear();
                    break;
                }
            }
        }
    }

    Ok(())
}

// ─── Dispatch ────────────────────────────────────────────────────────────────

async fn dispatch(
    state: &AppState,
    cmd: &Command<'static>,
    sess: &mut SessionState,
    user_id: &mut Option<Uuid>,
    tenant_id: &mut Option<Uuid>,
    selected: &mut Option<SelectedMailbox>,
) -> Vec<Response<'static>> {
    let tag = cmd.tag.clone();
    match &cmd.body {
        CommandBody::Capability => cmd_capability(tag),
        CommandBody::Noop => cmd_noop(state, tag, selected, *tenant_id).await,
        CommandBody::Logout => cmd_logout(tag),
        CommandBody::Login { username, password } => {
            let user = astring_to_string(username);
            let pass = astring_to_string(password.declassify());
            cmd_login(state, tag, &user, &pass, sess, user_id, tenant_id).await
        }
        CommandBody::List {
            reference,
            mailbox_wildcard: _,
        } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_list(state, tag, reference, user_id.unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Select { mailbox, .. } | CommandBody::Examine { mailbox, .. } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_select(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap(), sess, selected).await
        }
        CommandBody::Fetch {
            sequence_set,
            macro_or_item_names,
            ..
        } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_fetch(state, tag, sequence_set, macro_or_item_names, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Store {
            sequence_set,
            kind,
            response: _,
            flags,
            ..
        } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_store(state, tag, sequence_set, kind, flags, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Close => cmd_close(state, tag, sess, selected, *tenant_id).await,
        CommandBody::Expunge => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_expunge(state, tag, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        _ => {
            let msg = format!("{} not implemented", cmd.body.name());
            vec![bad_tagged(tag, &msg)]
        }
    }
}

// ─── Command handlers ───────────────────────────────────────────────────────

fn cmd_capability(tag: Tag<'static>) -> Vec<Response<'static>> {
    let caps = Vec1::try_from(vec![imap_codec::imap_types::response::Capability::Imap4Rev1]).unwrap();
    vec![
        Response::Data(Data::Capability(caps)),
        ok_tagged(tag, None, "CAPABILITY completed"),
    ]
}

fn cmd_logout(tag: Tag<'static>) -> Vec<Response<'static>> {
    vec![
        Response::Status(Status::Bye(Bye {
            code: None,
            text: Text::try_from("server logging out").unwrap(),
        })),
        ok_tagged(tag, None, "LOGOUT completed"),
    ]
}

async fn cmd_login(
    state: &AppState,
    tag: Tag<'static>,
    user: &str,
    pass: &str,
    sess: &mut SessionState,
    user_id: &mut Option<Uuid>,
    tenant_id: &mut Option<Uuid>,
) -> Vec<Response<'static>> {
    let lock = login_lockout();
    if lock.is_locked_out(user) {
        // Trata como falha sem bater no DB — economiza bcrypt round e
        // garante que rajada de tentativas pós-lockout não custa nada.
        // Retornamos a mesma mensagem genérica do path normal pra não
        // expor pro atacante que ele bateu no lockout vs. senha errada.
        IMAP_LOGINS_TOTAL.with_label_values(&["locked_out"]).inc();
        warn!(user = %user, "IMAP login refused (locked out)");
        return vec![no_tagged(tag, "LOGIN failed")];
    }

    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, tenant_id FROM users WHERE lower(email) = lower($1) AND password_hash = crypt($2, password_hash) LIMIT 1",
    )
    .bind(user)
    .bind(pass)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten();

    match row {
        Some((uid, tid)) => {
            *sess = SessionState::Authenticated;
            *user_id = Some(uid);
            *tenant_id = Some(tid);
            lock.clear_failures(user);
            IMAP_LOGINS_TOTAL.with_label_values(&["success"]).inc();
            vec![ok_tagged(tag, None, "LOGIN completed")]
        }
        None => {
            lock.record_failure(user);
            IMAP_LOGINS_TOTAL.with_label_values(&["failure"]).inc();
            warn!(user = %user, "IMAP login failed");
            vec![no_tagged(tag, "LOGIN failed")]
        }
    }
}

async fn cmd_list(
    state: &AppState,
    tag: Tag<'static>,
    _reference: &ImapMailbox<'_>,
    uid: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT folder_name, special_use FROM mailboxes \
         WHERE user_id = $1 AND tenant_id = $2 ORDER BY folder_name",
    )
    .bind(uid)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    let mut out: Vec<Response<'static>> = Vec::with_capacity(rows.len() + 1);
    for (name, special_use) in &rows {
        let mailbox = ImapMailbox::try_from(name.to_owned())
            .unwrap_or_else(|_| ImapMailbox::Inbox);

        // Emite RFC 6154 special-use attribute se presente (e.g. "\\Sent" → Atom "Sent").
        // DB armazena com backslash; Atom não aceita backslash — strip antes.
        // From<Atom> for FlagNameAttribute produz a extension attribute correta.
        let items: Vec<FlagNameAttribute<'static>> = special_use
            .as_deref()
            .and_then(|s| {
                let bare = s.trim().trim_start_matches('\\');
                if bare.is_empty() { return None; }
                Atom::try_from(bare.to_owned()).ok().map(FlagNameAttribute::from)
            })
            .into_iter()
            .collect();

        out.push(Response::Data(Data::List {
            items,
            delimiter: Some(imap_codec::imap_types::core::QuotedChar::try_from('.').unwrap()),
            mailbox,
        }));
    }
    out.push(ok_tagged(tag, None, "LIST completed"));
    out
}

async fn cmd_select(
    state: &AppState,
    tag: Tag<'static>,
    mailbox: &ImapMailbox<'_>,
    uid: Uuid,
    tenant_id: Uuid,
    sess: &mut SessionState,
    selected: &mut Option<SelectedMailbox>,
) -> Vec<Response<'static>> {
    let mbox_name = mailbox_to_string(mailbox);

    // Lê uid_validity, next_uid e message_count diretamente da tabela
    // mailboxes — ambos mantidos pelo DB (uid_validity imutável desde criação;
    // next_uid e message_count atualizados por trigger em INSERT/DELETE).
    // A correlated subquery COUNT(*) que estava aqui ignorava o counter de
    // trigger e relançava a UIDVALIDITY derivada do UUID (incorreto).
    let row: Option<(Uuid, i64, i64, i64)> = sqlx::query_as(
        "SELECT id, uid_validity, next_uid, message_count \
         FROM mailboxes \
         WHERE user_id = $1 AND folder_name = $2 AND tenant_id = $3",
    )
    .bind(uid)
    .bind(&mbox_name)
    .bind(tenant_id)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten();

    match row {
        Some((mailbox_id, uid_validity_raw, next_uid_raw, count)) => {
            let exists = count as u32;
            let uid_validity = NonZeroU32::new(uid_validity_raw as u32).unwrap_or(NonZeroU32::MIN);
            let uid_next    = NonZeroU32::new(next_uid_raw    as u32).unwrap_or(NonZeroU32::MIN);
            *sess = SessionState::Selected;
            *selected = Some(SelectedMailbox { mailbox_id, exists });

            // RFC 3501 §7.3.1: FLAGS, EXISTS, RECENT, UIDVALIDITY, UIDNEXT e
            // PERMANENTFLAGS são todos obrigatórios/SHOULD na resposta SELECT.
            vec![
                Response::Data(Data::Flags(vec![
                    Flag::Seen,
                    Flag::Answered,
                    Flag::Flagged,
                    Flag::Deleted,
                    Flag::Draft,
                ])),
                Response::Data(Data::Exists(exists)),
                Response::Data(Data::Recent(0)),
                untagged_ok(Code::UidValidity(uid_validity), "UIDs valid"),
                untagged_ok(Code::UidNext(uid_next), "predicted next UID"),
                untagged_ok(
                    Code::PermanentFlags(vec![
                        FlagPerm::Flag(Flag::Seen),
                        FlagPerm::Flag(Flag::Answered),
                        FlagPerm::Flag(Flag::Flagged),
                        FlagPerm::Flag(Flag::Deleted),
                        FlagPerm::Flag(Flag::Draft),
                        FlagPerm::Asterisk,
                    ]),
                    "Limited",
                ),
                ok_tagged(tag, Some(Code::ReadWrite), "SELECT completed"),
            ]
        }
        None => vec![no_tagged(tag, "mailbox not found")],
    }
}

async fn cmd_fetch(
    state: &AppState,
    tag: Tag<'static>,
    sequence_set: &imap_codec::imap_types::sequence::SequenceSet,
    macro_or: &MacroOrMessageDataItemNames<'_>,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let (start, end) = sequence_range(sequence_set, sel.exists);

    let rows = sqlx::query(
        "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, \
         uid, subject, from_addr, from_name, date, flags, size_bytes, received_at \
         FROM messages WHERE mailbox_id = $1 AND tenant_id = $4 \
         ORDER BY received_at ASC OFFSET $2 LIMIT $3",
    )
    .bind(sel.mailbox_id)
    .bind((start - 1) as i64)
    .bind((end - start + 1) as i64)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    let w_flags        = wants(macro_or, "FLAGS");
    let w_envelope     = wants(macro_or, "ENVELOPE");
    let w_size         = wants(macro_or, "RFC822.SIZE");
    let w_uid          = wants(macro_or, "UID");
    let w_internaldate = wants(macro_or, "INTERNALDATE");

    let mut out: Vec<Response<'static>> = Vec::with_capacity(rows.len() + 1);
    for row in &rows {
        let seq_num: i64 = row.get("seq");
        let seq = NonZeroU32::new(seq_num as u32).unwrap_or(NonZeroU32::MIN);
        let mut items: Vec<MessageDataItem<'static>> = Vec::new();

        if w_uid {
            let uid_val: i64 = row.get("uid");
            items.push(MessageDataItem::Uid(
                NonZeroU32::new(uid_val as u32).unwrap_or(NonZeroU32::MIN),
            ));
        }
        if w_flags {
            let flags_val: Vec<String> = row.get("flags");
            let flags: Vec<FlagFetch<'static>> = flags_val
                .iter()
                .filter_map(|f| parse_flag(f).map(FlagFetch::Flag))
                .collect();
            items.push(MessageDataItem::Flags(flags));
        }
        if w_size {
            let sz: i32 = row.get("size_bytes");
            items.push(MessageDataItem::Rfc822Size(sz as u32));
        }
        if w_envelope {
            let subject: Option<String> = row.get("subject");
            let from_addr: Option<String> = row.get("from_addr");
            let from_name: Option<String> = row.get("from_name");
            items.push(MessageDataItem::Envelope(build_envelope(
                subject.as_deref(),
                from_addr.as_deref(),
                from_name.as_deref(),
            )));
        }
        if w_internaldate {
            // INTERNALDATE = received_at (TIMESTAMPTZ) convertido para
            // imap_types::DateTime via chrono::DateTime<FixedOffset> (UTC+0).
            // RFC 3501 §2.3.3: "the internal date and time of the message".
            let ts: Option<ChronoDateTime<Utc>> = row.try_get("received_at").ok();
            if let Some(t) = ts {
                let fixed: ChronoDateTime<FixedOffset> = t.with_timezone(&FixedOffset::east_opt(0).unwrap());
                if let Ok(imap_dt) = ImapDateTime::try_from(fixed) {
                    items.push(MessageDataItem::InternalDate(imap_dt));
                }
            }
        }

        if let Ok(v) = Vec1::try_from(items) {
            out.push(Response::Data(Data::Fetch { seq, items: v }));
        }
    }

    out.push(ok_tagged(tag, None, "FETCH completed"));
    out
}

async fn cmd_store(
    state: &AppState,
    tag: Tag<'static>,
    sequence_set: &imap_codec::imap_types::sequence::SequenceSet,
    kind: &imap_codec::imap_types::flag::StoreType,
    flags: &[Flag<'_>],
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let (start, end) = sequence_range(sequence_set, sel.exists);
    let flag_strs: Vec<String> = flags.iter().map(|f| flag_to_str(f).to_owned()).collect();

    let sql = match kind {
        imap_codec::imap_types::flag::StoreType::Add => {
            "UPDATE messages SET flags = array_cat(flags, $1::text[]) \
             WHERE mailbox_id = $2 AND uid >= $3 AND uid <= $4 AND tenant_id = $5"
        }
        imap_codec::imap_types::flag::StoreType::Remove => {
            "UPDATE messages SET flags = (SELECT array_agg(e) FROM unnest(flags) e WHERE NOT e = ANY($1::text[])) \
             WHERE mailbox_id = $2 AND uid >= $3 AND uid <= $4 AND tenant_id = $5"
        }
        imap_codec::imap_types::flag::StoreType::Replace => {
            "UPDATE messages SET flags = $1::text[] \
             WHERE mailbox_id = $2 AND uid >= $3 AND uid <= $4 AND tenant_id = $5"
        }
    };

    let _ = sqlx::query(sql)
        .bind(&flag_strs)
        .bind(sel.mailbox_id)
        .bind(start as i64)
        .bind(end as i64)
        .bind(tenant_id)
        .execute(state.db())
        .await;

    vec![ok_tagged(tag, None, "STORE completed")]
}

async fn cmd_expunge(
    state: &AppState,
    tag: Tag<'static>,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let _ = sqlx::query(
        "DELETE FROM messages \
         WHERE mailbox_id = $1 AND tenant_id = $2 AND '\\Deleted' = ANY(flags)",
    )
    .bind(sel.mailbox_id)
    .bind(tenant_id)
    .execute(state.db())
    .await;

    vec![ok_tagged(tag, None, "EXPUNGE completed")]
}

/// NOOP — RFC 3501 §6.1.2: clients use this as a polling beat to discover
/// new messages without re-SELECTing. If a mailbox is currently selected we
/// re-query its message count and emit an untagged `* N EXISTS` when it
/// changed since SELECT, then update the cached count so subsequent FETCH
/// sequence math stays consistent. Outside the selected state, NOOP is a
/// pure liveness probe and we just return OK.
async fn cmd_noop(
    state: &AppState,
    tag: Tag<'static>,
    selected: &mut Option<SelectedMailbox>,
    tenant_id: Option<Uuid>,
) -> Vec<Response<'static>> {
    let mut out: Vec<Response<'static>> = Vec::with_capacity(2);
    if let (Some(sel), Some(tid)) = (selected.as_mut(), tenant_id) {
        // Usa message_count (trigger-maintained em mailboxes) em vez de COUNT(*) —
        // evita full scan em mailboxes com muitas mensagens durante polling de NOOP.
        let count: Option<i64> = sqlx::query_scalar(
            "SELECT message_count FROM mailboxes WHERE id = $1 AND tenant_id = $2",
        )
        .bind(sel.mailbox_id)
        .bind(tid)
        .fetch_optional(state.db())
        .await
        .ok()
        .flatten();
        if let Some(c) = count {
            let new_exists = c as u32;
            if new_exists != sel.exists {
                sel.exists = new_exists;
                out.push(Response::Data(Data::Exists(new_exists)));
            }
        }
    }
    out.push(ok_tagged(tag, None, "NOOP completed"));
    out
}

/// CLOSE — RFC 3501 §6.4.2: silently expunge `\Deleted` messages from the
/// selected mailbox and return to authenticated state. Crucially, NO untagged
/// EXPUNGE responses are sent (that is the whole point vs. a plain EXPUNGE):
/// clients use CLOSE to drop a mailbox without paying for a per-message
/// stream. Always treat the mailbox as read-write here — `cmd_select` grants
/// `Code::ReadWrite` unconditionally, so we never enter the read-only branch
/// where the spec says CLOSE skips the expunge.
async fn cmd_close(
    state: &AppState,
    tag: Tag<'static>,
    sess: &mut SessionState,
    selected: &mut Option<SelectedMailbox>,
    tenant_id: Option<Uuid>,
) -> Vec<Response<'static>> {
    if let (Some(sel), Some(tid)) = (selected.as_ref(), tenant_id) {
        let _ = sqlx::query(
            "DELETE FROM messages \
             WHERE mailbox_id = $1 AND tenant_id = $2 AND '\\Deleted' = ANY(flags)",
        )
        .bind(sel.mailbox_id)
        .bind(tid)
        .execute(state.db())
        .await;
    }
    *selected = None;
    if *sess == SessionState::Selected {
        *sess = SessionState::Authenticated;
    }
    vec![ok_tagged(tag, None, "CLOSE completed")]
}

// ─── Response helpers ────────────────────────────────────────────────────────

/// Map a dispatch's response sequence to a single outcome label for the
/// `mail_imap_commands_total` counter. The first tagged Status drives the
/// label; absent it (untagged-only chatter) we treat as `ok`.
fn outcome_of(responses: &[Response<'static>]) -> &'static str {
    for r in responses {
        if let Response::Status(Status::Tagged(t)) = r {
            return match t.body.kind {
                StatusKind::Ok  => "ok",
                StatusKind::No  => "no",
                StatusKind::Bad => "bad",
            };
        }
    }
    "ok"
}

fn untagged_ok(code: Code<'static>, text: &str) -> Response<'static> {
    Response::Status(Status::Untagged(StatusBody {
        kind: StatusKind::Ok,
        code: Some(code),
        text: Text::try_from(text.to_owned()).unwrap(),
    }))
}

fn ok_tagged(tag: Tag<'static>, code: Option<Code<'static>>, text: &str) -> Response<'static> {
    Response::Status(Status::Tagged(Tagged {
        tag,
        body: StatusBody {
            kind: StatusKind::Ok,
            code,
            text: Text::try_from(text.to_owned()).unwrap(),
        },
    }))
}

fn no_tagged(tag: Tag<'static>, text: &str) -> Response<'static> {
    Response::Status(Status::Tagged(Tagged {
        tag,
        body: StatusBody {
            kind: StatusKind::No,
            code: None,
            text: Text::try_from(text.to_owned()).unwrap(),
        },
    }))
}

fn bad_tagged(tag: Tag<'static>, text: &str) -> Response<'static> {
    Response::Status(Status::Tagged(Tagged {
        tag,
        body: StatusBody {
            kind: StatusKind::Bad,
            code: None,
            text: Text::try_from(text.to_owned()).unwrap(),
        },
    }))
}

// ─── Utility ─────────────────────────────────────────────────────────────────

fn astring_to_string(a: &AString<'_>) -> String {
    match a {
        AString::Atom(atom) => atom.inner().to_string(),
        AString::String(istring) => match istring {
            IString::Literal(lit) => String::from_utf8_lossy(lit.as_ref()).to_string(),
            IString::Quoted(q) => q.inner().to_string(),
        },
    }
}

fn mailbox_to_string(m: &ImapMailbox<'_>) -> String {
    match m {
        ImapMailbox::Inbox => "INBOX".to_owned(),
        ImapMailbox::Other(other) => {
            String::from_utf8_lossy(other.as_ref()).to_string()
        }
    }
}

fn flag_to_str(f: &Flag<'_>) -> &'static str {
    match f {
        Flag::Seen => "\\Seen",
        Flag::Answered => "\\Answered",
        Flag::Flagged => "\\Flagged",
        Flag::Deleted => "\\Deleted",
        Flag::Draft => "\\Draft",
        _ => "\\Seen",
    }
}

fn parse_flag(s: &str) -> Option<Flag<'static>> {
    match s {
        "\\Seen" => Some(Flag::Seen),
        "\\Answered" => Some(Flag::Answered),
        "\\Flagged" => Some(Flag::Flagged),
        "\\Deleted" => Some(Flag::Deleted),
        "\\Draft" => Some(Flag::Draft),
        _ => None,
    }
}

fn sequence_range(
    seq_set: &imap_codec::imap_types::sequence::SequenceSet,
    exists: u32,
) -> (u32, u32) {
    let mut start = 1u32;
    let mut end = exists;

    for seq in seq_set.0.as_ref() {
        match seq {
            imap_codec::imap_types::sequence::Sequence::Single(val) => {
                let n = seq_or_uid_val(val, exists);
                start = n;
                end = n;
            }
            imap_codec::imap_types::sequence::Sequence::Range(from, to) => {
                start = seq_or_uid_val(from, exists);
                end = seq_or_uid_val(to, exists);
            }
        }
    }
    (start.max(1), end.min(exists).max(start))
}

fn seq_or_uid_val(val: &imap_codec::imap_types::sequence::SeqOrUid, exists: u32) -> u32 {
    match val {
        imap_codec::imap_types::sequence::SeqOrUid::Value(n) => n.get(),
        imap_codec::imap_types::sequence::SeqOrUid::Asterisk => exists,
    }
}

fn wants(macro_or: &MacroOrMessageDataItemNames<'_>, name: &str) -> bool {
    use imap_codec::imap_types::fetch::Macro;
    match macro_or {
        MacroOrMessageDataItemNames::Macro(m) => match m {
            Macro::All => matches!(name, "FLAGS" | "ENVELOPE" | "RFC822.SIZE" | "INTERNALDATE"),
            Macro::Fast => matches!(name, "FLAGS" | "RFC822.SIZE" | "INTERNALDATE"),
            Macro::Full => matches!(name, "FLAGS" | "ENVELOPE" | "RFC822.SIZE" | "INTERNALDATE" | "BODY"),
            _ => false,
        },
        MacroOrMessageDataItemNames::MessageDataItemNames(items) => {
            items.iter().any(|item| format!("{:?}", item).to_uppercase().contains(name))
        }
    }
}

fn build_envelope(
    subject: Option<&str>,
    from_addr: Option<&str>,
    from_name: Option<&str>,
) -> imap_codec::imap_types::envelope::Envelope<'static> {
    use imap_codec::imap_types::envelope::{Address, Envelope};

    let subject_ns = subject
        .and_then(|s| NString::try_from(s.to_owned()).ok())
        .unwrap_or(NString(None));

    let from = from_addr.map(|addr| {
        let (local, domain) = addr.split_once('@').unwrap_or((addr, ""));
        Address {
            name: from_name
                .and_then(|n| NString::try_from(n.to_owned()).ok())
                .unwrap_or(NString(None)),
            adl: NString(None),
            mailbox: NString::try_from(local.to_owned())
                .ok()
                .unwrap_or(NString(None)),
            host: NString::try_from(domain.to_owned())
                .ok()
                .unwrap_or(NString(None)),
        }
    });

    let from_list = from.into_iter().collect::<Vec<_>>();

    Envelope {
        date: NString(None),
        subject: subject_ns,
        from: from_list.clone(),
        sender: from_list.clone(),
        reply_to: from_list.clone(),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        in_reply_to: NString(None),
        message_id: NString(None),
    }
}

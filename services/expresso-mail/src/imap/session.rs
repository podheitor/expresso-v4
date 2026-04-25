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
        core::{Atom, AString, IString, Literal, NString, Tag, Text, Vec1},
        fetch::{MacroOrMessageDataItemNames, MessageDataItem},
        flag::{Flag, FlagFetch, FlagNameAttribute, FlagPerm},
        mailbox::Mailbox as ImapMailbox,
        response::{
            Bye, Capability, Code, Data, Greeting, Response, Status, StatusBody,
            StatusKind, Tagged,
        },
        status::{StatusDataItem, StatusDataItemName},
        extensions::binary::LiteralOrLiteral8,
        extensions::uidplus::{UidElement, UidSet},
        search::SearchKey,
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
    read_only: bool,
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

        'decode: loop {
            match cmd_codec.decode(&buf) {
                Ok((remaining, cmd)) => {
                    let consumed = buf.len() - remaining.len();
                    let cmd = cmd.into_static();
                    let tag_s = cmd.tag.as_ref().to_owned();
                    buf.drain(..consumed);

                    let cmd_name = cmd.body.name();
                    debug!(tag = %tag_s, cmd = ?cmd_name, "imap ←");

                    // IDLE (RFC 2177) — tratado inline, não via dispatch.
                    // handle_idle() envia "+ idling", espera DONE do cliente
                    // (com polls de 28s para push de EXISTS), envia OK ao fim.
                    if let CommandBody::Idle = &cmd.body {
                        let outcome = if sess == SessionState::NotAuthenticated {
                            let resp = no_tagged(cmd.tag.clone(), "not authenticated");
                            writer.write_all(&resp_codec.encode(&resp).dump()).await?;
                            "no"
                        } else {
                            handle_idle(
                                cmd.tag.clone(), &mut reader, &mut writer,
                                &resp_codec, &state, &mut selected, tenant_id,
                            ).await?;
                            "ok"
                        };
                        IMAP_COMMANDS_TOTAL.with_label_values(&["IDLE", outcome]).inc();
                        buf.clear();
                        break 'decode;
                    }

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
                Err(CommandDecodeError::Incomplete) => break 'decode,
                Err(CommandDecodeError::LiteralFound { length, .. }) => {
                    // Accept literal continuation
                    let cont = format!("+ Ready for {} bytes\r\n", length);
                    writer.write_all(cont.as_bytes()).await?;
                    // Keep reading — literal data will be appended to buf
                    break 'decode;
                }
                Err(CommandDecodeError::Failed) => {
                    IMAP_SESSIONS_TOTAL.with_label_values(&["parse_error"]).inc();
                    writer.write_all(b"* BAD parse error\r\n").await?;
                    buf.clear();
                    break 'decode;
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
        CommandBody::Select { mailbox, .. } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_select(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap(), sess, selected, false).await
        }
        CommandBody::Examine { mailbox, .. } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_select(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap(), sess, selected, true).await
        }
        CommandBody::Subscribe { mailbox } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_subscribe(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap(), true).await
        }
        CommandBody::Unsubscribe { mailbox } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_subscribe(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap(), false).await
        }
        CommandBody::Lsub { reference, .. } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_lsub(state, tag, reference, user_id.unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Status { mailbox, item_names } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_status(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap(), item_names.as_ref()).await
        }
        CommandBody::Search { criteria, uid, .. } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_search(state, tag, criteria.as_ref(), *uid, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Append { mailbox, flags, message, .. } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_append(state, tag, mailbox, flags, message, user_id.unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Fetch {
            sequence_set,
            macro_or_item_names,
            uid,
            ..
        } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_fetch(state, tag, sequence_set, macro_or_item_names, *uid, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Store {
            sequence_set,
            kind,
            response: _,
            flags,
            uid,
            ..
        } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_store(state, tag, sequence_set, kind, flags, *uid, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Close => cmd_close(state, tag, sess, selected, *tenant_id).await,
        CommandBody::Expunge => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_expunge(state, tag, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Copy { sequence_set, mailbox, uid, .. } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_copy(state, tag, sequence_set, mailbox, *uid, selected.as_ref().unwrap(), user_id.unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::ExpungeUid { sequence_set } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_expunge_uid(state, tag, sequence_set, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        _ => {
            let msg = format!("{} not implemented", cmd.body.name());
            vec![bad_tagged(tag, &msg)]
        }
    }
}

// ─── Command handlers ───────────────────────────────────────────────────────

fn cmd_capability(tag: Tag<'static>) -> Vec<Response<'static>> {
    let caps = Vec1::try_from(vec![
        Capability::Imap4Rev1,
        Capability::Idle,
        Capability::UidPlus,
    ]).unwrap();
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

/// SUBSCRIBE / UNSUBSCRIBE — RFC 3501 §6.3.6-7.
/// Marks a mailbox as subscribed (true) or unsubscribed (false).
/// Idempotent — toggling an already-subscribed/unsubscribed mailbox is a no-op.
async fn cmd_subscribe(
    state: &AppState,
    tag: Tag<'static>,
    mailbox: &ImapMailbox<'_>,
    uid: Uuid,
    tenant_id: Uuid,
    subscribe: bool,
) -> Vec<Response<'static>> {
    let mbox_name = mailbox_to_string(mailbox);
    let _ = sqlx::query(
        "UPDATE mailboxes SET subscribed = $1 \
         WHERE user_id = $2 AND folder_name = $3 AND tenant_id = $4",
    )
    .bind(subscribe)
    .bind(uid)
    .bind(&mbox_name)
    .bind(tenant_id)
    .execute(state.db())
    .await;

    let msg = if subscribe { "SUBSCRIBE completed" } else { "UNSUBSCRIBE completed" };
    vec![ok_tagged(tag, None, msg)]
}

/// LSUB — RFC 3501 §6.3.9: list only subscribed mailboxes.
/// Same format as LIST responses but uses Data::Lsub.
async fn cmd_lsub(
    state: &AppState,
    tag: Tag<'static>,
    _reference: &ImapMailbox<'_>,
    uid: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT folder_name, special_use FROM mailboxes \
         WHERE user_id = $1 AND tenant_id = $2 AND subscribed = TRUE \
         ORDER BY folder_name",
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
        let items: Vec<FlagNameAttribute<'static>> = special_use
            .as_deref()
            .and_then(|s| {
                let bare = s.trim().trim_start_matches('\\');
                if bare.is_empty() { return None; }
                Atom::try_from(bare.to_owned()).ok().map(FlagNameAttribute::from)
            })
            .into_iter()
            .collect();
        out.push(Response::Data(Data::Lsub {
            items,
            delimiter: Some(imap_codec::imap_types::core::QuotedChar::try_from('.').unwrap()),
            mailbox,
        }));
    }
    out.push(ok_tagged(tag, None, "LSUB completed"));
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
    read_only: bool,
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

    // Primeira mensagem não-vista — subquery que conta mensagens recebidas antes
    // da mais antiga sem \Seen, produzindo o seq number (1-based) correto.
    // Executada apenas se há mailbox válido e feita em paralelo não-bloqueante
    // ao match abaixo seria complexo; mantemos sequencial para simplicidade.
    match row {
        Some((mailbox_id, uid_validity_raw, next_uid_raw, count)) => {
            let exists = count as u32;
            let uid_validity = NonZeroU32::new(uid_validity_raw as u32).unwrap_or(NonZeroU32::MIN);
            let uid_next    = NonZeroU32::new(next_uid_raw    as u32).unwrap_or(NonZeroU32::MIN);

            // RFC 3501 §7.3.1 SHOULD: UNSEEN — seq (1-based) da 1ª msg sem \Seen.
            // Subquery conta msgs com received_at < menor received_at de msgs sem \Seen.
            let first_unseen: Option<NonZeroU32> = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) + 1 FROM messages \
                 WHERE mailbox_id = $1 AND tenant_id = $2 \
                   AND received_at < COALESCE((\
                     SELECT MIN(received_at) FROM messages \
                     WHERE mailbox_id = $1 AND tenant_id = $2 \
                       AND NOT '\\Seen' = ANY(flags)\
                   ), 'infinity'::timestamptz)",
            )
            .bind(mailbox_id)
            .bind(tenant_id)
            .fetch_optional(state.db())
            .await
            .ok()
            .flatten()
            .and_then(|n| NonZeroU32::new(n as u32));

            *sess = SessionState::Selected;
            *selected = Some(SelectedMailbox { mailbox_id, exists, read_only });

            // RFC 3501 §7.3.1: FLAGS, EXISTS, RECENT, UIDVALIDITY, UIDNEXT e
            // PERMANENTFLAGS são todos obrigatórios/SHOULD na resposta SELECT.
            let mut out = vec![
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
            ];
            // Emite UNSEEN apenas se há de fato msgs não-vistas (seq <= exists).
            // Quando todas as msgs são vistas, COUNT(*)+1 > exists — omitido.
            if let Some(seq) = first_unseen.filter(|s| s.get() <= exists) {
                out.push(untagged_ok(Code::Unseen(seq), "first unseen"));
            }
            let (access_code, done_msg) = if read_only {
                (Code::ReadOnly, "EXAMINE completed")
            } else {
                (Code::ReadWrite, "SELECT completed")
            };
            out.push(ok_tagged(tag, Some(access_code), done_msg));
            out
        }
        None => vec![no_tagged(tag, "mailbox not found")],
    }
}

/// STATUS — RFC 3501 §6.3.10: returns per-mailbox counters without SELECTing.
/// Clients use this to refresh unread badges on folders they aren't currently viewing.
/// Reads trigger-maintained counters from `mailboxes` (O(1)) for MESSAGES/UNSEEN;
/// RECENT is always 0 (we don't track the \Recent flag per-session).
async fn cmd_status(
    state: &AppState,
    tag: Tag<'static>,
    mailbox: &ImapMailbox<'_>,
    uid: Uuid,
    tenant_id: Uuid,
    item_names: &[StatusDataItemName],
) -> Vec<Response<'static>> {
    let mbox_name = mailbox_to_string(mailbox);

    let row: Option<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT uid_validity, next_uid, message_count, unseen_count \
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

    let (uid_validity_raw, next_uid_raw, msg_count, unseen_count) = match row {
        None => return vec![no_tagged(tag, "mailbox not found")],
        Some(r) => r,
    };

    let uid_validity = NonZeroU32::new(uid_validity_raw as u32).unwrap_or(NonZeroU32::MIN);
    let uid_next     = NonZeroU32::new(next_uid_raw    as u32).unwrap_or(NonZeroU32::MIN);

    let mut items: Vec<StatusDataItem> = Vec::with_capacity(item_names.len());
    for name in item_names {
        let item = match name {
            StatusDataItemName::Messages    => StatusDataItem::Messages(msg_count as u32),
            StatusDataItemName::Recent      => StatusDataItem::Recent(0),
            StatusDataItemName::UidNext     => StatusDataItem::UidNext(uid_next),
            StatusDataItemName::UidValidity => StatusDataItem::UidValidity(uid_validity),
            StatusDataItemName::Unseen      => StatusDataItem::Unseen(unseen_count as u32),
            // Deleted/Size/DeletedStorage/HighestModSeq not tracked — skip silently.
            _ => continue,
        };
        items.push(item);
    }

    let mbox_static = ImapMailbox::try_from(mbox_name).unwrap_or(ImapMailbox::Inbox);
    vec![
        Response::Data(Data::Status {
            mailbox: mbox_static,
            items: std::borrow::Cow::Owned(items),
        }),
        ok_tagged(tag, None, "STATUS completed"),
    ]
}

/// SEARCH — RFC 3501 §6.4.4: return sequence numbers (or UIDs when uid=true)
/// of messages matching all criteria in `criteria` (implicit AND of the top-level
/// Vec; individual SearchKey may contain nested AND/OR/NOT).
///
/// Flag criteria are evaluated exactly. Since/Before/On are evaluated against
/// `received_at` (RFC 3501 §2.3.3 internal date). SentSince/SentBefore/SentOn
/// and content criteria (Body/Text/Subject/…) conservatively return true to
/// avoid missing matching messages without full MIME parsing.
async fn cmd_search(
    state: &AppState,
    tag: Tag<'static>,
    criteria: &[SearchKey<'_>],
    uid: bool,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let rows = sqlx::query(
        "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, uid, flags, received_at \
         FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 ORDER BY received_at ASC",
    )
    .bind(sel.mailbox_id)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    let mut matches: Vec<NonZeroU32> = Vec::new();
    for row in &rows {
        let seq_val: i64 = row.get("seq");
        let uid_val: i64 = row.get("uid");
        let flags: Vec<String> = row.get("flags");
        let recv: Option<ChronoDateTime<Utc>> = row.try_get("received_at").ok();
        if criteria.iter().all(|key| search_key_matches(key, &flags, recv.as_ref())) {
            let n = if uid {
                NonZeroU32::new(uid_val as u32).unwrap_or(NonZeroU32::MIN)
            } else {
                NonZeroU32::new(seq_val as u32).unwrap_or(NonZeroU32::MIN)
            };
            matches.push(n);
        }
    }

    vec![
        Response::Data(Data::Search(matches)),
        ok_tagged(tag, None, "SEARCH completed"),
    ]
}

fn search_key_matches(
    key: &SearchKey<'_>,
    flags: &[String],
    recv: Option<&ChronoDateTime<Utc>>,
) -> bool {
    let has = |f: &str| flags.iter().any(|x| x == f);
    match key {
        SearchKey::All     => true,
        SearchKey::Recent  => false, // \Recent not tracked per-session
        SearchKey::New     => false, // New = Recent + Unseen; \Recent not tracked
        SearchKey::Seen    => has("\\Seen"),
        SearchKey::Unseen  => !has("\\Seen"),
        SearchKey::Flagged   => has("\\Flagged"),
        SearchKey::Unflagged => !has("\\Flagged"),
        SearchKey::Answered   => has("\\Answered"),
        SearchKey::Unanswered => !has("\\Answered"),
        SearchKey::Deleted   => has("\\Deleted"),
        SearchKey::Undeleted => !has("\\Deleted"),
        SearchKey::Draft     => has("\\Draft"),
        SearchKey::Undraft   => !has("\\Draft"),
        // Internal-date criteria: compare against received_at (UTC midnight boundary).
        // SentSince/SentBefore/SentOn require envelope Date parsing — conservative true.
        SearchKey::Since(date) => recv.map_or(true, |r| r.date_naive() >= *date.as_ref()),
        SearchKey::Before(date) => recv.map_or(true, |r| r.date_naive() < *date.as_ref()),
        SearchKey::On(date) => recv.map_or(true, |r| r.date_naive() == *date.as_ref()),
        // Recursive logical operators
        SearchKey::Not(inner) => !search_key_matches(inner.as_ref(), flags, recv),
        SearchKey::Or(a, b)   => {
            search_key_matches(a.as_ref(), flags, recv)
                || search_key_matches(b.as_ref(), flags, recv)
        }
        SearchKey::And(inner) => inner.iter().all(|k| search_key_matches(k, flags, recv)),
        // Content/size/envelope criteria — conservative true (no MIME parsing)
        _ => true,
    }
}

/// APPEND — RFC 3501 §6.3.11 + RFC 4315 (UIDPLUS).
/// Stores a message verbatim into the target mailbox. The raw bytes are
/// written to object store; the DB row is inserted atomically with a
/// `FOR UPDATE` lock on the mailbox row to avoid uid collision under
/// concurrent APPENDs. The trigger `trg_messages_sync_mailbox_stats`
/// updates message_count/unseen_count/next_uid after the INSERT.
async fn cmd_append(
    state: &AppState,
    tag: Tag<'static>,
    mailbox: &ImapMailbox<'_>,
    flags: &[Flag<'_>],
    message: &LiteralOrLiteral8<'_>,
    uid: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let mbox_name = mailbox_to_string(mailbox);

    // Extract raw bytes from either Literal or Literal8 (BINARY extension).
    let raw_bytes: &[u8] = match message {
        LiteralOrLiteral8::Literal(l)  => l.as_ref(),
        LiteralOrLiteral8::Literal8(l) => l.as_ref(),
    };

    let flag_strs: Vec<String> = flags.iter().map(|f| flag_to_str(f).to_owned()).collect();

    // Begin transaction; lock mailbox row to serialise concurrent APPENDs.
    let mut tx = match state.db().begin().await {
        Ok(t) => t,
        Err(_) => return vec![no_tagged(tag, "internal error")],
    };

    let row: Option<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT id, uid_validity, next_uid FROM mailboxes \
         WHERE user_id = $1 AND folder_name = $2 AND tenant_id = $3 FOR UPDATE",
    )
    .bind(uid)
    .bind(&mbox_name)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    let (mailbox_id, uid_validity_raw, new_uid_raw) = match row {
        None => {
            let _ = tx.rollback().await;
            return vec![no_tagged(tag, "mailbox not found")];
        }
        Some(r) => r,
    };

    let msg_id = Uuid::new_v4();
    let body_path = if let Some(store) = state.store() {
        let key = format!("raw/{msg_id}.eml");
        if store.put(&key, raw_bytes.to_vec(), Some("message/rfc822")).await.is_err() {
            let _ = tx.rollback().await;
            return vec![no_tagged(tag, "storage error")];
        }
        format!("s3://{}/{key}", store.bucket())
    } else {
        format!("/tmp/{msg_id}.eml")
    };

    let insert = sqlx::query(
        "INSERT INTO messages \
         (id, mailbox_id, tenant_id, uid, flags, size_bytes, body_path, received_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
    )
    .bind(msg_id)
    .bind(mailbox_id)
    .bind(tenant_id)
    .bind(new_uid_raw)
    .bind(&flag_strs)
    .bind(raw_bytes.len() as i32)
    .bind(&body_path)
    .execute(&mut *tx)
    .await;

    if insert.is_err() {
        let _ = tx.rollback().await;
        return vec![no_tagged(tag, "internal error")];
    }

    if tx.commit().await.is_err() {
        return vec![no_tagged(tag, "internal error")];
    }

    let uid_validity = NonZeroU32::new(uid_validity_raw as u32).unwrap_or(NonZeroU32::MIN);
    let appended_uid = NonZeroU32::new(new_uid_raw as u32).unwrap_or(NonZeroU32::MIN);

    vec![ok_tagged(
        tag,
        Some(Code::AppendUid { uid_validity, uid: appended_uid }),
        "APPEND completed",
    )]
}

async fn cmd_fetch(
    state: &AppState,
    tag: Tag<'static>,
    sequence_set: &imap_codec::imap_types::sequence::SequenceSet,
    macro_or: &MacroOrMessageDataItemNames<'_>,
    uid: bool,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    // UID FETCH: sequence_set holds UID values; * resolves to u32::MAX (all).
    // Seq FETCH: sequence_set holds ordinal positions (1-based); * = exists.
    let rows = if uid {
        let (start, end) = sequence_range(sequence_set, u32::MAX);
        sqlx::query(
            "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, \
             uid, subject, from_addr, from_name, date, flags, size_bytes, received_at, body_path \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND uid >= $3 AND uid <= $4 \
             ORDER BY received_at ASC",
        )
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .bind(start as i64)
        .bind(end as i64)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    } else {
        let (start, end) = sequence_range(sequence_set, sel.exists);
        sqlx::query(
            "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, \
             uid, subject, from_addr, from_name, date, flags, size_bytes, received_at, body_path \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $4 \
             ORDER BY received_at ASC OFFSET $2 LIMIT $3",
        )
        .bind(sel.mailbox_id)
        .bind((start - 1) as i64)
        .bind((end - start + 1) as i64)
        .bind(tenant_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    };

    let w_flags        = wants(macro_or, "FLAGS");
    let w_envelope     = wants(macro_or, "ENVELOPE");
    let w_size         = wants(macro_or, "RFC822.SIZE");
    let w_uid          = wants(macro_or, "UID");
    let w_internaldate = wants(macro_or, "INTERNALDATE");
    let w_body         = wants(macro_or, "BODYEXT");

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
        if w_body {
            // BODY[] — busca o .eml completo do body_path e retorna como
            // BODY[]{size}\r\n...bytes... via NString(Literal). section=None
            // significa BODY[] (mensagem inteira); origin=None = sem partial.
            let body_path: Option<String> = row.try_get("body_path").ok();
            if let Some(path) = body_path {
                let raw = fetch_body_bytes(state, &path).await;
                if let Some(bytes) = raw {
                    let data = NString::from(Literal::unvalidated(bytes));
                    items.push(MessageDataItem::BodyExt {
                        section: None,
                        origin:  None,
                        data,
                    });
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
    uid: bool,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let flag_strs: Vec<String> = flags.iter().map(|f| flag_to_str(f).to_owned()).collect();

    if uid {
        // UID STORE: sequence_set holds UID values. Filter by uid column directly;
        // no CTE needed since UIDs are stable identifiers, not positional.
        // * resolves to u32::MAX to cover all possible UIDs.
        let (start, end) = sequence_range(sequence_set, u32::MAX);
        let sql = match kind {
            imap_codec::imap_types::flag::StoreType::Add => {
                "UPDATE messages SET flags = array_cat(flags, $1::text[]) \
                 WHERE mailbox_id = $2 AND tenant_id = $5 AND uid >= $3 AND uid <= $4"
            }
            imap_codec::imap_types::flag::StoreType::Remove => {
                "UPDATE messages \
                 SET flags = (SELECT array_agg(e) FROM unnest(flags) e WHERE NOT e = ANY($1::text[])) \
                 WHERE mailbox_id = $2 AND tenant_id = $5 AND uid >= $3 AND uid <= $4"
            }
            imap_codec::imap_types::flag::StoreType::Replace => {
                "UPDATE messages SET flags = $1::text[] \
                 WHERE mailbox_id = $2 AND tenant_id = $5 AND uid >= $3 AND uid <= $4"
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
    } else {
        // STORE: sequence_set holds ordinal positions (1-based). CTE maps seq →
        // row so the UPDATE targets correct rows despite uid gaps from deletes.
        let (start, end) = sequence_range(sequence_set, sel.exists);
        let sql = match kind {
            imap_codec::imap_types::flag::StoreType::Add => {
                "WITH ordered AS (\
                   SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
                   FROM messages WHERE mailbox_id = $2 AND tenant_id = $5\
                 ) \
                 UPDATE messages SET flags = array_cat(flags, $1::text[]) \
                 WHERE id IN (SELECT id FROM ordered WHERE seq >= $3 AND seq <= $4)"
            }
            imap_codec::imap_types::flag::StoreType::Remove => {
                "WITH ordered AS (\
                   SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
                   FROM messages WHERE mailbox_id = $2 AND tenant_id = $5\
                 ) \
                 UPDATE messages \
                 SET flags = (SELECT array_agg(e) FROM unnest(flags) e WHERE NOT e = ANY($1::text[])) \
                 WHERE id IN (SELECT id FROM ordered WHERE seq >= $3 AND seq <= $4)"
            }
            imap_codec::imap_types::flag::StoreType::Replace => {
                "WITH ordered AS (\
                   SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
                   FROM messages WHERE mailbox_id = $2 AND tenant_id = $5\
                 ) \
                 UPDATE messages SET flags = $1::text[] \
                 WHERE id IN (SELECT id FROM ordered WHERE seq >= $3 AND seq <= $4)"
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
    }

    vec![ok_tagged(tag, None, "STORE completed")]
}

/// COPY / UID COPY — RFC 3501 §6.4.7 + RFC 4315 (UIDPLUS COPYUID).
/// Duplicates messages from the selected mailbox into a destination folder,
/// sharing the original body_path (S3 objects are immutable; no S3 copy needed).
/// Returns COPYUID so UID-PUSH clients know the destination UIDs immediately.
async fn cmd_copy(
    state: &AppState,
    tag: Tag<'static>,
    sequence_set: &imap_codec::imap_types::sequence::SequenceSet,
    dst_mailbox: &ImapMailbox<'_>,
    uid: bool,
    sel: &SelectedMailbox,
    user_id: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let dst_name = mailbox_to_string(dst_mailbox);

    // Query source messages — uid mode filters by UID range; seq mode uses
    // ROW_NUMBER() CTE to map ordinal positions to rows.
    let src_rows = if uid {
        let (start, end) = sequence_range(sequence_set, u32::MAX);
        sqlx::query(
            "SELECT uid, flags, size_bytes, body_path \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 \
             AND uid >= $3 AND uid <= $4 ORDER BY uid ASC",
        )
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .bind(start as i64)
        .bind(end as i64)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    } else {
        let (start, end) = sequence_range(sequence_set, sel.exists);
        sqlx::query(
            "SELECT uid, flags, size_bytes, body_path FROM (\
               SELECT uid, flags, size_bytes, body_path, \
                      ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
               FROM messages WHERE mailbox_id = $1 AND tenant_id = $4\
             ) AS ordered WHERE seq >= $2 AND seq <= $3 ORDER BY seq ASC",
        )
        .bind(sel.mailbox_id)
        .bind(start as i64)
        .bind(end as i64)
        .bind(tenant_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    };

    if src_rows.is_empty() {
        return vec![ok_tagged(tag, None, "COPY completed")];
    }

    // Lock destination mailbox to serialise concurrent COPY/APPEND on same target.
    let mut tx = match state.db().begin().await {
        Ok(t) => t,
        Err(_) => return vec![no_tagged(tag, "internal error")],
    };

    let dst_row: Option<(Uuid, i64, i64)> = sqlx::query_as(
        "SELECT id, uid_validity, next_uid FROM mailboxes \
         WHERE user_id = $1 AND folder_name = $2 AND tenant_id = $3 FOR UPDATE",
    )
    .bind(user_id)
    .bind(&dst_name)
    .bind(tenant_id)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    let (dst_mailbox_id, dst_uid_validity_raw, initial_next_uid) = match dst_row {
        None => {
            let _ = tx.rollback().await;
            return vec![no_tagged(tag, "mailbox not found")];
        }
        Some(r) => r,
    };

    let mut src_uids: Vec<NonZeroU32> = Vec::with_capacity(src_rows.len());
    let mut dst_uids: Vec<NonZeroU32> = Vec::with_capacity(src_rows.len());

    for (i, row) in src_rows.iter().enumerate() {
        let src_uid_val: i64 = row.get("uid");
        let dst_uid_val = initial_next_uid + i as i64;
        let flags: Vec<String> = row.get("flags");
        let size_bytes: i32 = row.get("size_bytes");
        let body_path: String = row.get("body_path");

        if sqlx::query(
            "INSERT INTO messages (id, mailbox_id, tenant_id, uid, flags, size_bytes, body_path, received_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, now())",
        )
        .bind(Uuid::new_v4())
        .bind(dst_mailbox_id)
        .bind(tenant_id)
        .bind(dst_uid_val)
        .bind(&flags)
        .bind(size_bytes)
        .bind(&body_path)
        .execute(&mut *tx)
        .await
        .is_err()
        {
            let _ = tx.rollback().await;
            return vec![no_tagged(tag, "internal error")];
        }

        src_uids.push(NonZeroU32::new(src_uid_val as u32).unwrap_or(NonZeroU32::MIN));
        dst_uids.push(NonZeroU32::new(dst_uid_val as u32).unwrap_or(NonZeroU32::MIN));
    }

    if tx.commit().await.is_err() {
        return vec![no_tagged(tag, "internal error")];
    }

    let dst_uid_validity = NonZeroU32::new(dst_uid_validity_raw as u32).unwrap_or(NonZeroU32::MIN);
    vec![ok_tagged(
        tag,
        Some(Code::CopyUid {
            uid_validity: dst_uid_validity,
            source: build_uid_set(src_uids),
            destination: build_uid_set(dst_uids),
        }),
        "COPY completed",
    )]
}

fn build_uid_set(uids: Vec<NonZeroU32>) -> UidSet {
    let elements: Vec<UidElement> = uids.into_iter().map(UidElement::Single).collect();
    UidSet(Vec1::try_from(elements).expect("non-empty uid set"))
}

/// EXPUNGE — RFC 3501 §6.4.3.
/// For each \Deleted message, emits `* N EXPUNGE` where N is the
/// current sequence number of that message (accounting for preceding
/// EXPUNGEs in the same command which shift later numbers down by 1).
/// After all untagged responses the messages are physically deleted.
async fn cmd_expunge(
    state: &AppState,
    tag: Tag<'static>,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    // Fetch seq positions of \Deleted messages (within the full mailbox).
    let seq_rows = sqlx::query(
        "SELECT seq FROM (\
           SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, flags \
           FROM messages WHERE mailbox_id = $1 AND tenant_id = $2\
         ) sub \
         WHERE '\\Deleted' = ANY(flags) ORDER BY seq ASC",
    )
    .bind(sel.mailbox_id)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    let _ = sqlx::query(
        "DELETE FROM messages \
         WHERE mailbox_id = $1 AND tenant_id = $2 AND '\\Deleted' = ANY(flags)",
    )
    .bind(sel.mailbox_id)
    .bind(tenant_id)
    .execute(state.db())
    .await;

    // RFC 3501 §7.4.1: each expunge shifts subsequent sequence numbers down by 1.
    // The i-th deleted message (0-indexed) had original seq S; by the time
    // that response is sent, i earlier messages are already gone, so emit S-i.
    let mut out: Vec<Response<'static>> = Vec::with_capacity(seq_rows.len() + 1);
    for (i, row) in seq_rows.iter().enumerate() {
        let orig: i64 = row.get("seq");
        let adj = (orig as usize).saturating_sub(i);
        if let Some(n) = NonZeroU32::new(adj as u32) {
            out.push(Response::Data(Data::Expunge(n)));
        }
    }
    out.push(ok_tagged(tag, None, "EXPUNGE completed"));
    out
}

/// UID EXPUNGE — RFC 4315 §4.4: expunge only the \Deleted messages whose
/// UID falls within the given set. Required when UIDPLUS is advertised.
/// Emits `* N EXPUNGE` per message as RFC 3501 §7.4.1 requires.
async fn cmd_expunge_uid(
    state: &AppState,
    tag: Tag<'static>,
    sequence_set: &imap_codec::imap_types::sequence::SequenceSet,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let (start, end) = sequence_range(sequence_set, u32::MAX);

    let seq_rows = sqlx::query(
        "SELECT seq FROM (\
           SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, uid, flags \
           FROM messages WHERE mailbox_id = $1 AND tenant_id = $2\
         ) sub \
         WHERE '\\Deleted' = ANY(flags) AND uid >= $3 AND uid <= $4 ORDER BY seq ASC",
    )
    .bind(sel.mailbox_id)
    .bind(tenant_id)
    .bind(start as i64)
    .bind(end as i64)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    let _ = sqlx::query(
        "DELETE FROM messages \
         WHERE mailbox_id = $1 AND tenant_id = $2 \
           AND '\\Deleted' = ANY(flags) \
           AND uid >= $3 AND uid <= $4",
    )
    .bind(sel.mailbox_id)
    .bind(tenant_id)
    .bind(start as i64)
    .bind(end as i64)
    .execute(state.db())
    .await;

    let mut out: Vec<Response<'static>> = Vec::with_capacity(seq_rows.len() + 1);
    for (i, row) in seq_rows.iter().enumerate() {
        let orig: i64 = row.get("seq");
        let adj = (orig as usize).saturating_sub(i);
        if let Some(n) = NonZeroU32::new(adj as u32) {
            out.push(Response::Data(Data::Expunge(n)));
        }
    }
    out.push(ok_tagged(tag, None, "UID EXPUNGE completed"));
    out
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

/// CLOSE — RFC 3501 §6.4.2: silently expunge `\Deleted` messages and return
/// to authenticated state. No untagged EXPUNGE responses are sent. When the
/// mailbox was opened read-only via EXAMINE, the spec explicitly requires that
/// no messages be removed — the DELETE is skipped in that case.
async fn cmd_close(
    state: &AppState,
    tag: Tag<'static>,
    sess: &mut SessionState,
    selected: &mut Option<SelectedMailbox>,
    tenant_id: Option<Uuid>,
) -> Vec<Response<'static>> {
    if let (Some(sel), Some(tid)) = (selected.as_ref(), tenant_id) {
        if !sel.read_only {
            let _ = sqlx::query(
                "DELETE FROM messages \
                 WHERE mailbox_id = $1 AND tenant_id = $2 AND '\\Deleted' = ANY(flags)",
            )
            .bind(sel.mailbox_id)
            .bind(tid)
            .execute(state.db())
            .await;
        }
    }
    *selected = None;
    if *sess == SessionState::Selected {
        *sess = SessionState::Authenticated;
    }
    vec![ok_tagged(tag, None, "CLOSE completed")]
}

/// IDLE RFC 2177: envia "+ idling", espera DONE do cliente.
/// Enquanto aguarda, a cada 28s verifica se message_count mudou e envia
/// * N EXISTS se houver novos emails — elimina o polling NOOP do cliente.
/// DONE do cliente pode chegar em qualquer burst de leitura; aceita como
/// prefixo case-insensitive (clientes enviam "DONE\r\n").
async fn handle_idle(
    tag:       Tag<'static>,
    reader:    &mut tokio::net::tcp::OwnedReadHalf,
    writer:    &mut tokio::net::tcp::OwnedWriteHalf,
    resp_codec: &ResponseCodec,
    state:     &AppState,
    selected:  &mut Option<SelectedMailbox>,
    tenant_id: Option<Uuid>,
) -> anyhow::Result<()> {
    writer.write_all(b"+ idling\r\n").await?;

    let mut ibuf = [0u8; 32];
    loop {
        tokio::select! {
            result = reader.read(&mut ibuf) => {
                let n = result?;
                if n == 0 { return Ok(()); }
                // DONE pode chegar sozinho ou colado com o próximo comando.
                // Basta detectar "done" no início do buffer recebido.
                let chunk = std::str::from_utf8(&ibuf[..n]).unwrap_or("").trim();
                if chunk.to_ascii_uppercase().starts_with("DONE") {
                    let ok = ok_tagged(tag, None, "IDLE done");
                    writer.write_all(&resp_codec.encode(&ok).dump()).await?;
                    return Ok(());
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(28)) => {
                if let (Some(sel), Some(tid)) = (selected.as_mut(), tenant_id) {
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
                            let resp = Response::Data(Data::Exists(new_exists));
                            writer.write_all(&resp_codec.encode(&resp).dump()).await?;
                        }
                    }
                }
            }
        }
    }
}

/// Busca os bytes do corpo da mensagem a partir do body_path armazenado no DB.
/// - `s3://bucket/key` → lê via ObjectStore (MinIO/S3)
/// - `/path/to/file`   → lê via sistema de arquivos (dev/fallback)
/// Retorna None silenciosamente se o store não estiver configurado ou a leitura falhar.
async fn fetch_body_bytes(state: &AppState, body_path: &str) -> Option<Vec<u8>> {
    if let Some(idx) = body_path.strip_prefix("s3://").and_then(|s| s.find('/').map(|i| "s3://".len() + i + 1)) {
        let key = &body_path[idx..];
        state.store()?.get(key).await.ok()
    } else if body_path.starts_with('/') {
        tokio::fs::read(body_path).await.ok()
    } else {
        None
    }
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
    use imap_codec::imap_types::fetch::{Macro, MessageDataItemName};
    match macro_or {
        MacroOrMessageDataItemNames::Macro(m) => match m {
            Macro::All  => matches!(name, "FLAGS" | "ENVELOPE" | "RFC822.SIZE" | "INTERNALDATE"),
            Macro::Fast => matches!(name, "FLAGS" | "RFC822.SIZE" | "INTERNALDATE"),
            Macro::Full => matches!(name, "FLAGS" | "ENVELOPE" | "RFC822.SIZE" | "INTERNALDATE" | "BODY"),
            _ => false,
        },
        // Pattern matching explícito por variante — o Debug repr anterior
        // produzia "Rfc822Size" (sem ponto) para RFC822.SIZE, quebrando a
        // detecção quando o cliente listava itens explicitamente.
        MacroOrMessageDataItemNames::MessageDataItemNames(items) => {
            items.iter().any(|item| matches!(
                (name, item),
                ("FLAGS",        MessageDataItemName::Flags)
                | ("ENVELOPE",    MessageDataItemName::Envelope)
                | ("RFC822.SIZE", MessageDataItemName::Rfc822Size)
                | ("UID",         MessageDataItemName::Uid)
                | ("INTERNALDATE",MessageDataItemName::InternalDate)
                | ("BODYEXT",     MessageDataItemName::BodyExt { .. })
                | ("BODY",        MessageDataItemName::BodyStructure)
            ))
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

//! IMAP session state machine — one per TCP connection.
//! Handles core IMAP4rev1 commands: CAPABILITY, LOGIN, LIST, SELECT,
//! FETCH, STORE, EXPUNGE, CLOSE, LOGOUT, NOOP.
//! Extensions: UIDPLUS, IDLE, UNSELECT, MOVE, LITERAL+ (RFC 7888).
//!
//! Tenant scoping: após LOGIN, `tenant_id` é propagado para todo handler
//! subsequente e cada query aplica `AND tenant_id = $` explícito. Sem isso,
//! um mailbox_id ou user_id vazado (via misconfig/log/debug endpoint) daria
//! acesso cross-tenant — a RLS de `mailboxes`/`messages` é NULL-bypass e
//! não bloqueia operações IMAP que rodam fora de `begin_tenant_tx`.

use std::collections::HashMap;
use std::num::NonZeroU32;

use imap_codec::{
    CommandCodec, GreetingCodec, ResponseCodec,
    decode::{CommandDecodeError, Decoder},
    encode::Encoder,
    imap_types::{
        auth::AuthMechanism,
        body::{BasicFields, Body, BodyStructure, SpecificFields},
        command::{Command, CommandBody},
        core::{Atom, AString, IString, Literal, NString, Tag, Text, Vec1},
        fetch::{MacroOrMessageDataItemNames, MessageDataItem, MessageDataItemName, Section},
        flag::{Flag, FlagFetch, FlagNameAttribute, FlagPerm, StoreResponse},
        mailbox::Mailbox as ImapMailbox,
        response::{
            Bye, Capability, CapabilityOther, Code, Data, Greeting, Response, Status, StatusBody,
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
    /// seq (1-based) → flags as stored in DB; used by NOOP to detect flag
    /// changes made by other sessions and push unsolicited FETCH responses.
    flags_snapshot: HashMap<u32, Vec<String>>,
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

                    // AUTHENTICATE PLAIN (RFC 4616 + RFC 3501 §6.2.2) — tratado inline
                    // pois pode precisar de um round-trip extra para leitura de dados
                    // quando o cliente não usa SASL-IR (initial_response == None).
                    if let CommandBody::Authenticate { mechanism, initial_response } = &cmd.body {
                        if *sess != SessionState::NotAuthenticated {
                            let resp = no_tagged(cmd.tag.clone(), "already authenticated");
                            writer.write_all(&resp_codec.encode(&resp).dump()).await?;
                            IMAP_COMMANDS_TOTAL.with_label_values(&["AUTHENTICATE", "no"]).inc();
                        } else if !matches!(mechanism, AuthMechanism::Plain) {
                            let resp = no_tagged(cmd.tag.clone(), "unsupported mechanism");
                            writer.write_all(&resp_codec.encode(&resp).dump()).await?;
                            IMAP_COMMANDS_TOTAL.with_label_values(&["AUTHENTICATE", "no"]).inc();
                        } else {
                            // Try to get PLAIN blob: prefer SASL-IR (initial_response),
                            // fall back to a challenge round-trip.
                            let plain_bytes: Option<Vec<u8>> = if let Some(ir) = initial_response {
                                Some(ir.declassify().to_vec())
                            } else {
                                // Send challenge line; client replies with base64-encoded blob.
                                writer.write_all(b"+ \r\n").await?;
                                let mut line_buf = Vec::with_capacity(512);
                                let mut tmp2 = [0u8; 512];
                                'auth_read: loop {
                                    let n = reader.read(&mut tmp2).await?;
                                    if n == 0 { break 'auth_read; }
                                    line_buf.extend_from_slice(&tmp2[..n]);
                                    if line_buf.contains(&b'\n') { break 'auth_read; }
                                }
                                let line = String::from_utf8_lossy(&line_buf);
                                let trimmed = line.trim();
                                // Client may send "*" to cancel AUTHENTICATE.
                                if trimmed == "*" {
                                    let resp = bad_tagged(cmd.tag.clone(), "AUTHENTICATE cancelled");
                                    writer.write_all(&resp_codec.encode(&resp).dump()).await?;
                                    IMAP_COMMANDS_TOTAL.with_label_values(&["AUTHENTICATE", "bad"]).inc();
                                    buf.clear();
                                    break 'decode;
                                }
                                let decoded: Option<Vec<u8>> = {
                                    use base64::Engine as _;
                                    base64::engine::general_purpose::STANDARD.decode(trimmed.as_bytes()).ok()
                                };
                                decoded
                            };

                            let outcome = handle_authenticate_plain(
                                &state, cmd.tag.clone(), plain_bytes.as_deref(),
                                &mut sess, &mut user_id, &mut tenant_id,
                            ).await;
                            let outcome_label = outcome_of(&outcome);
                            IMAP_COMMANDS_TOTAL.with_label_values(&["AUTHENTICATE", outcome_label]).inc();
                            for resp in &outcome {
                                writer.write_all(&resp_codec.encode(resp).dump()).await?;
                            }
                        }
                        buf.clear();
                        break 'decode;
                    }

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
                Err(CommandDecodeError::LiteralFound { length, mode, .. }) => {
                    // RFC 7888 (LITERAL+): non-synchronizing literals skip the
                    // continuation request — the client sends data immediately.
                    // Sync literals require the server to grant permission first.
                    use imap_codec::imap_types::core::LiteralMode;
                    if !matches!(mode, LiteralMode::NonSync) {
                        let cont = format!("+ Ready for {} bytes\r\n", length);
                        writer.write_all(cont.as_bytes()).await?;
                    }
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
            response,
            flags,
            uid,
            ..
        } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_store(state, tag, sequence_set, kind, response, flags, *uid, selected.as_ref().unwrap(), tenant_id.unwrap()).await
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
        CommandBody::Move { sequence_set, mailbox, uid, .. } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_move(state, tag, sequence_set, mailbox, *uid, selected.as_mut().unwrap(), user_id.unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::ExpungeUid { sequence_set } => {
            if selected.is_none() {
                return vec![no_tagged(tag, "no mailbox selected")];
            }
            cmd_expunge_uid(state, tag, sequence_set, selected.as_ref().unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Unselect => {
            cmd_unselect(tag, sess, selected)
        }
        CommandBody::Create { mailbox } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_create(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Delete { mailbox } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_delete(state, tag, mailbox, user_id.unwrap(), tenant_id.unwrap()).await
        }
        CommandBody::Rename { from, to } => {
            if *sess == SessionState::NotAuthenticated {
                return vec![no_tagged(tag, "not authenticated")];
            }
            cmd_rename(state, tag, from, to, user_id.unwrap(), tenant_id.unwrap()).await
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
        Capability::Auth(AuthMechanism::Plain),
        Capability::Idle,
        Capability::UidPlus,
        Capability::Other(CapabilityOther(Atom::try_from("UNSELECT").unwrap())),
        Capability::Other(CapabilityOther(Atom::try_from("MOVE").unwrap())),
        Capability::Other(CapabilityOther(Atom::try_from("LITERAL+").unwrap())),
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

/// AUTHENTICATE PLAIN helper — RFC 4616 §2.
/// Decodes `\0authzid\0authcid\0passwd` blob (authzid may be empty).
/// Uses the same DB query and lockout as `cmd_login`.
async fn handle_authenticate_plain(
    state: &AppState,
    tag: Tag<'static>,
    plain_bytes: Option<&[u8]>,
    sess: &mut SessionState,
    user_id: &mut Option<Uuid>,
    tenant_id: &mut Option<Uuid>,
) -> Vec<Response<'static>> {
    let bytes = match plain_bytes {
        None => return vec![no_tagged(tag, "AUTHENTICATE failed")],
        Some(b) => b,
    };

    // RFC 4616 §2: message = [authzid] NUL authcid NUL passwd
    let parts: Vec<&[u8]> = bytes.splitn(3, |&b| b == 0).collect();
    if parts.len() != 3 {
        return vec![no_tagged(tag, "AUTHENTICATE failed")];
    }
    let user = match std::str::from_utf8(parts[1]) {
        Ok(s) if !s.is_empty() => s,
        _ => return vec![no_tagged(tag, "AUTHENTICATE failed")],
    };
    let pass = match std::str::from_utf8(parts[2]) {
        Ok(s) => s,
        _ => return vec![no_tagged(tag, "AUTHENTICATE failed")],
    };

    let lock = login_lockout();
    if lock.is_locked_out(user) {
        IMAP_LOGINS_TOTAL.with_label_values(&["locked_out"]).inc();
        warn!(user = %user, "IMAP AUTHENTICATE refused (locked out)");
        return vec![no_tagged(tag, "AUTHENTICATE failed")];
    }

    let row: Option<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, tenant_id FROM users \
         WHERE lower(email) = lower($1) AND password_hash = crypt($2, password_hash) LIMIT 1",
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
            vec![ok_tagged(tag, None, "AUTHENTICATE completed")]
        }
        None => {
            lock.record_failure(user);
            IMAP_LOGINS_TOTAL.with_label_values(&["failure"]).inc();
            warn!(user = %user, "IMAP AUTHENTICATE failed");
            vec![no_tagged(tag, "AUTHENTICATE failed")]
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

            // Build initial flags snapshot so NOOP can diff against it.
            let flags_snapshot: HashMap<u32, Vec<String>> = sqlx::query(
                "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, flags \
                 FROM messages WHERE mailbox_id = $1 AND tenant_id = $2",
            )
            .bind(mailbox_id)
            .bind(tenant_id)
            .fetch_all(state.db())
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let s: i64 = r.get("seq");
                let f: Vec<String> = r.get("flags");
                (s as u32, f)
            })
            .collect();

            *sess = SessionState::Selected;
            *selected = Some(SelectedMailbox { mailbox_id, exists, read_only, flags_snapshot });

            // RFC 3501 §7.3.1: FLAGS, EXISTS, RECENT, UIDVALIDITY, UIDNEXT e
            // PERMANENTFLAGS são todos obrigatórios/SHOULD na resposta SELECT.
            let mut out = vec![
                Response::Data(Data::Flags(vec![
                    Flag::Seen,
                    Flag::Answered,
                    Flag::Flagged,
                    Flag::Deleted,
                    Flag::Draft,
                    Flag::Recent, // RFC 3501 §2.3.2: server-managed system flag
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
/// `received_at` (RFC 3501 §2.3.3 internal date). Subject/From use the DB
/// columns (ILIKE). Text/Body/Header also match against subject+from_addr.
/// SentSince/SentBefore/SentOn compare against the `date` DB column (envelope Date header).
async fn cmd_search(
    state: &AppState,
    tag: Tag<'static>,
    criteria: &[SearchKey<'_>],
    uid: bool,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let rows = sqlx::query(
        "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, uid, flags, \
         received_at, date, size_bytes, subject, from_addr \
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
        let sent: Option<ChronoDateTime<Utc>> = row.try_get("date").ok().flatten();
        let size: Option<i32> = row.try_get("size_bytes").ok();
        let subject: Option<String> = row.try_get("subject").ok().flatten();
        let from_addr: Option<String> = row.try_get("from_addr").ok().flatten();
        if criteria.iter().all(|key| search_key_matches(
            key, &flags, recv.as_ref(), sent.as_ref(), size, subject.as_deref(), from_addr.as_deref(),
            seq_val as u32, uid_val as u32, sel.exists,
        )) {
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
    sent: Option<&ChronoDateTime<Utc>>,
    size: Option<i32>,
    subject: Option<&str>,
    from_addr: Option<&str>,
    seq: u32,
    msg_uid: u32,
    exists: u32,
) -> bool {
    let has = |f: &str| flags.iter().any(|x| x == f);
    // Case-insensitive substring check — mirrors RFC 3501 §6.4.4 ILIKE semantics.
    let icontains = |haystack: Option<&str>, needle: &str| -> bool {
        haystack.map_or(true, |h| h.to_ascii_lowercase().contains(&needle.to_ascii_lowercase()))
    };
    // Check if a value falls within any of the (start, end) ranges derived from a SequenceSet.
    let in_ranges = |ranges: &[(u32, u32)], val: u32| -> bool {
        ranges.iter().any(|&(s, e)| val >= s && val <= e)
    };
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
        SearchKey::Since(date) => recv.map_or(true, |r| r.date_naive() >= *date.as_ref()),
        SearchKey::Before(date) => recv.map_or(true, |r| r.date_naive() < *date.as_ref()),
        SearchKey::On(date) => recv.map_or(true, |r| r.date_naive() == *date.as_ref()),
        // SentSince/SentBefore/SentOn compare against the envelope Date header stored in DB.
        // Conservative true when Date header is absent (NULL) — no false negatives.
        SearchKey::SentSince(date)  => sent.map_or(true, |s| s.date_naive() >= *date.as_ref()),
        SearchKey::SentBefore(date) => sent.map_or(true, |s| s.date_naive() < *date.as_ref()),
        SearchKey::SentOn(date)     => sent.map_or(true, |s| s.date_naive() == *date.as_ref()),
        // Size criteria: size_bytes comes from DB (set by APPEND/ingest).
        // Conservative true when size_bytes is NULL (e.g. old messages before APPEND sprint).
        SearchKey::Larger(n)  => size.map_or(true, |s| (s as u64) > (*n as u64)),
        SearchKey::Smaller(n) => size.map_or(true, |s| (s as u64) < (*n as u64)),
        // Envelope criteria — matched against DB columns (ILIKE).
        // Subject uses subject column; From matches from_addr.
        // CC/BCC/To/ReplyTo are not stored — conservative true.
        SearchKey::Subject(pat) => {
            let p = astring_to_string(pat);
            icontains(subject, &p)
        }
        SearchKey::From(pat) => {
            let p = astring_to_string(pat);
            icontains(from_addr, &p)
        }
        // Header field matching: support Subject: and From: header names.
        // Other header fields: conservative true (not stored in DB).
        SearchKey::Header(field, value) => {
            let field_lower = astring_to_string(field).to_ascii_lowercase();
            let val_str = astring_to_string(value);
            match field_lower.as_str() {
                "subject" => icontains(subject, &val_str),
                "from"    => icontains(from_addr, &val_str),
                _         => true,
            }
        }
        // TEXT / BODY — full body search requires fetching raw bytes from object store:
        // too expensive per-message in a hot path; conservative true (no false negatives).
        SearchKey::Text(_) | SearchKey::Body(_) => true,
        // Keyword / Unkeyword — match against the flags array using flag_to_str.
        SearchKey::Keyword(kw) => has(flag_to_str(kw)),
        SearchKey::Unkeyword(kw) => !has(flag_to_str(kw)),
        // SequenceSet — RFC 3501 §6.4.4: match by seq number (* resolves to exists).
        SearchKey::SequenceSet(seq_set) => {
            let ranges = sequence_ranges(seq_set, exists);
            in_ranges(&ranges, seq)
        }
        // UID — match by UID (* resolves to u32::MAX in UID mode).
        SearchKey::Uid(uid_set) => {
            let ranges = sequence_ranges(uid_set, u32::MAX);
            in_ranges(&ranges, msg_uid)
        }
        // Recursive logical operators
        SearchKey::Not(inner) => !search_key_matches(inner.as_ref(), flags, recv, sent, size, subject, from_addr, seq, msg_uid, exists),
        SearchKey::Or(a, b)   => {
            search_key_matches(a.as_ref(), flags, recv, sent, size, subject, from_addr, seq, msg_uid, exists)
                || search_key_matches(b.as_ref(), flags, recv, sent, size, subject, from_addr, seq, msg_uid, exists)
        }
        SearchKey::And(inner) => inner.iter().all(|k| search_key_matches(k, flags, recv, sent, size, subject, from_addr, seq, msg_uid, exists)),
        // Remaining criteria (Cc, Bcc, To) —
        // conservative true: not missing matches is safer than false positives for clients.
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
        let uid_w = uid_clause(&sequence_ranges(sequence_set, u32::MAX));
        sqlx::query(&format!(
            "SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, \
             uid, subject, from_addr, from_name, date, flags, size_bytes, received_at, body_path, \
             to_addrs, cc_addrs, message_id, in_reply_to, reply_to \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND ({uid_w}) \
             ORDER BY received_at ASC",
        ))
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    } else {
        let seq_w = seq_clause(&sequence_ranges(sequence_set, sel.exists));
        sqlx::query(&format!(
            "SELECT id, seq, uid, subject, from_addr, from_name, date, flags, size_bytes, received_at, body_path, \
             to_addrs, cc_addrs, message_id, in_reply_to, reply_to \
             FROM ( \
               SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, \
                      uid, subject, from_addr, from_name, date, flags, size_bytes, received_at, body_path, \
                      to_addrs, cc_addrs, message_id, in_reply_to, reply_to \
               FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 \
             ) sub WHERE ({seq_w}) \
             ORDER BY seq ASC",
        ))
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    };

    let w_flags        = wants(macro_or, "FLAGS");
    let w_envelope     = wants(macro_or, "ENVELOPE");
    let w_size         = wants(macro_or, "RFC822.SIZE");
    // RFC 3501 §6.4.8 + RFC 4315 §3: UID MUST be in every UID FETCH response,
    // even when the client didn't explicitly request it.
    let w_uid            = uid || wants(macro_or, "UID");
    let w_internaldate   = wants(macro_or, "INTERNALDATE");
    // BODYSTRUCTURE — RFC 3501 §7.4.2: extensible body structure.
    // Macro::Full includes BODY (non-extensible). We serve a minimal single-part
    // TEXT/PLAIN structure for both BODYSTRUCTURE and BODY macros. Clients
    // use this for preview/thread-pane rendering without downloading the body.
    let w_bodystructure  = wants(macro_or, "BODYSTRUCTURE") || wants(macro_or, "BODY");

    // RFC822 / RFC822.HEADER / RFC822.TEXT aliases (RFC 3501 §6.4.5).
    // RFC822       ≡ BODY[] (full message, sets \Seen implicitly)
    // RFC822.HEADER ≡ BODY.PEEK[HEADER] (header only, does NOT set \Seen)
    // RFC822.TEXT   ≡ BODY[TEXT] (body text, sets \Seen implicitly)
    let w_rfc822        = wants(macro_or, "RFC822");
    let w_rfc822_header = wants(macro_or, "RFC822.HEADER");
    let w_rfc822_text   = wants(macro_or, "RFC822.TEXT");

    // Determine which body sections the client wants.
    // BODY[HEADER] / BODY.PEEK[HEADER] → want_header
    // BODY[TEXT]   / BODY.PEEK[TEXT]   → want_text
    // BODY[]       / BODY.PEEK[]       → want_full_body
    // BODY[HEADER.FIELDS (...)] → header_fields_reqs (field names + is_not flag)
    // BODY[]<offset.count>     → full_body_partials (offset, count)
    // Other section specs (BODY[1], BODY[1.HEADER], …) → want_full_body (conservative)
    let (mut want_full_body, mut want_header, mut want_text, mut set_seen) = (
        w_rfc822,
        w_rfc822_header,
        w_rfc822_text,
        w_rfc822 || w_rfc822_text,
    );
    // Each entry: (field_names_lowercase, is_not)
    let mut header_fields_reqs: Vec<(Vec<String>, bool)> = Vec::new();
    // Partial reqs: (section_tag, offset, count)
    // section_tag: 0=full, 1=header, 2=text
    let mut partial_reqs: Vec<(u8, u32, u32)> = Vec::new();
    if let MacroOrMessageDataItemNames::MessageDataItemNames(names) = macro_or {
        for name in names.iter() {
            if let MessageDataItemName::BodyExt { section, peek, partial, .. } = name {
                // Non-peek body fetch implicitly sets \Seen (RFC 3501 §6.4.5).
                if !peek { set_seen = true; }
                // Partial fetch: BODY[<section>]<offset.count>
                if let Some((offset, count)) = partial {
                    let tag = match section {
                        Some(Section::Header(_)) => 1u8,
                        Some(Section::Text(_))   => 2u8,
                        _                        => 0u8,
                    };
                    partial_reqs.push((tag, *offset, *count));
                    continue;
                }
                match section {
                    None                      => want_full_body = true,
                    Some(Section::Header(_))  => want_header = true,
                    Some(Section::Text(_))    => want_text = true,
                    Some(Section::HeaderFields(_, fields)) => {
                        let names_lc: Vec<String> = fields.iter()
                            .map(|f| astring_to_string(f).to_ascii_lowercase())
                            .collect();
                        header_fields_reqs.push((names_lc, false));
                    }
                    Some(Section::HeaderFieldsNot(_, fields)) => {
                        let names_lc: Vec<String> = fields.iter()
                            .map(|f| astring_to_string(f).to_ascii_lowercase())
                            .collect();
                        header_fields_reqs.push((names_lc, true));
                    }
                    _ => want_full_body = true,
                }
            }
        }
    }
    let need_body = want_full_body || want_header || want_text
        || !header_fields_reqs.is_empty() || !partial_reqs.is_empty();

    let mut out: Vec<Response<'static>> = Vec::with_capacity(rows.len() + 1);
    for row in &rows {
        let seq_num: i64 = row.get("seq");
        let seq = NonZeroU32::new(seq_num as u32).unwrap_or(NonZeroU32::MIN);
        let mut items: Vec<MessageDataItem<'static>> = Vec::new();
        let msg_id: Uuid = row.get("id");

        if w_uid {
            let uid_val: i64 = row.get("uid");
            items.push(MessageDataItem::Uid(
                NonZeroU32::new(uid_val as u32).unwrap_or(NonZeroU32::MIN),
            ));
        }

        // Flags are fetched unconditionally: needed for \Seen implicit-set
        // check even when the client did not request FLAGS in the item list.
        let mut flags_val: Vec<String> = row.get("flags");

        // RFC 3501 §6.4.5: non-peek body fetch MUST implicitly set \Seen.
        // We update the DB first, then update flags_val so the FLAGS response
        // (if emitted) reflects the new state without a second DB round-trip.
        let mut seen_was_set = false;
        if set_seen && !flags_val.iter().any(|f| f == "\\Seen") {
            let _ = sqlx::query(
                "UPDATE messages \
                 SET flags = array_cat(flags, ARRAY['\\Seen']::text[]) \
                 WHERE id = $1 AND NOT '\\Seen' = ANY(flags)",
            )
            .bind(msg_id)
            .execute(state.db())
            .await;
            flags_val.push("\\Seen".to_string());
            seen_was_set = true;
        }

        // Emit FLAGS when explicitly requested OR when \Seen was just set
        // (RFC 3501 §7.4.2: server SHOULD report updated flags).
        if w_flags || seen_was_set {
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
            let subject:    Option<String> = row.get("subject");
            let from_addr:  Option<String> = row.get("from_addr");
            let from_name:  Option<String> = row.get("from_name");
            let to_addrs:   Option<serde_json::Value> = row.try_get("to_addrs").ok();
            let cc_addrs:   Option<serde_json::Value> = row.try_get("cc_addrs").ok();
            let message_id: Option<String> = row.try_get("message_id").ok().flatten();
            let in_reply_to:Option<String> = row.try_get("in_reply_to").ok().flatten();
            let reply_to:   Option<String> = row.try_get("reply_to").ok().flatten();
            let date_ts:    Option<ChronoDateTime<Utc>> = row.try_get("date").ok().flatten();
            let date_str = date_ts.map(|t| t.format("%a, %d %b %Y %H:%M:%S +0000").to_string());
            items.push(MessageDataItem::Envelope(build_envelope(
                date_str.as_deref(),
                subject.as_deref(),
                from_addr.as_deref(),
                from_name.as_deref(),
                to_addrs.as_ref(),
                cc_addrs.as_ref(),
                message_id.as_deref(),
                in_reply_to.as_deref(),
                reply_to.as_deref(),
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
        if w_bodystructure {
            let sz: i32 = row.try_get("size_bytes").unwrap_or(0);
            items.push(MessageDataItem::BodyStructure(build_body_structure(sz as u32)));
        }
        if need_body {
            let body_path: Option<String> = row.try_get("body_path").ok();
            if let Some(path) = body_path {
                if let Some(raw) = fetch_body_bytes(state, &path).await {
                    // RFC822.HEADER ≡ BODY.PEEK[HEADER] — emits Rfc822Header item (no \Seen).
                    if w_rfc822_header {
                        let hdr = email_header_bytes(&raw);
                        items.push(MessageDataItem::Rfc822Header(
                            NString::from(Literal::unvalidated(hdr.clone())),
                        ));
                    }
                    // RFC822.TEXT ≡ BODY[TEXT] — emits Rfc822Text item (sets \Seen above).
                    if w_rfc822_text {
                        let txt = email_text_bytes(&raw);
                        items.push(MessageDataItem::Rfc822Text(
                            NString::from(Literal::unvalidated(txt.clone())),
                        ));
                    }
                    // RFC822 ≡ BODY[] — emits Rfc822 item (sets \Seen above).
                    if w_rfc822 {
                        items.push(MessageDataItem::Rfc822(
                            NString::from(Literal::unvalidated(raw.clone())),
                        ));
                    }
                    // BODY[HEADER] — RFC 3501 §6.4.5: header lines up to and
                    // including the blank separator (\r\n\r\n).
                    if want_header && !w_rfc822_header {
                        let hdr = email_header_bytes(&raw);
                        items.push(MessageDataItem::BodyExt {
                            section: Some(Section::Header(None)),
                            origin:  None,
                            data:    NString::from(Literal::unvalidated(hdr)),
                        });
                    }
                    // BODY[HEADER.FIELDS (...)] / BODY[HEADER.FIELDS.NOT (...)] —
                    // RFC 3501 §6.4.5: return only the named header lines (or all
                    // header lines EXCEPT the named ones for NOT variant).
                    for (field_names, is_not) in &header_fields_reqs {
                        let filtered = filter_header_fields(&raw, field_names, *is_not);
                        let astrings: Vec<AString<'static>> = field_names.iter()
                            .filter_map(|f| AString::try_from(f.clone()).ok())
                            .collect();
                        let section = if let Ok(v1) = Vec1::try_from(astrings.clone()) {
                            if *is_not {
                                Section::HeaderFieldsNot(None, v1)
                            } else {
                                Section::HeaderFields(None, v1)
                            }
                        } else {
                            // Empty field list: fall back to full header
                            Section::Header(None)
                        };
                        items.push(MessageDataItem::BodyExt {
                            section: Some(section),
                            origin:  None,
                            data:    NString::from(Literal::unvalidated(filtered)),
                        });
                    }
                    // BODY[TEXT] — everything after the blank separator.
                    if want_text && !w_rfc822_text {
                        let txt = email_text_bytes(&raw);
                        items.push(MessageDataItem::BodyExt {
                            section: Some(Section::Text(None)),
                            origin:  None,
                            data:    NString::from(Literal::unvalidated(txt)),
                        });
                    }
                    // BODY[] or fallback for unrecognised section specs.
                    if want_full_body && !w_rfc822 {
                        items.push(MessageDataItem::BodyExt {
                            section: None,
                            origin:  None,
                            data:    NString::from(Literal::unvalidated(raw.clone())),
                        });
                    }
                    // Partial fetch: BODY[<section>]<offset.count>
                    // RFC 3501 §6.4.5: the response echoes the starting octet in origin.
                    // Reads beyond EOF are truncated; starting beyond EOF returns empty.
                    for (sec_tag, offset, count) in &partial_reqs {
                        let source: Vec<u8> = match sec_tag {
                            1 => email_header_bytes(&raw),
                            2 => email_text_bytes(&raw),
                            _ => raw.clone(),
                        };
                        let section = match sec_tag {
                            1 => Some(Section::Header(None)),
                            2 => Some(Section::Text(None)),
                            _ => None,
                        };
                        let start = (*offset as usize).min(source.len());
                        let end   = (start + *count as usize).min(source.len());
                        let slice = source[start..end].to_vec();
                        items.push(MessageDataItem::BodyExt {
                            section,
                            origin:  Some(*offset),
                            data:    NString::from(Literal::unvalidated(slice)),
                        });
                    }
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

/// STORE / UID STORE — RFC 3501 §6.4.6.
/// Updates flags on matching messages. Returns untagged `* N FETCH (FLAGS ...)`
/// per message unless the command used the `.SILENT` suffix (StoreResponse::Silent).
async fn cmd_store(
    state: &AppState,
    tag: Tag<'static>,
    sequence_set: &imap_codec::imap_types::sequence::SequenceSet,
    kind: &imap_codec::imap_types::flag::StoreType,
    response: &StoreResponse,
    flags: &[Flag<'_>],
    uid: bool,
    sel: &SelectedMailbox,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let flag_strs: Vec<String> = flags.iter().map(|f| flag_to_str(f).to_owned()).collect();

    // Compute the WHERE clause once; reused for both UPDATE and post-SELECT.
    let range_clause: String = if uid {
        uid_clause(&sequence_ranges(sequence_set, u32::MAX))
    } else {
        seq_clause(&sequence_ranges(sequence_set, sel.exists))
    };

    // Run the UPDATE.
    let update_sql: String = if uid {
        match kind {
            imap_codec::imap_types::flag::StoreType::Add => format!(
                "UPDATE messages SET flags = array_cat(flags, $1::text[]) \
                 WHERE mailbox_id = $2 AND tenant_id = $3 AND ({range_clause})"
            ),
            imap_codec::imap_types::flag::StoreType::Remove => format!(
                "UPDATE messages \
                 SET flags = (SELECT array_agg(e) FROM unnest(flags) e WHERE NOT e = ANY($1::text[])) \
                 WHERE mailbox_id = $2 AND tenant_id = $3 AND ({range_clause})"
            ),
            imap_codec::imap_types::flag::StoreType::Replace => format!(
                "UPDATE messages SET flags = $1::text[] \
                 WHERE mailbox_id = $2 AND tenant_id = $3 AND ({range_clause})"
            ),
        }
    } else {
        match kind {
            imap_codec::imap_types::flag::StoreType::Add => format!(
                "WITH ordered AS (\
                   SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
                   FROM messages WHERE mailbox_id = $2 AND tenant_id = $3\
                 ) \
                 UPDATE messages SET flags = array_cat(flags, $1::text[]) \
                 WHERE id IN (SELECT id FROM ordered WHERE ({range_clause}))"
            ),
            imap_codec::imap_types::flag::StoreType::Remove => format!(
                "WITH ordered AS (\
                   SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
                   FROM messages WHERE mailbox_id = $2 AND tenant_id = $3\
                 ) \
                 UPDATE messages \
                 SET flags = (SELECT array_agg(e) FROM unnest(flags) e WHERE NOT e = ANY($1::text[])) \
                 WHERE id IN (SELECT id FROM ordered WHERE ({range_clause}))"
            ),
            imap_codec::imap_types::flag::StoreType::Replace => format!(
                "WITH ordered AS (\
                   SELECT id, ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
                   FROM messages WHERE mailbox_id = $2 AND tenant_id = $3\
                 ) \
                 UPDATE messages SET flags = $1::text[] \
                 WHERE id IN (SELECT id FROM ordered WHERE ({range_clause}))"
            ),
        }
    };
    let _ = sqlx::query(&update_sql)
        .bind(&flag_strs)
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .execute(state.db())
        .await;

    // RFC 3501 §6.4.6: unless .SILENT, return * N FETCH (FLAGS ...) for each row.
    let mut out: Vec<Response<'static>> = Vec::new();
    if matches!(*response, StoreResponse::Answer) {
        // Re-SELECT the affected rows to read back the post-update flag state.
        let select_sql = if uid {
            format!(
                "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, flags \
                 FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 AND ({range_clause}) \
                 ORDER BY received_at ASC"
            )
        } else {
            format!(
                "SELECT seq, flags FROM (\
                   SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, flags \
                   FROM messages WHERE mailbox_id = $1 AND tenant_id = $2\
                 ) sub WHERE ({range_clause}) ORDER BY seq ASC"
            )
        };
        let rows = sqlx::query(&select_sql)
            .bind(sel.mailbox_id)
            .bind(tenant_id)
            .fetch_all(state.db())
            .await
            .unwrap_or_default();

        for row in &rows {
            let seq_num: i64 = row.get("seq");
            let seq = NonZeroU32::new(seq_num as u32).unwrap_or(NonZeroU32::MIN);
            let flags_val: Vec<String> = row.get("flags");
            let flag_items: Vec<FlagFetch<'static>> = flags_val
                .iter()
                .filter_map(|f| parse_flag(f).map(FlagFetch::Flag))
                .collect();
            if let Ok(items) = Vec1::try_from(vec![MessageDataItem::Flags(flag_items)]) {
                out.push(Response::Data(Data::Fetch { seq, items }));
            }
        }
    }

    out.push(ok_tagged(tag, None, "STORE completed"));
    out
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
        let uid_w = uid_clause(&sequence_ranges(sequence_set, u32::MAX));
        sqlx::query(&format!(
            "SELECT uid, flags, size_bytes, body_path \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 \
             AND ({uid_w}) ORDER BY uid ASC",
        ))
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    } else {
        let seq_w = seq_clause(&sequence_ranges(sequence_set, sel.exists));
        sqlx::query(&format!(
            "SELECT uid, flags, size_bytes, body_path FROM (\
               SELECT uid, flags, size_bytes, body_path, \
                      ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
               FROM messages WHERE mailbox_id = $1 AND tenant_id = $2\
             ) AS ordered WHERE ({seq_w}) ORDER BY seq ASC",
        ))
        .bind(sel.mailbox_id)
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

/// MOVE / UID MOVE — RFC 6851.
/// Atomically moves messages from the selected mailbox to a destination.
/// Equivalent to COPY + expunge of the moved set (regardless of \Deleted flag).
/// Returns untagged EXPUNGE responses followed by a tagged OK [COPYUID …].
/// The selected mailbox exists count is updated after the move.
async fn cmd_move(
    state: &AppState,
    tag: Tag<'static>,
    sequence_set: &imap_codec::imap_types::sequence::SequenceSet,
    dst_mailbox: &ImapMailbox<'_>,
    uid: bool,
    sel: &mut SelectedMailbox,
    user_id: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let dst_name = mailbox_to_string(dst_mailbox);

    // Fetch source messages and their seq positions atomically.
    let src_rows = if uid {
        let uid_w = uid_clause(&sequence_ranges(sequence_set, u32::MAX));
        sqlx::query(&format!(
            "SELECT id, uid, flags, size_bytes, body_path, \
             ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $2 \
             AND ({uid_w}) ORDER BY received_at ASC",
        ))
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    } else {
        let seq_w = seq_clause(&sequence_ranges(sequence_set, sel.exists));
        sqlx::query(&format!(
            "SELECT id, uid, flags, size_bytes, body_path, seq FROM (\
               SELECT id, uid, flags, size_bytes, body_path, \
                      ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq \
               FROM messages WHERE mailbox_id = $1 AND tenant_id = $2\
             ) AS ordered WHERE ({seq_w}) ORDER BY seq ASC",
        ))
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
    };

    if src_rows.is_empty() {
        return vec![ok_tagged(tag, None, "MOVE completed")];
    }

    // Lock destination to serialise concurrent MOVE/COPY on same target.
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
    let mut src_ids: Vec<Uuid> = Vec::with_capacity(src_rows.len());
    let mut src_seqs: Vec<i64> = Vec::with_capacity(src_rows.len());

    for (i, row) in src_rows.iter().enumerate() {
        let src_id: Uuid = row.get("id");
        let src_uid_val: i64 = row.get("uid");
        let dst_uid_val = initial_next_uid + i as i64;
        let flags: Vec<String> = row.get("flags");
        let size_bytes: i32 = row.get("size_bytes");
        let body_path: String = row.get("body_path");
        let seq_val: i64 = row.get("seq");

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
        src_ids.push(src_id);
        src_seqs.push(seq_val);
    }

    // Delete source messages within the same transaction (atomic MOVE).
    let id_list: Vec<String> = src_ids.iter().map(|id| format!("'{id}'")).collect();
    let del_sql = format!(
        "DELETE FROM messages WHERE id IN ({}) AND mailbox_id = $1 AND tenant_id = $2",
        id_list.join(",")
    );
    if sqlx::query(&del_sql)
        .bind(sel.mailbox_id)
        .bind(tenant_id)
        .execute(&mut *tx)
        .await
        .is_err()
    {
        let _ = tx.rollback().await;
        return vec![no_tagged(tag, "internal error")];
    }

    if tx.commit().await.is_err() {
        return vec![no_tagged(tag, "internal error")];
    }

    // Update cached exists count.
    sel.exists = sel.exists.saturating_sub(src_seqs.len() as u32);

    // RFC 6851 §4.4: emit * N EXPUNGE for each moved message (seq adjusted for
    // preceding expunges in the same command).
    let mut out: Vec<Response<'static>> = Vec::with_capacity(src_seqs.len() + 1);
    for (i, &orig_seq) in src_seqs.iter().enumerate() {
        let adj = (orig_seq as usize).saturating_sub(i);
        if let Some(n) = NonZeroU32::new(adj as u32) {
            out.push(Response::Data(Data::Expunge(n)));
        }
    }

    let dst_uid_validity = NonZeroU32::new(dst_uid_validity_raw as u32).unwrap_or(NonZeroU32::MIN);
    out.push(ok_tagged(
        tag,
        Some(Code::CopyUid {
            uid_validity: dst_uid_validity,
            source: build_uid_set(src_uids),
            destination: build_uid_set(dst_uids),
        }),
        "MOVE completed",
    ));
    out
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
    let uid_w = uid_clause(&sequence_ranges(sequence_set, u32::MAX));

    let seq_rows = sqlx::query(&format!(
        "SELECT seq FROM (\
           SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, uid, flags \
           FROM messages WHERE mailbox_id = $1 AND tenant_id = $2\
         ) sub \
         WHERE '\\Deleted' = ANY(flags) AND ({uid_w}) ORDER BY seq ASC",
    ))
    .bind(sel.mailbox_id)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    let _ = sqlx::query(&format!(
        "DELETE FROM messages \
         WHERE mailbox_id = $1 AND tenant_id = $2 \
           AND '\\Deleted' = ANY(flags) AND ({uid_w})",
    ))
    .bind(sel.mailbox_id)
    .bind(tenant_id)
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

/// CREATE — RFC 3501 §6.3.3: create a new mailbox with default uid_validity.
/// Returns NO when the name already exists; ON CONFLICT DO NOTHING is the
/// guard (UNIQUE (user_id, folder_name) in the schema).
async fn cmd_create(
    state: &AppState,
    tag: Tag<'static>,
    mailbox: &ImapMailbox<'_>,
    user_id: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let name = mailbox_to_string(mailbox);
    let rows = sqlx::query(
        "INSERT INTO mailboxes (user_id, tenant_id, folder_name) \
         VALUES ($1, $2, $3) ON CONFLICT (user_id, folder_name) DO NOTHING \
         RETURNING id",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(&name)
    .fetch_all(state.db())
    .await
    .unwrap_or_default();

    if rows.is_empty() {
        return vec![no_tagged(tag, "mailbox already exists")];
    }
    vec![ok_tagged(tag, None, "CREATE completed")]
}

/// DELETE — RFC 3501 §6.3.4: permanently remove a mailbox.
/// ON DELETE CASCADE on messages removes all messages automatically.
/// INBOX deletion is prohibited by the RFC.
async fn cmd_delete(
    state: &AppState,
    tag: Tag<'static>,
    mailbox: &ImapMailbox<'_>,
    user_id: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let name = mailbox_to_string(mailbox);
    if name.eq_ignore_ascii_case("INBOX") {
        return vec![no_tagged(tag, "cannot delete INBOX")];
    }
    let result = sqlx::query(
        "DELETE FROM mailboxes WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(&name)
    .execute(state.db())
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => vec![ok_tagged(tag, None, "DELETE completed")],
        _ => vec![no_tagged(tag, "no such mailbox")],
    }
}

/// RENAME — RFC 3501 §6.3.5: rename an existing mailbox.
/// RENAME INBOX requires moving all messages to the new name and re-creating
/// an empty INBOX; that is not implemented — clients rarely need it.
async fn cmd_rename(
    state: &AppState,
    tag: Tag<'static>,
    from: &ImapMailbox<'_>,
    to: &ImapMailbox<'_>,
    user_id: Uuid,
    tenant_id: Uuid,
) -> Vec<Response<'static>> {
    let src = mailbox_to_string(from);
    let dst = mailbox_to_string(to);
    if src.eq_ignore_ascii_case("INBOX") {
        return vec![no_tagged(tag, "RENAME INBOX not supported")];
    }
    let result = sqlx::query(
        "UPDATE mailboxes SET folder_name = $4 \
         WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3",
    )
    .bind(user_id)
    .bind(tenant_id)
    .bind(&src)
    .bind(&dst)
    .execute(state.db())
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => vec![ok_tagged(tag, None, "RENAME completed")],
        _ => vec![no_tagged(tag, "no such mailbox")],
    }
}

/// NOOP — RFC 3501 §6.1.2: clients use this as a polling beat to discover
/// new messages and flag changes without re-SELECTing. When a mailbox is
/// selected we:
///   1. Emit `* N EXISTS` if the message count changed.
///   2. Emit `* N FETCH (FLAGS …)` for every message whose flags differ from
///      the snapshot taken at SELECT time (or the previous NOOP), so that
///      changes made by another session (e.g. webmail marking read) are pushed.
async fn cmd_noop(
    state: &AppState,
    tag: Tag<'static>,
    selected: &mut Option<SelectedMailbox>,
    tenant_id: Option<Uuid>,
) -> Vec<Response<'static>> {
    let mut out: Vec<Response<'static>> = Vec::with_capacity(4);
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

        // Re-query all seq+flags and diff against snapshot to detect external changes.
        let current: Vec<(u32, Vec<String>)> = sqlx::query(
            "SELECT ROW_NUMBER() OVER (ORDER BY received_at ASC) AS seq, flags \
             FROM messages WHERE mailbox_id = $1 AND tenant_id = $2",
        )
        .bind(sel.mailbox_id)
        .bind(tid)
        .fetch_all(state.db())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| {
            let s: i64 = r.get("seq");
            let f: Vec<String> = r.get("flags");
            (s as u32, f)
        })
        .collect();

        let mut new_snapshot: HashMap<u32, Vec<String>> = HashMap::with_capacity(current.len());
        for (seq, flags_now) in &current {
            let changed = sel.flags_snapshot.get(seq).map_or(true, |prev| prev != flags_now);
            if changed {
                let flag_items: Vec<FlagFetch<'static>> = flags_now
                    .iter()
                    .filter_map(|f| parse_flag(f).map(FlagFetch::Flag))
                    .collect();
                if let Ok(items) = Vec1::try_from(vec![MessageDataItem::Flags(flag_items)]) {
                    if let Some(seq_nz) = NonZeroU32::new(*seq) {
                        out.push(Response::Data(Data::Fetch { seq: seq_nz, items }));
                    }
                }
            }
            new_snapshot.insert(*seq, flags_now.clone());
        }
        sel.flags_snapshot = new_snapshot;
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

/// UNSELECT — RFC 3691: return to authenticated state WITHOUT silently expunging
/// \Deleted messages. Contrast with CLOSE (RFC 3501 §6.4.2) which expunges first.
/// Server MUST advertise UNSELECT capability before clients may use this command.
fn cmd_unselect(
    tag: Tag<'static>,
    sess: &mut SessionState,
    selected: &mut Option<SelectedMailbox>,
) -> Vec<Response<'static>> {
    if selected.is_none() {
        return vec![no_tagged(tag, "no mailbox selected")];
    }
    *selected = None;
    if *sess == SessionState::Selected {
        *sess = SessionState::Authenticated;
    }
    vec![ok_tagged(tag, None, "UNSELECT completed")]
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

/// Return the header section of an RFC 2822 message (up to and including the
/// blank-line separator \r\n\r\n). If no separator is found, the whole message
/// is treated as header. Per RFC 3501 §6.4.5 BODY[HEADER] semantics.
fn email_header_bytes(raw: &[u8]) -> Vec<u8> {
    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        raw[..pos + 4].to_vec()
    } else {
        raw.to_vec()
    }
}

/// Return the body section of an RFC 2822 message (everything after the
/// blank-line separator \r\n\r\n). Per RFC 3501 §6.4.5 BODY[TEXT] semantics.
fn email_text_bytes(raw: &[u8]) -> Vec<u8> {
    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
        raw[pos + 4..].to_vec()
    } else {
        Vec::new()
    }
}

/// Return only the header lines whose field name (before the first ':') is in
/// `fields` (or NOT in `fields` when `is_not` is true). Folded header lines
/// (continuation lines starting with whitespace) are kept with their parent.
/// The result always ends with the blank-line separator \r\n.
fn filter_header_fields(raw: &[u8], fields: &[String], is_not: bool) -> Vec<u8> {
    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n");
    let header_bytes = match header_end {
        Some(p) => &raw[..p],
        None => raw,
    };
    let header_str = String::from_utf8_lossy(header_bytes);
    let mut out = Vec::<u8>::new();
    let mut include_current = false;
    for line in header_str.split_inclusive('\n') {
        let is_fold = line.starts_with(' ') || line.starts_with('\t');
        if is_fold {
            if include_current {
                out.extend_from_slice(line.as_bytes());
            }
        } else if let Some(colon_pos) = line.find(':') {
            let field_name = line[..colon_pos].trim().to_ascii_lowercase();
            let matched = fields.iter().any(|f| f == &field_name);
            include_current = matched ^ is_not;
            if include_current {
                out.extend_from_slice(line.as_bytes());
            }
        } else {
            include_current = false;
        }
    }
    out.extend_from_slice(b"\r\n");
    out
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

/// Expand a SequenceSet into a list of (start, end) inclusive ranges.
/// `exists` resolves `*` (Asterisk) — pass `u32::MAX` for UID mode,
/// `sel.exists` for seq mode. Ranges with swapped endpoints are normalised.
fn sequence_ranges(
    seq_set: &imap_codec::imap_types::sequence::SequenceSet,
    exists: u32,
) -> Vec<(u32, u32)> {
    seq_set.0.as_ref().iter().map(|seq| {
        match seq {
            imap_codec::imap_types::sequence::Sequence::Single(val) => {
                let n = seq_or_uid_val(val, exists).max(1).min(exists);
                (n, n)
            }
            imap_codec::imap_types::sequence::Sequence::Range(from, to) => {
                let a = seq_or_uid_val(from, exists);
                let b = seq_or_uid_val(to, exists);
                let s = a.min(b).max(1);
                let e = a.max(b).min(exists);
                (s, e)
            }
        }
    }).collect()
}

/// Build a SQL fragment matching uid against a set of (start,end) ranges.
/// The result is safe to embed in a format! query because the values are
/// all u32 integers (no user-controlled strings). Falls back to FALSE when
/// the list is empty so the query returns zero rows rather than all rows.
fn uid_clause(ranges: &[(u32, u32)]) -> String {
    if ranges.is_empty() {
        return "FALSE".to_string();
    }
    ranges.iter()
        .map(|(s, e)| format!("(uid >= {s} AND uid <= {e})"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Same as uid_clause but for seq (ROW_NUMBER alias) columns.
fn seq_clause(ranges: &[(u32, u32)]) -> String {
    if ranges.is_empty() {
        return "FALSE".to_string();
    }
    ranges.iter()
        .map(|(s, e)| format!("(seq >= {s} AND seq <= {e})"))
        .collect::<Vec<_>>()
        .join(" OR ")
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
            Macro::Full => matches!(name, "FLAGS" | "ENVELOPE" | "RFC822.SIZE" | "INTERNALDATE" | "BODY" | "BODYSTRUCTURE"),
            _ => false,
        },
        // Pattern matching explícito por variante — o Debug repr anterior
        // produzia "Rfc822Size" (sem ponto) para RFC822.SIZE, quebrando a
        // detecção quando o cliente listava itens explicitamente.
        MacroOrMessageDataItemNames::MessageDataItemNames(items) => {
            items.iter().any(|item| matches!(
                (name, item),
                ("FLAGS",         MessageDataItemName::Flags)
                | ("ENVELOPE",    MessageDataItemName::Envelope)
                | ("RFC822.SIZE", MessageDataItemName::Rfc822Size)
                | ("UID",         MessageDataItemName::Uid)
                | ("INTERNALDATE",MessageDataItemName::InternalDate)
                | ("BODYEXT",     MessageDataItemName::BodyExt { .. })
                | ("BODY",        MessageDataItemName::Body)
                | ("BODYSTRUCTURE", MessageDataItemName::BodyStructure)
                | ("RFC822",        MessageDataItemName::Rfc822)
                | ("RFC822.HEADER", MessageDataItemName::Rfc822Header)
                | ("RFC822.TEXT",   MessageDataItemName::Rfc822Text)
            ))
        }
    }
}

/// Build a minimal RFC 3501 §7.4.2 BODYSTRUCTURE for a message.
/// Returns a single-part TEXT/PLAIN 7BIT structure using the stored
/// size_bytes. Without full MIME parsing this is conservative but
/// RFC-compliant: clients use it for preview without downloading.
fn build_body_structure(size_bytes: u32) -> BodyStructure<'static> {
    BodyStructure::Single {
        body: Body {
            basic: BasicFields {
                parameter_list: vec![],
                id: NString(None),
                description: NString(None),
                content_transfer_encoding: IString::try_from("7BIT").unwrap(),
                size: size_bytes,
            },
            specific: SpecificFields::Basic {
                r#type: IString::try_from("TEXT").unwrap(),
                subtype: IString::try_from("PLAIN").unwrap(),
            },
        },
        extension_data: None,
    }
}

fn addr_from_str(addr: &str, name: Option<&str>) -> imap_codec::imap_types::envelope::Address<'static> {
    use imap_codec::imap_types::envelope::Address;
    let (local, domain) = addr.split_once('@').unwrap_or((addr, ""));
    Address {
        name: name.and_then(|n| NString::try_from(n.to_owned()).ok()).unwrap_or(NString(None)),
        adl: NString(None),
        mailbox: NString::try_from(local.to_owned()).ok().unwrap_or(NString(None)),
        host: NString::try_from(domain.to_owned()).ok().unwrap_or(NString(None)),
    }
}

fn json_to_addr_list(v: Option<&serde_json::Value>) -> Vec<imap_codec::imap_types::envelope::Address<'static>> {
    v.and_then(|j| j.as_array())
        .map(|arr| {
            arr.iter().filter_map(|item| {
                let addr = item.get("addr").and_then(|a| a.as_str())?;
                let name = item.get("name").and_then(|n| n.as_str());
                Some(addr_from_str(addr, name))
            }).collect()
        })
        .unwrap_or_default()
}

fn build_envelope(
    date: Option<&str>,
    subject: Option<&str>,
    from_addr: Option<&str>,
    from_name: Option<&str>,
    to_addrs: Option<&serde_json::Value>,
    cc_addrs: Option<&serde_json::Value>,
    message_id: Option<&str>,
    in_reply_to: Option<&str>,
    reply_to_addr: Option<&str>,
) -> imap_codec::imap_types::envelope::Envelope<'static> {
    use imap_codec::imap_types::envelope::Envelope;

    let date_ns = date
        .and_then(|s| NString::try_from(s.to_owned()).ok())
        .unwrap_or(NString(None));

    let subject_ns = subject
        .and_then(|s| NString::try_from(s.to_owned()).ok())
        .unwrap_or(NString(None));

    let from = from_addr.map(|a| addr_from_str(a, from_name));
    let from_list = from.into_iter().collect::<Vec<_>>();

    let reply_to_list = reply_to_addr
        .map(|a| addr_from_str(a, None))
        .into_iter()
        .collect::<Vec<_>>();
    let reply_to_list = if reply_to_list.is_empty() { from_list.clone() } else { reply_to_list };

    let to_list = json_to_addr_list(to_addrs);
    let cc_list  = json_to_addr_list(cc_addrs);

    Envelope {
        date: date_ns,
        subject: subject_ns,
        from: from_list.clone(),
        sender: from_list.clone(),
        reply_to: reply_to_list,
        to: to_list,
        cc: cc_list,
        bcc: vec![],
        in_reply_to: in_reply_to
            .and_then(|s| NString::try_from(s.to_owned()).ok())
            .unwrap_or(NString(None)),
        message_id: message_id
            .and_then(|s| NString::try_from(s.to_owned()).ok())
            .unwrap_or(NString(None)),
    }
}

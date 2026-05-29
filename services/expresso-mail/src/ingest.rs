//! SMTP ingest pipeline: parse envelope/header/body and persist to DB.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use expresso_core::begin_tenant_tx;

use crate::state::AppState;

#[derive(Debug)]
struct ParsedMessage {
    subject: Option<String>,
    from_addr: Option<String>,
    from_name: Option<String>,
    to_addrs: Value,
    cc_addrs: Value,
    reply_to: Option<String>,
    message_id: Option<String>,
    in_reply_to: Option<String>,
    references_: Vec<String>,
    has_attachments: bool,
    preview_text: Option<String>,
    date: Option<OffsetDateTime>,
}

pub async fn process(
    state: &AppState,
    mail_from: Option<&str>,
    rcpts: &[String],
    raw: &[u8],
) -> anyhow::Result<usize> {
    if rcpts.is_empty() {
        return Ok(0);
    }

    // Scan for spam/virus (no-op if env vars not set; fail-open on errors).
    let scan = crate::scanner::scan(raw).await;
    if let Some(reason) = crate::scanner::should_reject(&scan) {
        tracing::warn!(rcpts = ?rcpts, reason = %reason, "ingest rejected");
        anyhow::bail!("message rejected: {}", reason);
    }
    let scan_headers = scan.to_headers();
    let raw_with_scan: Vec<u8> = if scan_headers.is_empty() {
        raw.to_vec()
    } else {
        let mut v = Vec::with_capacity(scan_headers.len() + raw.len());
        v.extend_from_slice(scan_headers.as_bytes());
        v.extend_from_slice(raw);
        v
    };
    let raw: &[u8] = &raw_with_scan;

    let parsed = parse_message(mail_from, rcpts, raw);
    let body_path = write_raw_message(state, raw).await?;
    let size_bytes = raw.len().min(i32::MAX as usize) as i32;

    let mut delivered = 0usize;
    let recipients = normalized_recipients(rcpts);

    for rcpt in recipients {
        // Recipient resolution runs without a tenant session var — inbound
        // delivery legitimately crosses tenants, one rcpt at a time.
        let mut probe = state.db().begin().await?;
        let user_rows: Vec<(Uuid, Uuid)> = sqlx::query_as(
            r#"
            SELECT id, tenant_id
            FROM users
            WHERE lower(email) = $1
            LIMIT 2
            "#,
        )
        .bind(&rcpt)
        .fetch_all(&mut *probe)
        .await?;
        probe.commit().await?;

        let (user_id, tenant_id) = match user_rows.as_slice() {
            [(uid, tid)] => (*uid, *tid),
            []           => {
                tracing::warn!(rcpt = %rcpt, "recipient not found; dropping delivery");
                continue;
            }
            _            => {
                // users.email is per-tenant unique; the same address in two
                // tenants is legal schema-wise. Refuse rather than guess.
                tracing::error!(rcpt = %rcpt, "recipient ambiguous across tenants; dropping delivery");
                continue;
            }
        };

        // Now arm the tenant session var for the rest of this delivery so RLS
        // can shadow-check every write below.
        let mut tx = begin_tenant_tx(state.db(), tenant_id).await?;

        // Sieve evaluation — fetch user script, evaluate, determine target folder/action.
        let target_folder: String = match apply_sieve(&mut tx, user_id, raw).await {
            SieveDecision::Deliver { folder } => folder,
            SieveDecision::Discard => {
                tracing::info!(rcpt = %rcpt, "sieve: discard");
                tx.commit().await?;
                continue;
            }
            SieveDecision::Reject { reason } => {
                tracing::warn!(rcpt = %rcpt, reason = %reason, "sieve: reject");
                tx.commit().await?;
                anyhow::bail!("sieve reject: {}", reason);
            }
        };

        let (mailbox_id, uid) = lock_or_create_mailbox(&mut tx, tenant_id, user_id, &target_folder).await?;

        let thread_id = resolve_thread_id(&mut tx, tenant_id, &parsed).await?;


        let msg_row_id: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO messages (
                mailbox_id,
                tenant_id,
                uid,
                flags,
                subject,
                from_addr,
                from_name,
                to_addrs,
                cc_addrs,
                bcc_addrs,
                reply_to,
                message_id,
                in_reply_to,
                references_,
                thread_id,
                has_attachments,
                size_bytes,
                body_path,
                preview_text,
                date
            )
            VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, '[]'::jsonb, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19
            )
            RETURNING id
            "#,
        )
        .bind(mailbox_id)
        .bind(tenant_id)
        .bind(uid)
        .bind(Vec::<String>::new())
        .bind(&parsed.subject)
        .bind(&parsed.from_addr)
        .bind(&parsed.from_name)
        .bind(&parsed.to_addrs)
        .bind(&parsed.cc_addrs)
        .bind(&parsed.reply_to)
        .bind(&parsed.message_id)
        .bind(&parsed.in_reply_to)
        .bind(&parsed.references_)
        .bind(thread_id)
        .bind(parsed.has_attachments)
        .bind(size_bytes)
        .bind(&body_path)
        .bind(&parsed.preview_text)
        .bind(parsed.date)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query("UPDATE mailboxes SET next_uid = GREATEST(next_uid, $2) WHERE id = $1")
            .bind(mailbox_id)
            .bind(uid + 1)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        delivered += 1;

        // Fire-and-forget: forward iMIP REPLY to calendar scheduling inbox.
        // Mail delivery must succeed regardless; only log on error.
        let calendar_url = state.cfg().calendar_url.clone();
        if !calendar_url.is_empty() {
            if let Some(ics) = crate::imip::extract_imip_reply(raw) {
                let tid = tenant_id;
                let uid_org = user_id;
                tokio::spawn(async move {
                    if let Err(e) = crate::imip::forward_reply(&calendar_url, tid, uid_org, &ics).await {
                        tracing::warn!(error = %e, tenant_id = %tid, user_id = %uid_org,
                            "iMIP REPLY forward failed");
                    }
                });
            }
        }

        // Fire-and-forget: notify search service
        let search_url   = state.cfg().search_url.clone();
        let search_token = state.cfg().search_token.clone();
        if !search_url.is_empty() {
            let doc = serde_json::json!({
                "document_id": format!("{}/{}", mailbox_id, uid),
                "tenant_id": tenant_id.to_string(),
                "subject": parsed.subject,
                "from_addr": parsed.from_addr,
                "body": parsed.preview_text,
            });
            tokio::spawn(async move {
                let mut req = reqwest::Client::new()
                    .post(format!("{}/api/v1/index", search_url))
                    .json(&doc);
                if !search_token.is_empty() {
                    req = req.bearer_auth(&search_token);
                }
                let _ = req.send().await;
            });
        }

        // Fire-and-forget: push SSE notification to expresso-notifications.
        let notif_url = state.cfg().notifications_url.clone();
        if !notif_url.is_empty() {
            let folder = target_folder.clone();
            tokio::spawn(async move {
                let payload = serde_json::json!({
                    "kind":       "new_mail",
                    "user_id":    user_id,
                    "tenant_id":  tenant_id,
                    "folder":     folder,
                });
                let _ = reqwest::Client::new()
                    .post(format!("{notif_url}/internal/notify"))
                    .json(&payload)
                    .timeout(std::time::Duration::from_secs(3))
                    .send()
                    .await;
            });
        }

        // Evaluate workflow rules via expresso-flows and apply resulting actions.
        let flows_url = state.cfg().flows_url.clone();
        if !flows_url.is_empty() {
            let payload = serde_json::json!({
                "user_id":         user_id,
                "tenant_id":       tenant_id,
                "message_id":      msg_row_id,
                "folder":          target_folder,
                "from_addr":       parsed.from_addr,
                "to_addrs":        parsed.to_addrs
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>())
                    .unwrap_or_default(),
                "subject":         parsed.subject,
                "has_attachments": parsed.has_attachments,
                "size_bytes":      size_bytes,
            });
            match reqwest::Client::new()
                .post(format!("{flows_url}/internal/process"))
                .json(&payload)
                .timeout(std::time::Duration::from_secs(3))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        apply_flow_actions(state, tenant_id, user_id, msg_row_id, &body).await;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "flows process call failed"),
                _ => {}
            }
        }

        // Fire-and-forget: journal a compliance copy to expresso-compliance.
        let compliance_url = state.cfg().compliance_url.clone();
        if !compliance_url.is_empty() {
            let from_addr  = parsed.from_addr.clone();
            let to_addrs   = parsed.to_addrs.clone();
            let subject    = parsed.subject.clone();
            let body_path2 = body_path.clone();
            tokio::spawn(async move {
                let payload = serde_json::json!({
                    "tenant_id":  tenant_id,
                    "user_id":    user_id,
                    "body_path":  body_path2,
                    "from_addr":  from_addr,
                    "to_addrs":   to_addrs,
                    "subject":    subject,
                    "size_bytes": size_bytes,
                });
                let _ = reqwest::Client::new()
                    .post(format!("{compliance_url}/internal/archive"))
                    .json(&payload)
                    .timeout(std::time::Duration::from_secs(3))
                    .send()
                    .await;
            });
        }
    }

    Ok(delivered)
}

/// Execute actions returned by expresso-flows (best-effort; errors are logged, not fatal).
async fn apply_flow_actions(
    state:      &AppState,
    tenant_id:  Uuid,
    user_id:    Uuid,
    message_id: Uuid,
    body:       &serde_json::Value,
) {
    let actions = match body.get("actions").and_then(|v| v.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => return,
    };

    for action in actions {
        let action_type = action.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match action_type {
            "move_to_folder" => {
                let folder = match action.get("params").and_then(|p| p.get("folder")).and_then(|v| v.as_str()) {
                    Some(f) => f.to_owned(),
                    None    => { tracing::warn!("flows: move_to_folder missing params.folder"); continue; }
                };
                let res = sqlx::query(
                    "UPDATE messages SET mailbox_id = ( \
                        SELECT id FROM mailboxes \
                        WHERE user_id = $1 AND tenant_id = $2 AND folder_name = $3 LIMIT 1 \
                     ) \
                     WHERE id = $4 AND tenant_id = $2 \
                       AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $1 AND tenant_id = $2)",
                )
                .bind(user_id)
                .bind(tenant_id)
                .bind(&folder)
                .bind(message_id)
                .execute(state.db())
                .await;
                if let Err(e) = res {
                    tracing::warn!(error = %e, folder = %folder, "flows: move_to_folder failed");
                }
            }
            "add_flag" => {
                let flag = match action.get("params").and_then(|p| p.get("flag")).and_then(|v| v.as_str()) {
                    Some(f) => f.to_owned(),
                    None    => { tracing::warn!("flows: add_flag missing params.flag"); continue; }
                };
                let res = sqlx::query(
                    "UPDATE messages \
                     SET flags = array_append(flags, $1) \
                     WHERE id = $2 AND tenant_id = $3 AND NOT ($1 = ANY(flags)) \
                       AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $4 AND tenant_id = $3)",
                )
                .bind(&flag)
                .bind(message_id)
                .bind(tenant_id)
                .bind(user_id)
                .execute(state.db())
                .await;
                if let Err(e) = res {
                    tracing::warn!(error = %e, flag = %flag, "flows: add_flag failed");
                }
            }
            "webhook" => {
                if let Some(url) = action.get("params").and_then(|p| p.get("url")).and_then(|v| v.as_str()) {
                    let url = url.to_owned();
                    let payload = serde_json::json!({ "message_id": message_id, "tenant_id": tenant_id });
                    tokio::spawn(async move {
                        let _ = reqwest::Client::new()
                            .post(&url)
                            .json(&payload)
                            .timeout(std::time::Duration::from_secs(5))
                            .send()
                            .await;
                    });
                }
            }
            "discard" => {
                // Hard-delete the message — the rule says to discard it.
                let res = sqlx::query(
                    "DELETE FROM messages \
                     WHERE id = $1 AND tenant_id = $2 \
                       AND mailbox_id IN (SELECT id FROM mailboxes WHERE user_id = $3 AND tenant_id = $2)",
                )
                .bind(message_id)
                .bind(tenant_id)
                .bind(user_id)
                .execute(state.db())
                .await;
                if let Err(e) = res {
                    tracing::warn!(error = %e, "flows: discard failed");
                }
            }
            "forward" => {
                let to_addr = match action.get("params").and_then(|p| p.get("to")).and_then(|v| v.as_str()) {
                    Some(a) => a.to_owned(),
                    None    => { tracing::warn!("flows: forward missing params.to"); continue; }
                };
                let db          = state.db().clone();
                let store       = state.store().cloned();
                let relay_host  = state.cfg().mail_server.relay_host.clone();
                let relay_port  = state.cfg().mail_server.relay_port;
                let domain      = state.cfg().mail_server.domain.clone();
                tokio::spawn(async move {
                    let body_path: Option<String> = sqlx::query_scalar(
                        "SELECT body_path FROM messages WHERE id = $1 AND tenant_id = $2 LIMIT 1",
                    )
                    .bind(message_id)
                    .bind(tenant_id)
                    .fetch_optional(&db)
                    .await
                    .unwrap_or(None);

                    let Some(path) = body_path else { return; };

                    // Read raw bytes from store or filesystem.
                    let raw: Option<Vec<u8>> = if let Some(s3_path) = path.strip_prefix("s3://") {
                        if let Some(idx) = s3_path.find('/') {
                            let key = &s3_path[idx + 1..];
                            if let Some(s) = &store {
                                s.get(key).await.ok()
                            } else { None }
                        } else { None }
                    } else if path.starts_with('/') {
                        tokio::fs::read(&path).await.ok()
                    } else { None };

                    let Some(raw) = raw else {
                        tracing::warn!(message_id = %message_id, "flows: forward — body not found");
                        return;
                    };

                    // Relay via plain SMTP to relay_host:relay_port.
                    match relay_forward(&relay_host, relay_port, &domain, &to_addr, &raw).await {
                        Ok(()) => tracing::info!(
                            message_id = %message_id, to = %to_addr, "flows: forward sent"),
                        Err(e) => tracing::warn!(
                            error = %e, message_id = %message_id, to = %to_addr,
                            "flows: forward relay failed"),
                    }
                });
            }
            other => tracing::warn!(action_type = %other, "flows: unknown action type"),
        }
    }
}

/// Relay raw RFC 5321 message bytes to `to_addr` via plain SMTP on `relay_host:relay_port`.
/// Implements the minimal SMTP exchange: EHLO → MAIL FROM → RCPT TO → DATA → QUIT.
/// No auth, no TLS — intended for internal MTA relay (Postfix, etc.) on trusted network.
async fn relay_forward(
    relay_host: &str,
    relay_port: u16,
    domain:     &str,
    to_addr:    &str,
    raw:        &[u8],
) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    let stream = TcpStream::connect((relay_host, relay_port)).await?;
    let (reader, mut writer) = tokio::io::split(stream);
    let mut lines = BufReader::new(reader).lines();

    macro_rules! expect {
        ($prefix:expr) => {{
            let line = lines.next_line().await?.unwrap_or_default();
            anyhow::ensure!(
                line.starts_with($prefix),
                "relay unexpected response: {line}"
            );
        }};
    }

    expect!("220");
    writer.write_all(format!("EHLO {domain}\r\n").as_bytes()).await?;
    // consume multi-line EHLO response
    loop {
        let line = lines.next_line().await?.unwrap_or_default();
        if line.starts_with("250 ") || (!line.starts_with("250") && !line.is_empty()) {
            break;
        }
        if line.is_empty() { break; }
    }
    writer.write_all(b"MAIL FROM:<>\r\n").await?;
    expect!("250");
    writer.write_all(format!("RCPT TO:<{to_addr}>\r\n").as_bytes()).await?;
    expect!("250");
    writer.write_all(b"DATA\r\n").await?;
    expect!("354");

    // Dot-stuff and send message body.
    for line in raw.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.starts_with(b".") {
            writer.write_all(b".").await?;
        }
        writer.write_all(line).await?;
        writer.write_all(b"\r\n").await?;
    }
    writer.write_all(b".\r\n").await?;
    expect!("250");
    writer.write_all(b"QUIT\r\n").await?;

    Ok(())
}

/// Lock the target folder's mailbox row (FOR UPDATE) and return (id, next_uid).
/// Auto-creates INBOX when missing; for non-INBOX folders returned by Sieve,
/// falls back to INBOX rather than silently materializing a folder the user
/// has no UI for. The lookup is filtered by `tenant_id` for defense-in-depth
/// in addition to RLS.
async fn lock_or_create_mailbox(
    tx:        &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    user_id:   Uuid,
    target:    &str,
) -> anyhow::Result<(Uuid, i64)> {
    let row: Option<(Uuid, i64)> = sqlx::query_as(
        r#"
        SELECT id, next_uid
        FROM mailboxes
        WHERE tenant_id = $1 AND user_id = $2 AND folder_name = $3
        FOR UPDATE
        "#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(target)
    .fetch_optional(&mut **tx)
    .await?;

    if let Some(r) = row { return Ok(r); }

    // Target folder not found. For INBOX, auto-create. For everything else,
    // fall through to INBOX so the message isn't lost.
    if target.eq_ignore_ascii_case("INBOX") {
        let created: Uuid = sqlx::query_scalar(
            r#"
            INSERT INTO mailboxes (user_id, tenant_id, folder_name, special_use, subscribed)
            VALUES ($1, $2, 'INBOX', '\Inbox', true)
            RETURNING id
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .fetch_one(&mut **tx)
        .await?;
        return Ok((created, 1));
    }

    tracing::warn!(
        user_id = %user_id, tenant_id = %tenant_id, target_folder = target,
        "Sieve target folder missing; falling back to INBOX",
    );
    let inbox: Option<(Uuid, i64)> = sqlx::query_as(
        r#"
        SELECT id, next_uid
        FROM mailboxes
        WHERE tenant_id = $1 AND user_id = $2 AND folder_name = 'INBOX'
        FOR UPDATE
        "#,
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(r) = inbox { return Ok(r); }

    let created: Uuid = sqlx::query_scalar(
        r#"
        INSERT INTO mailboxes (user_id, tenant_id, folder_name, special_use, subscribed)
        VALUES ($1, $2, 'INBOX', '\Inbox', true)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok((created, 1))
}

/// Resolve thread_id: lookup existing thread by in_reply_to/references, or create new.
async fn resolve_thread_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    parsed: &ParsedMessage,
) -> anyhow::Result<Uuid> {
    // Collect candidate message-ids to look up (in_reply_to first, then references)
    let mut candidates: Vec<&str> = Vec::new();
    if let Some(ref irt) = parsed.in_reply_to {
        candidates.push(irt.as_str());
    }
    for r in &parsed.references_ {
        if !candidates.contains(&r.as_str()) {
            candidates.push(r.as_str());
        }
    }

    if !candidates.is_empty() {
        let existing: Option<(Uuid,)> = sqlx::query_as(
            r#"
            SELECT thread_id
            FROM messages
            WHERE tenant_id = $1
              AND message_id = ANY($2)
              AND thread_id IS NOT NULL
            ORDER BY received_at ASC
            LIMIT 1
            "#,
        )
        .bind(tenant_id)
        .bind(&candidates)
        .fetch_optional(&mut **tx)
        .await?;

        if let Some((tid,)) = existing {
            return Ok(tid);
        }
    }

    Ok(Uuid::now_v7())
}

fn normalized_recipients(rcpts: &[String]) -> BTreeSet<String> {
    rcpts
        .iter()
        .map(|rcpt| rcpt.trim().to_ascii_lowercase())
        .filter(|rcpt| !rcpt.is_empty())
        .collect()
}

pub async fn write_raw_message(state: &AppState, raw: &[u8]) -> anyhow::Result<String> {
    let msg_id = Uuid::now_v7();
    // S3 path when object store available
    if let Some(store) = state.store() {
        let key = format!("raw/{msg_id}.eml");
        store.put(&key, raw.to_vec(), Some("message/rfc822")).await?;
        return Ok(format!("s3://{}/{key}", store.bucket()));
    }
    // Fallback: local filesystem
    let base_dir = std::env::var("EXPRESSO_MAIL_RAW_DIR")
        .unwrap_or_else(|_| "/tmp/expresso-mail/raw".to_string());
    tokio::fs::create_dir_all(&base_dir).await?;
    let path = format!("{base_dir}/{msg_id}.eml");
    tokio::fs::write(&path, raw).await?;
    Ok(path)
}

fn parse_message(mail_from: Option<&str>, rcpts: &[String], raw: &[u8]) -> ParsedMessage {
    let text = String::from_utf8_lossy(raw);
    let (headers_raw, body_raw) = split_headers_body(&text);
    let headers = parse_headers(headers_raw);

    let from_header = headers.get("from").map(String::as_str).or(mail_from);
    let (from_name, from_addr) = from_header
        .map(parse_single_address)
        .unwrap_or((None, None));

    let cc_addrs = parse_address_list_json(headers.get("cc").map(String::as_str).unwrap_or(""));
    let to_addrs = if rcpts.is_empty() {
        parse_address_list_json(headers.get("to").map(String::as_str).unwrap_or(""))
    } else {
        Value::Array(
            rcpts
                .iter()
                .map(|a| json!({"addr": a.trim(), "name": Value::Null}))
                .collect(),
        )
    };

    ParsedMessage {
        subject: headers.get("subject").cloned().filter(|s| !s.is_empty()),
        from_addr,
        from_name,
        to_addrs,
        cc_addrs,
        reply_to: headers.get("reply-to").cloned().filter(|s| !s.is_empty()),
        message_id: normalize_message_id(headers.get("message-id").cloned()),
        in_reply_to: normalize_message_id(headers.get("in-reply-to").cloned()),
        references_: parse_references(headers.get("references").map(String::as_str).unwrap_or("")),
        has_attachments: headers_raw
            .to_ascii_lowercase()
            .contains("content-disposition: attachment"),
        preview_text: make_preview(body_raw),
        date: parse_date(headers.get("date").map(String::as_str)),
    }
}

fn split_headers_body(raw: &str) -> (&str, &str) {
    if let Some((h, b)) = raw.split_once("\r\n\r\n") {
        return (h, b);
    }
    if let Some((h, b)) = raw.split_once("\n\n") {
        return (h, b);
    }
    (raw, "")
}

fn parse_headers(raw: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut current_key: Option<String> = None;

    for line in raw.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(key) = &current_key {
                if let Some(existing) = out.get_mut(key) {
                    existing.push(' ');
                    existing.push_str(line.trim());
                }
            }
            continue;
        }

        let Some((k, v)) = line.split_once(':') else {
            continue;
        };

        let key = k.trim().to_ascii_lowercase();
        let value = v.trim().to_string();

        out.entry(key.clone()).or_insert(value);
        current_key = Some(key);
    }

    out
}

fn parse_single_address(raw: &str) -> (Option<String>, Option<String>) {
    let value = raw.trim();
    if let (Some(start), Some(end)) = (value.rfind('<'), value.rfind('>')) {
        if start < end {
            let name = value[..start].trim().trim_matches('"');
            let addr = value[start + 1..end].trim();
            let out_name = if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            };
            let out_addr = if addr.is_empty() {
                None
            } else {
                Some(addr.to_string())
            };
            return (out_name, out_addr);
        }
    }

    let addr = if value.contains('@') {
        Some(value.trim_matches('"').to_string())
    } else {
        None
    };
    (None, addr)
}

fn parse_address_list_json(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Array(Vec::new());
    }

    let items = raw
        .split(',')
        .map(|chunk| {
            let (name, addr) = parse_single_address(chunk);
            json!({
                "addr": addr,
                "name": name,
            })
        })
        .collect();

    Value::Array(items)
}

fn parse_references(raw: &str) -> Vec<String> {
    raw.split_whitespace()
        .map(|part| part.trim_matches(|c| c == '<' || c == '>'))
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn normalize_message_id(id: Option<String>) -> Option<String> {
    id.map(|s| s.trim().trim_matches(|c| c == '<' || c == '>').to_string())
        .filter(|s| !s.is_empty())
}

fn make_preview(body: &str) -> Option<String> {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(500).collect())
}

fn parse_date(raw: Option<&str>) -> Option<OffsetDateTime> {
    let value = raw?.trim();
    if value.is_empty() {
        return None;
    }

    if let Ok(dt) = DateTime::<FixedOffset>::parse_from_rfc2822(value) {
        return OffsetDateTime::from_unix_timestamp(dt.timestamp()).ok();
    }

    if let Ok(dt) = DateTime::<FixedOffset>::parse_from_rfc3339(value) {
        return OffsetDateTime::from_unix_timestamp(dt.timestamp()).ok();
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_recipients_deduplicates_and_filters_empty_values() {
        let rcpts = vec![
            " User@Example.com ".to_string(),
            "user@example.com".to_string(),
            "".to_string(),
            "   ".to_string(),
            "Other@Example.com".to_string(),
        ];

        let got = normalized_recipients(&rcpts);

        assert_eq!(
            got.into_iter().collect::<Vec<_>>(),
            vec!["other@example.com".to_string(), "user@example.com".to_string()]
        );
    }

    #[test]
    fn parse_message_extracts_core_metadata() {
        let rcpts = vec!["recipient@example.com".to_string()];
        let raw = concat!(
            "From: \"Sender Example\" <sender@example.com>\r\n",
            "Subject: Hello\r\n",
            "Message-ID: <msg-1@example.com>\r\n",
            "In-Reply-To: <msg-0@example.com>\r\n",
            "References: <msg-a@example.com> <msg-b@example.com>\r\n",
            "Date: Wed, 01 Apr 2026 12:34:56 +0000\r\n",
            "\r\n",
            "Hello    world\nSecond line"
        )
        .as_bytes();

        let parsed = parse_message(Some("envelope@example.com"), &rcpts, raw);

        assert_eq!(parsed.subject.as_deref(), Some("Hello"));
        assert_eq!(parsed.from_name.as_deref(), Some("Sender Example"));
        assert_eq!(parsed.from_addr.as_deref(), Some("sender@example.com"));
        assert_eq!(parsed.reply_to, None);
        assert_eq!(parsed.message_id.as_deref(), Some("msg-1@example.com"));
        assert_eq!(parsed.in_reply_to.as_deref(), Some("msg-0@example.com"));
        assert_eq!(
            parsed.references_,
            vec!["msg-a@example.com".to_string(), "msg-b@example.com".to_string()]
        );
        assert_eq!(
            parsed.to_addrs,
            json!([{"addr": "recipient@example.com", "name": null}])
        );
        assert_eq!(parsed.preview_text.as_deref(), Some("Hello world Second line"));
        assert!(parsed.date.is_some());
    }
}

// ─── Sieve integration ───────────────────────────────────────────────────────

enum SieveDecision {
    Deliver { folder: String },
    Discard,
    Reject { reason: String },
}

async fn apply_sieve(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    raw: &[u8],
) -> SieveDecision {
    let vacation_script: Option<String> = sqlx::query_scalar(
        r#"SELECT sieve_script FROM user_vacation WHERE user_id = $1 AND enabled = true"#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .unwrap_or(None);

    let filter_script: Option<String> = sqlx::query_scalar(
        r#"SELECT script FROM user_sieve WHERE user_id = $1 AND enabled = true"#,
    )
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await
    .unwrap_or(None);

    let mut combined = String::new();
    if let Some(v) = vacation_script { combined.push_str(&v); combined.push('\n'); }
    if let Some(f) = filter_script   { combined.push_str(&f); }

    if combined.trim().is_empty() {
        return SieveDecision::Deliver { folder: "INBOX".to_string() };
    }

    let actions = crate::sieve::evaluate(combined.as_bytes(), raw);
    for a in &actions {
        match a {
            crate::sieve::FilterAction::Reject { reason } => {
                return SieveDecision::Reject { reason: reason.clone() };
            }
            crate::sieve::FilterAction::Discard => return SieveDecision::Discard,
            _ => {}
        }
    }
    for a in &actions {
        if let crate::sieve::FilterAction::FileInto { folder, .. } = a {
            return SieveDecision::Deliver { folder: folder.clone() };
        }
    }
    SieveDecision::Deliver { folder: "INBOX".to_string() }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn split_headers_body_crlf() {
        let (h, b) = split_headers_body("Subject: Hi\r\n\r\nBody text");
        assert_eq!(h, "Subject: Hi");
        assert_eq!(b, "Body text");
    }

    #[test]
    fn split_headers_body_lf_only() {
        let (h, b) = split_headers_body("Subject: Hi\n\nBody text");
        assert_eq!(h, "Subject: Hi");
        assert_eq!(b, "Body text");
    }

    #[test]
    fn split_headers_body_no_separator_returns_empty_body() {
        let (h, b) = split_headers_body("no blank line here");
        assert_eq!(h, "no blank line here");
        assert_eq!(b, "");
    }

    #[test]
    fn parse_single_address_angle_brackets() {
        let (name, addr) = parse_single_address("John Doe <john@ex.com>");
        assert_eq!(name.as_deref(), Some("John Doe"));
        assert_eq!(addr.as_deref(), Some("john@ex.com"));
    }

    #[test]
    fn parse_single_address_bare_email() {
        let (name, addr) = parse_single_address("user@ex.com");
        assert!(name.is_none());
        assert_eq!(addr.as_deref(), Some("user@ex.com"));
    }

    #[test]
    fn parse_single_address_non_email_returns_none() {
        let (name, addr) = parse_single_address("Not An Email");
        assert!(name.is_none());
        assert!(addr.is_none());
    }

    #[test]
    fn normalize_message_id_strips_angle_brackets() {
        assert_eq!(
            normalize_message_id(Some("<id@ex.com>".into())).as_deref(),
            Some("id@ex.com")
        );
    }

    #[test]
    fn normalize_message_id_none_returns_none() {
        assert!(normalize_message_id(None).is_none());
    }

    #[test]
    fn normalize_message_id_empty_returns_none() {
        assert!(normalize_message_id(Some("  ".into())).is_none());
    }

    #[test]
    fn make_preview_collapses_whitespace() {
        assert_eq!(make_preview("  hello   world  ").as_deref(), Some("hello world"));
    }

    #[test]
    fn make_preview_empty_body_returns_none() {
        assert!(make_preview("").is_none());
        assert!(make_preview("   ").is_none());
    }

    #[test]
    fn parse_references_strips_angle_brackets() {
        let refs = parse_references("<a@ex.com> <b@ex.com>");
        assert_eq!(refs, vec!["a@ex.com", "b@ex.com"]);
    }

    #[test]
    fn parse_references_empty_input() {
        assert!(parse_references("").is_empty());
    }

    #[test]
    fn make_preview_truncates_to_500_chars() {
        // "abcde " * 200 = 1200 chars; after split_whitespace join = 1199 chars
        // make_preview should truncate to 500.
        let long = "abcde ".repeat(200);
        let preview = make_preview(&long).unwrap();
        assert_eq!(preview.len(), 500);
    }

    #[test]
    fn parse_references_no_angle_brackets_treated_as_single_token() {
        let refs = parse_references("bare-id-no-brackets");
        assert!(!refs.is_empty());
    }

    #[test]
    fn normalize_message_id_non_empty_result() {
        let id = normalize_message_id(Some("<msg001@example.com>".to_string()));
        assert!(id.is_some());
        assert!(!id.unwrap().is_empty());
    }

    #[test]
    fn parse_single_address_quoted_name_stripped() {
        let (name, addr) = parse_single_address("\"Alice B\" <alice@ex.com>");
        assert_eq!(name.as_deref(), Some("Alice B"));
        assert_eq!(addr.as_deref(), Some("alice@ex.com"));
    }

    #[test]
    fn parse_single_address_empty_returns_none() {
        let (name, addr) = parse_single_address("");
        assert!(name.is_none());
        assert!(addr.is_none());
    }

    #[test]
    fn normalize_message_id_with_angle_brackets_strips_them() {
        let result = normalize_message_id(Some("<abc@example.com>".to_string())).expect("should parse");
        assert!(!result.starts_with('<'));
        assert!(!result.ends_with('>'));
    }

    #[test]
    fn make_preview_single_word_returns_word() {
        let preview = make_preview("hello");
        assert_eq!(preview, Some("hello".to_string()));
    }

    #[test]
    fn parse_references_whitespace_between_ids_splits_correctly() {
        let refs = parse_references("<a@b.com> <c@d.com>");
        assert_eq!(refs.len(), 2);
    }
}

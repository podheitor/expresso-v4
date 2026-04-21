//! SMTP ingest pipeline: parse envelope/header/body and persist to DB.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, FixedOffset};
use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

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

    let parsed = parse_message(mail_from, rcpts, raw);
    let body_path = write_raw_message(state, raw).await?;
    let size_bytes = raw.len().min(i32::MAX as usize) as i32;

    let mut delivered = 0usize;
    let recipients = normalized_recipients(rcpts);

    for rcpt in recipients {
        let mut tx = state.db().begin().await?;

        let user_row: Option<(Uuid, Uuid)> = sqlx::query_as(
            r#"
            SELECT id, tenant_id
            FROM users
            WHERE lower(email) = $1
            LIMIT 1
            "#,
        )
        .bind(&rcpt)
        .fetch_optional(&mut *tx)
        .await?;

        let Some((user_id, tenant_id)) = user_row else {
            tracing::warn!(rcpt = %rcpt, "recipient not found; dropping delivery");
            tx.commit().await?;
            continue;
        };

        let inbox_row: Option<(Uuid, i64)> = sqlx::query_as(
            r#"
            SELECT id, next_uid
            FROM mailboxes
            WHERE user_id = $1 AND folder_name = 'INBOX'
            FOR UPDATE
            "#,
        )
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;

        let (mailbox_id, uid) = match inbox_row {
            Some(row) => row,
            None => {
                let created_id: Uuid = sqlx::query_scalar(
                    r#"
                    INSERT INTO mailboxes (user_id, tenant_id, folder_name, special_use, subscribed)
                    VALUES ($1, $2, 'INBOX', '\Inbox', true)
                    RETURNING id
                    "#,
                )
                .bind(user_id)
                .bind(tenant_id)
                .fetch_one(&mut *tx)
                .await?;

                (created_id, 1)
            }
        };

        let thread_id = resolve_thread_id(&mut tx, tenant_id, &parsed).await?;


        sqlx::query(
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
        .execute(&mut *tx)
        .await?;

        sqlx::query("UPDATE mailboxes SET next_uid = GREATEST(next_uid, $2) WHERE id = $1")
            .bind(mailbox_id)
            .bind(uid + 1)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        delivered += 1;

        // Fire-and-forget: notify search service
        let search_url = state.cfg().search_url.clone();
        if !search_url.is_empty() {
            let doc = serde_json::json!({
                "document_id": format!("{}/{}", mailbox_id, uid),
                "tenant_id": tenant_id.to_string(),
                "subject": parsed.subject,
                "from_addr": parsed.from_addr,
                "body": parsed.preview_text,
            });
            tokio::spawn(async move {
                let _ = reqwest::Client::new()
                    .post(format!("{}/api/v1/index", search_url))
                    .json(&doc)
                    .send()
                    .await;
            });
        }
    }

    Ok(delivered)
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

async fn write_raw_message(state: &AppState, raw: &[u8]) -> anyhow::Result<String> {
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

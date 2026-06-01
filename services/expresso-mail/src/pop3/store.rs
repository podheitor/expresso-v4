//! DB access for the POP3 server. POP3 operates on a single mailbox (INBOX),
//! so these helpers resolve the INBOX message list, fetch raw bodies, and
//! commit the deletions accumulated during a session.

use uuid::Uuid;

use crate::state::AppState;

/// One INBOX message as POP3 sees it: a 1-based scan position is assigned by
/// the caller from the Vec index. `uid` (the message UUID) doubles as the
/// stable UIDL string. `size` is the octet count reported by LIST/STAT.
#[derive(Debug, Clone)]
pub struct Pop3Message {
    pub id: Uuid,
    pub size: i64,
    pub body_path: Option<String>,
}

/// Authenticate against the legacy `users` table (same pgcrypto `crypt()`
/// path IMAP LOGIN uses). Returns (user_id, tenant_id) on success.
pub async fn verify_login(state: &AppState, user: &str, pass: &str) -> Option<(Uuid, Uuid)> {
    sqlx::query_as(
        "SELECT id, tenant_id FROM users \
         WHERE lower(email) = lower($1) AND password_hash = crypt($2, password_hash) LIMIT 1",
    )
    .bind(user)
    .bind(pass)
    .fetch_optional(state.db())
    .await
    .ok()
    .flatten()
}

/// Load the INBOX message list in stable scan order (ascending received_at,
/// then UID) so positions are deterministic across STAT/LIST/RETR within a
/// session. Returns an empty Vec if the user has no INBOX.
pub async fn load_inbox(
    state: &AppState,
    user_id: Uuid,
    tenant_id: Uuid,
) -> anyhow::Result<Vec<Pop3Message>> {
    let mailbox_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM mailboxes \
         WHERE user_id = $1 AND folder_name = 'INBOX' AND tenant_id = $2 LIMIT 1",
    )
    .bind(user_id)
    .bind(tenant_id)
    .fetch_optional(state.db())
    .await?;

    let Some(mailbox_id) = mailbox_id else {
        return Ok(Vec::new());
    };

    let rows: Vec<(Uuid, Option<i32>, Option<String>)> = sqlx::query_as(
        "SELECT id, size_bytes, body_path FROM messages \
         WHERE mailbox_id = $1 AND tenant_id = $2 ORDER BY received_at ASC, uid ASC",
    )
    .bind(mailbox_id)
    .bind(tenant_id)
    .fetch_all(state.db())
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, size, body_path)| Pop3Message {
            id,
            size: size.unwrap_or(0) as i64,
            body_path,
        })
        .collect())
}

/// Fetch a message's raw RFC 2822 bytes from object storage (s3://…) or the
/// local filesystem fallback, mirroring the IMAP read path.
pub async fn fetch_body(state: &AppState, body_path: &str) -> Option<Vec<u8>> {
    if let Some(idx) = body_path
        .strip_prefix("s3://")
        .and_then(|s| s.find('/').map(|i| "s3://".len() + i + 1))
    {
        let key = &body_path[idx..];
        state.store()?.get(key).await.ok()
    } else if body_path.starts_with('/') {
        tokio::fs::read(body_path).await.ok()
    } else {
        None
    }
}

/// Commit the deletions marked during the session (RFC 1939 UPDATE state).
/// Hard-deletes the rows by id, scoped to the tenant for RLS safety.
pub async fn delete_messages(
    state: &AppState,
    ids: &[Uuid],
    tenant_id: Uuid,
) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query("DELETE FROM messages WHERE id = ANY($1) AND tenant_id = $2")
        .bind(ids)
        .bind(tenant_id)
        .execute(state.db())
        .await?;
    Ok(())
}

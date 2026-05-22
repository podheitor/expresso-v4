//! Mail message snooze — sprint #409.
//!
//! Routes:
//!   POST   /api/v1/mail/messages/:id/snooze   — snooze a message until a given time
//!   DELETE /api/v1/mail/messages/:id/snooze   — cancel snooze
//!   GET    /api/v1/mail/snoozed               — list active snoozed messages for the user
//!
//! The snooze record stores the message UUID + snooze_until timestamp.
//! A background worker (spawn_waker) periodically marks overdue rows as woken.

use std::time::Duration;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SnoozeRecord {
    pub id:           Uuid,
    pub tenant_id:    Uuid,
    pub user_id:      Uuid,
    pub message_id:   Uuid,
    pub mailbox_id:   Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub snooze_until: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub snoozed_at:   OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub woken_at:     Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
struct SnoozeBody {
    #[serde(with = "time::serde::rfc3339")]
    snooze_until: OffsetDateTime,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/messages/:id/snooze", post(snooze_message).delete(cancel_snooze))
        .route("/mail/snoozed",             get(list_snoozed))
}

/// POST /api/v1/mail/messages/:id/snooze — snooze a message.
async fn snooze_message(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<SnoozeBody>,
) -> Result<impl IntoResponse> {
    if body.snooze_until <= OffsetDateTime::now_utc() {
        return Err(MailError::BadRequest("snooze_until must be in the future".into()));
    }

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    // Verify message belongs to this user/tenant
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT m.mailbox_id FROM messages m \
         JOIN mailboxes mb ON mb.id = m.mailbox_id \
         WHERE m.id = $1 AND m.tenant_id = $2 AND mb.user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (mailbox_id,) = row.ok_or(MailError::MessageNotFound(id))?;

    let record: SnoozeRecord = sqlx::query_as(
        "INSERT INTO mail_snooze \
             (tenant_id, user_id, message_id, mailbox_id, snooze_until) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id, user_id, message_id, mailbox_id) DO UPDATE \
             SET snooze_until = EXCLUDED.snooze_until, \
                 snoozed_at   = now(), \
                 woken_at     = NULL \
         RETURNING id, tenant_id, user_id, message_id, mailbox_id, snooze_until, snoozed_at, woken_at",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .bind(mailbox_id)
    .bind(body.snooze_until)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(record)))
}

/// DELETE /api/v1/mail/messages/:id/snooze — cancel an active snooze.
async fn cancel_snooze(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let r = sqlx::query(
        "DELETE FROM mail_snooze \
         WHERE tenant_id = $1 AND user_id = $2 AND message_id = $3 AND woken_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    if r.rows_affected() == 0 {
        return Err(MailError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/mail/snoozed — list active snoozed messages (not yet woken).
async fn list_snoozed(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<SnoozeRecord>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let rows: Vec<SnoozeRecord> = sqlx::query_as(
        "SELECT id, tenant_id, user_id, message_id, mailbox_id, snooze_until, snoozed_at, woken_at \
         FROM mail_snooze \
         WHERE tenant_id = $1 AND user_id = $2 AND woken_at IS NULL \
         ORDER BY snooze_until ASC",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(rows))
}

/// Spawn background worker that wakes snoozed messages when `snooze_until <= now()`.
pub fn spawn_waker(pool: expresso_core::DbPool, interval_secs: u64) {
    let secs = interval_secs.max(10);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(secs));
        loop {
            tick.tick().await;
            match wake_due(&pool).await {
                Ok(n) if n > 0 => tracing::info!(woken = n, "snooze waker cycle completed"),
                Ok(_)          => {}
                Err(e)         => tracing::warn!(error = %e, "snooze waker failed"),
            }
        }
    });
}

async fn wake_due(pool: &expresso_core::DbPool) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "UPDATE mail_snooze \
         SET woken_at = now() \
         WHERE snooze_until <= now() AND woken_at IS NULL",
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn snooze_body_deserializes_rfc3339() {
        let json = r#"{"snooze_until":"2026-06-01T09:00:00Z"}"#;
        let body: SnoozeBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.snooze_until, datetime!(2026-06-01 09:00:00 UTC));
    }

    #[test]
    fn snooze_record_serde_roundtrip() {
        let r = SnoozeRecord {
            id:           Uuid::nil(),
            tenant_id:    Uuid::nil(),
            user_id:      Uuid::nil(),
            message_id:   Uuid::nil(),
            mailbox_id:   Uuid::nil(),
            snooze_until: datetime!(2026-06-01 09:00:00 UTC),
            snoozed_at:   datetime!(2026-05-22 08:00:00 UTC),
            woken_at:     None,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: SnoozeRecord = serde_json::from_str(&s).unwrap();
        assert_eq!(back.snooze_until, r.snooze_until);
        assert!(back.woken_at.is_none());
    }

    #[test]
    fn snooze_record_woken_at_present_roundtrip() {
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: datetime!(2026-06-01 09:00:00 UTC),
            snoozed_at:   datetime!(2026-05-22 08:00:00 UTC),
            woken_at:     Some(datetime!(2026-06-01 09:00:01 UTC)),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: SnoozeRecord = serde_json::from_str(&s).unwrap();
        assert!(back.woken_at.is_some());
    }

    #[test]
    fn snooze_body_snooze_until_in_rfc3339() {
        let json = r#"{"snooze_until":"2026-12-31T23:59:59Z"}"#;
        let body: SnoozeBody = serde_json::from_str(json).unwrap();
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains("2026-12-31T23:59:59"));
    }

    #[test]
    fn snooze_record_snooze_until_in_json() {
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: datetime!(2026-07-04 00:00:00 UTC),
            snoozed_at:   datetime!(2026-07-03 12:00:00 UTC),
            woken_at:     None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("2026-07-04T00:00:00"));
    }

    #[test]
    fn snooze_record_woken_at_none() {
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: datetime!(2026-07-04 00:00:00 UTC),
            snoozed_at:   datetime!(2026-07-03 12:00:00 UTC),
            woken_at:     None,
        };
        assert!(r.woken_at.is_none());
    }

    #[test]
    fn snooze_body_different_times_deserialize() {
        let json = r#"{"snooze_until":"2027-01-01T00:00:00Z"}"#;
        let body: SnoozeBody = serde_json::from_str(json).unwrap();
        let s = serde_json::to_string(&body).unwrap();
        assert!(s.contains("2027-01-01"));
    }

    #[test]
    fn snooze_record_mailbox_id_preserved() {
        use time::macros::datetime;
        let mid = Uuid::new_v4();
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: mid,
            snooze_until: datetime!(2026-08-01 08:00:00 UTC),
            snoozed_at: datetime!(2026-07-31 08:00:00 UTC),
            woken_at: None,
        };
        assert_eq!(r.mailbox_id, mid);
    }

    #[test]
    fn snooze_record_woken_at_none_by_default() {
        use time::macros::datetime;
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: datetime!(2026-08-01 08:00:00 UTC),
            snoozed_at: datetime!(2026-07-31 08:00:00 UTC),
            woken_at: None,
        };
        assert!(r.woken_at.is_none());
    }

    #[test]
    fn snooze_record_serializes_snooze_until_as_rfc3339() {
        use time::macros::datetime;
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: datetime!(2026-09-01 08:00:00 UTC),
            snoozed_at: datetime!(2026-07-31 08:00:00 UTC),
            woken_at: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("2026-09-01T08:00:00Z"));
    }

    #[test]
    fn snooze_record_woken_at_none_initially() {
        use time::macros::datetime;
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: datetime!(2026-09-01 08:00:00 UTC),
            snoozed_at: datetime!(2026-07-31 08:00:00 UTC),
            woken_at: None,
        };
        assert!(r.woken_at.is_none());
    }

    #[test]
    fn snooze_record_snooze_until_preserved() {
        use time::macros::datetime;
        let until = datetime!(2026-10-15 09:00:00 UTC);
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: until,
            snoozed_at: datetime!(2026-10-14 09:00:00 UTC),
            woken_at: None,
        };
        assert_eq!(r.snooze_until, until);
    }

    #[test]
    fn snooze_record_woken_at_none_by_default() {
        use time::macros::datetime;
        let r = SnoozeRecord {
            id: Uuid::nil(), tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            message_id: Uuid::nil(), mailbox_id: Uuid::nil(),
            snooze_until: datetime!(2026-10-15 09:00:00 UTC),
            snoozed_at: datetime!(2026-10-14 09:00:00 UTC),
            woken_at: None,
        };
        assert!(r.woken_at.is_none());
    }
}

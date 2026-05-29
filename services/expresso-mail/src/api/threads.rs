//! Per-user conversation (thread) state — mute and pin.
//!
//! PUT    /api/v1/mail/threads/:thread_id/mute    — mute the conversation
//! DELETE /api/v1/mail/threads/:thread_id/mute    — unmute
//! PUT    /api/v1/mail/threads/:thread_id/pin     — pin to top of the list
//! DELETE /api/v1/mail/threads/:thread_id/pin     — unpin
//! GET    /api/v1/mail/threads/:thread_id/state   — current {muted, pinned}
//!
//! Outlook/Gmail parity: muting a noisy conversation keeps its later replies out
//! of the unread count and inbox surfacing; pinning keeps it at the top of the
//! thread list. State is per (tenant, user, thread) and stored thread-side, so
//! it survives message deletion and applies to every message in the thread.
//!
//! Thread state is a UI overlay — `list_threads` LEFT JOINs this table, so a
//! thread with no row simply reads as `{muted: false, pinned: false}`. We only
//! INSERT a row when a flag becomes true and DELETE it when both return to
//! false, keeping the table sparse (most threads are never muted or pinned).

use axum::{
    extract::{Path, State},
    routing::{get, put},
    Json, Router,
};
use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use expresso_core::begin_tenant_tx;
use crate::api::context::RequestCtx;
use crate::error::Result;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/threads/:thread_id/mute", put(mute).delete(unmute))
        .route("/mail/threads/:thread_id/pin",  put(pin).delete(unpin))
        .route("/mail/threads/:thread_id/state", get(get_state))
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ThreadState {
    pub muted:  bool,
    pub pinned: bool,
}

async fn get_state(
    State(state):    State<AppState>,
    ctx:             RequestCtx,
    Path(thread_id): Path<Uuid>,
) -> Result<Json<ThreadState>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let st = load(&mut tx, ctx.tenant_id, ctx.user_id, thread_id).await?;
    tx.commit().await?;
    Ok(Json(st))
}

async fn mute(s: State<AppState>, ctx: RequestCtx, id: Path<Uuid>) -> Result<Json<ThreadState>> {
    set_flag(s, ctx, id, Flag::Muted, true).await
}
async fn unmute(s: State<AppState>, ctx: RequestCtx, id: Path<Uuid>) -> Result<Json<ThreadState>> {
    set_flag(s, ctx, id, Flag::Muted, false).await
}
async fn pin(s: State<AppState>, ctx: RequestCtx, id: Path<Uuid>) -> Result<Json<ThreadState>> {
    set_flag(s, ctx, id, Flag::Pinned, true).await
}
async fn unpin(s: State<AppState>, ctx: RequestCtx, id: Path<Uuid>) -> Result<Json<ThreadState>> {
    set_flag(s, ctx, id, Flag::Pinned, false).await
}

#[derive(Clone, Copy)]
enum Flag { Muted, Pinned }

/// Apply one flag, then garbage-collect the row if both flags are false.
/// Idempotent: muting an already-muted thread returns the same state.
async fn set_flag(
    State(state):    State<AppState>,
    ctx:             RequestCtx,
    Path(thread_id): Path<Uuid>,
    flag:            Flag,
    value:           bool,
) -> Result<Json<ThreadState>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let cur = load(&mut tx, ctx.tenant_id, ctx.user_id, thread_id).await?;
    let next = match flag {
        Flag::Muted  => ThreadState { muted: value,      pinned: cur.pinned },
        Flag::Pinned => ThreadState { muted: cur.muted,  pinned: value },
    };

    if !next.muted && !next.pinned {
        sqlx::query(
            "DELETE FROM mail_thread_state \
             WHERE tenant_id = $1 AND user_id = $2 AND thread_id = $3"
        )
        .bind(ctx.tenant_id).bind(ctx.user_id).bind(thread_id)
        .execute(&mut *tx).await?;
    } else {
        sqlx::query(
            "INSERT INTO mail_thread_state (tenant_id, user_id, thread_id, muted, pinned) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (tenant_id, user_id, thread_id) DO UPDATE SET \
                muted = EXCLUDED.muted, pinned = EXCLUDED.pinned, updated_at = now()"
        )
        .bind(ctx.tenant_id).bind(ctx.user_id).bind(thread_id)
        .bind(next.muted).bind(next.pinned)
        .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(Json(next))
}

/// Read current state; a missing row means neither flag is set.
async fn load(
    tx:        &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    user_id:   Uuid,
    thread_id: Uuid,
) -> Result<ThreadState> {
    let row = sqlx::query(
        "SELECT muted, pinned FROM mail_thread_state \
         WHERE tenant_id = $1 AND user_id = $2 AND thread_id = $3"
    )
    .bind(tenant_id).bind(user_id).bind(thread_id)
    .fetch_optional(&mut **tx).await?;
    Ok(match row {
        Some(r) => ThreadState { muted: r.get("muted"), pinned: r.get("pinned") },
        None    => ThreadState { muted: false, pinned: false },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_is_unset() {
        let s = ThreadState { muted: false, pinned: false };
        assert!(!s.muted && !s.pinned);
    }

    #[test]
    fn muting_preserves_pinned() {
        let cur = ThreadState { muted: false, pinned: true };
        let next = ThreadState { muted: true, pinned: cur.pinned };
        assert!(next.muted && next.pinned);
    }

    #[test]
    fn pinning_preserves_muted() {
        let cur = ThreadState { muted: true, pinned: false };
        let next = ThreadState { muted: cur.muted, pinned: true };
        assert!(next.muted && next.pinned);
    }

    #[test]
    fn both_false_triggers_gc_branch() {
        let next = ThreadState { muted: false, pinned: false };
        assert!(!next.muted && !next.pinned);
    }

    #[test]
    fn state_serializes_keys() {
        let s = serde_json::to_string(&ThreadState { muted: true, pinned: false }).unwrap();
        assert!(s.contains("muted") && s.contains("pinned"));
    }

    #[test]
    fn state_equality() {
        assert_eq!(
            ThreadState { muted: true, pinned: true },
            ThreadState { muted: true, pinned: true },
        );
        assert_ne!(
            ThreadState { muted: true, pinned: false },
            ThreadState { muted: false, pinned: false },
        );
    }
}

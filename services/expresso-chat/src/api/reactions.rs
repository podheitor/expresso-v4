//! Reactions — emoji reactions on messages.
//!
//! POST   /api/v1/channels/:id/reactions          → add    {event_id, emoji}
//! DELETE /api/v1/channels/:id/reactions          → remove {event_id, emoji}
//! GET    /api/v1/channels/:id/reactions?events=a,b → aggregated counts
//!
//! event_id is the Matrix event id of the target message. Reaction add/remove
//! broadcasts over the SSE bus so other members update live.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{ChannelRepo, ReactionCount, ReactionRepo};
use crate::error::{ChatError, Result};
use crate::events::ChatEvent;
use crate::state::AppState;

/// Cap on emoji length. A grapheme cluster (incl. ZWJ sequences + skin-tone
/// modifiers) fits well under this; the cap just blocks abuse (a "reaction"
/// carrying a paragraph). Bytes, not chars — emoji are multi-byte.
const MAX_EMOJI_BYTES: usize = 64;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/channels/:id/reactions",
        post(add).delete(remove).get(list),
    )
}

#[derive(Debug, Deserialize)]
pub struct ReactionBody {
    pub event_id: String,
    pub emoji: String,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Comma-separated Matrix event ids to aggregate reactions for.
    #[serde(default)]
    pub events: String,
}

/// Validate the (event_id, emoji) pair shared by add/remove.
fn validate(body: &ReactionBody) -> Result<()> {
    if body.event_id.trim().is_empty() {
        return Err(ChatError::BadRequest("event_id required".into()));
    }
    if body.emoji.trim().is_empty() {
        return Err(ChatError::BadRequest("emoji required".into()));
    }
    if body.emoji.len() > MAX_EMOJI_BYTES {
        return Err(ChatError::BadRequest(format!(
            "emoji too long: {} bytes (max {})",
            body.emoji.len(),
            MAX_EMOJI_BYTES
        )));
    }
    Ok(())
}

/// Shared membership gate.
async fn require_member(state: &AppState, ctx: &RequestCtx, channel: Uuid) -> Result<()> {
    let pool = state.db_or_unavailable()?;
    if !ChannelRepo::new(pool)
        .is_member(ctx.tenant_id, channel, ctx.user_id)
        .await?
    {
        return Err(ChatError::NotMember);
    }
    Ok(())
}

async fn add(
    state: State<AppState>,
    ctx: RequestCtx,
    channel_id: Path<Uuid>,
    body: Json<ReactionBody>,
) -> Result<StatusCode> {
    let r = add_inner(state, ctx, channel_id, body).await;
    crate::metrics::record_result(crate::metrics::OP_REACTION, &r);
    r
}

async fn add_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
    Json(body): Json<ReactionBody>,
) -> Result<StatusCode> {
    validate(&body)?;
    require_member(&state, &ctx, channel_id).await?;
    let pool = state.db_or_unavailable()?;
    let inserted = ReactionRepo::new(pool)
        .add(
            ctx.tenant_id,
            channel_id,
            &body.event_id,
            ctx.user_id,
            &body.emoji,
        )
        .await?;
    // Only broadcast on a real change (idempotent re-react inserts nothing).
    if inserted {
        state.bus().publish(ChatEvent::reaction(
            ctx.tenant_id,
            channel_id,
            ctx.user_id,
            body.event_id,
            body.emoji,
            true,
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn remove(
    state: State<AppState>,
    ctx: RequestCtx,
    channel_id: Path<Uuid>,
    body: Json<ReactionBody>,
) -> Result<StatusCode> {
    let r = remove_inner(state, ctx, channel_id, body).await;
    crate::metrics::record_result(crate::metrics::OP_REACTION, &r);
    r
}

async fn remove_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
    Json(body): Json<ReactionBody>,
) -> Result<StatusCode> {
    validate(&body)?;
    require_member(&state, &ctx, channel_id).await?;
    let pool = state.db_or_unavailable()?;
    let deleted = ReactionRepo::new(pool)
        .remove(
            ctx.tenant_id,
            channel_id,
            &body.event_id,
            ctx.user_id,
            &body.emoji,
        )
        .await?;
    if deleted {
        state.bus().publish(ChatEvent::reaction(
            ctx.tenant_id,
            channel_id,
            ctx.user_id,
            body.event_id,
            body.emoji,
            false,
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(channel_id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ReactionCount>>> {
    require_member(&state, &ctx, channel_id).await?;
    let event_ids: Vec<String> = q
        .events
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let pool = state.db_or_unavailable()?;
    let counts = ReactionRepo::new(pool)
        .list_for_events(ctx.tenant_id, channel_id, &event_ids)
        .await?;
    Ok(Json(counts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(event_id: &str, emoji: &str) -> ReactionBody {
        ReactionBody {
            event_id: event_id.into(),
            emoji: emoji.into(),
        }
    }

    #[test]
    fn validate_accepts_normal_reaction() {
        assert!(validate(&body("$evt:hs", "👍")).is_ok());
    }

    #[test]
    fn validate_rejects_empty_event_id() {
        let err = format!("{:?}", validate(&body("  ", "👍")).unwrap_err());
        assert!(err.contains("event_id required"), "got: {err}");
    }

    #[test]
    fn validate_rejects_empty_emoji() {
        let err = format!("{:?}", validate(&body("$evt:hs", "")).unwrap_err());
        assert!(err.contains("emoji required"), "got: {err}");
    }

    #[test]
    fn validate_rejects_oversize_emoji() {
        let big = "x".repeat(MAX_EMOJI_BYTES + 1);
        let err = format!("{:?}", validate(&body("$evt:hs", &big)).unwrap_err());
        assert!(err.contains("too long"), "got: {err}");
    }

    #[test]
    fn validate_accepts_zwj_sequence_emoji() {
        // 👨‍👩‍👧 (family, ZWJ) is multi-byte but well under the cap.
        assert!(validate(&body("$evt:hs", "👨‍👩‍👧")).is_ok());
    }

    #[test]
    fn list_query_splits_and_trims_event_ids() {
        let q = ListQuery {
            events: " $a:hs , $b:hs ,, ".into(),
        };
        let ids: Vec<String> = q
            .events
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(ids, vec!["$a:hs".to_string(), "$b:hs".to_string()]);
    }
}

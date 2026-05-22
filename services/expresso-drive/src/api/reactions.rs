//! Drive comment reactions — sprint #406.
//!
//! Routes:
//!   POST   /api/v1/drive/files/:id/comments/:comment_id/reactions
//!   DELETE /api/v1/drive/files/:id/comments/:comment_id/reactions

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{DriveError, Result};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CommentReaction {
    pub id:         Uuid,
    pub comment_id: Uuid,
    pub file_id:    Uuid,
    pub tenant_id:  Uuid,
    pub user_id:    Uuid,
    pub emoji:      String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct ReactionBody {
    emoji: String,
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/v1/drive/files/:id/comments/:comment_id/reactions",
        post(add_reaction).delete(remove_reaction),
    )
}

/// POST /api/v1/drive/files/:id/comments/:comment_id/reactions — add or update a reaction.
async fn add_reaction(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((file_id, comment_id)): Path<(Uuid, Uuid)>,
    Json(body):   Json<ReactionBody>,
) -> Result<impl IntoResponse> {
    let emoji = body.emoji.trim().to_string();
    if emoji.is_empty() || emoji.chars().count() > 8 {
        return Err(DriveError::BadRequest("emoji must be 1-8 characters".into()));
    }

    let pool = state.db_or_unavailable()?;

    // Verify comment belongs to this file and tenant
    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM drive_file_comments \
         WHERE id = $1 AND file_id = $2 AND tenant_id = $3",
    )
    .bind(comment_id)
    .bind(file_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(DriveError::NotFound(comment_id));
    }

    let reaction: CommentReaction = sqlx::query_as(
        "INSERT INTO drive_comment_reactions \
             (comment_id, file_id, tenant_id, user_id, emoji) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (comment_id, tenant_id, user_id, emoji) DO UPDATE \
             SET created_at = EXCLUDED.created_at \
         RETURNING id, comment_id, file_id, tenant_id, user_id, emoji, created_at",
    )
    .bind(comment_id)
    .bind(file_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&emoji)
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(reaction)))
}

/// DELETE /api/v1/drive/files/:id/comments/:comment_id/reactions — remove a reaction.
async fn remove_reaction(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((file_id, comment_id)): Path<(Uuid, Uuid)>,
    Json(body):   Json<ReactionBody>,
) -> Result<impl IntoResponse> {
    let emoji = body.emoji.trim().to_string();
    if emoji.is_empty() {
        return Err(DriveError::BadRequest("emoji is required".into()));
    }

    let pool = state.db_or_unavailable()?;

    let r = sqlx::query(
        "DELETE FROM drive_comment_reactions \
         WHERE comment_id = $1 AND file_id = $2 AND tenant_id = $3 \
           AND user_id = $4 AND emoji = $5",
    )
    .bind(comment_id)
    .bind(file_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&emoji)
    .execute(pool)
    .await?;

    if r.rows_affected() == 0 {
        return Err(DriveError::NotFound(comment_id));
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn comment_reaction_serde_roundtrip() {
        let r = CommentReaction {
            id: Uuid::nil(), comment_id: Uuid::nil(), file_id: Uuid::nil(),
            tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            emoji: "👍".into(),
            created_at: datetime!(2026-05-22 10:00:00 UTC),
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: CommentReaction = serde_json::from_str(&s).unwrap();
        assert_eq!(back.emoji, "👍");
    }

    #[test]
    fn reaction_body_deserializes() {
        let json = r#"{"emoji":"❤️"}"#;
        let b: ReactionBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.emoji, "❤️");
    }

    #[test]
    fn comment_reaction_created_at_rfc3339() {
        use time::macros::datetime;
        let r = CommentReaction {
            id: Uuid::nil(), comment_id: Uuid::nil(), file_id: Uuid::nil(),
            tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            emoji: "🔥".into(),
            created_at: datetime!(2026-03-15 12:00:00 UTC),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("2026-03-15T12:00:00"));
    }

    #[test]
    fn comment_reaction_clone_preserves_emoji() {
        use time::macros::datetime;
        let r = CommentReaction {
            id: Uuid::nil(), comment_id: Uuid::nil(), file_id: Uuid::nil(),
            tenant_id: Uuid::nil(), user_id: Uuid::nil(),
            emoji: "✅".into(),
            created_at: datetime!(2026-01-01 00:00:00 UTC),
        };
        let c = r.clone();
        assert_eq!(c.emoji, "✅");
    }
}

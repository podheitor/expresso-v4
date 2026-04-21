//! Messages — thin proxy to Matrix CS API.
//!
//! POST /api/v1/channels/:id/messages       → send m.room.message (text)
//! GET  /api/v1/channels/:id/messages       → list recent events (raw Matrix chunk JSON)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::ChannelRepo;
use crate::error::{ChatError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/channels/:id/messages", post(send).get(list))
}

#[derive(Debug, Deserialize)]
pub struct SendBody {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 { 50 }

async fn send(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<SendBody>,
) -> Result<(StatusCode, Json<Value>)> {
    if body.body.trim().is_empty() {
        return Err(ChatError::BadRequest("body required".into()));
    }
    let pool   = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo   = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo.get(ctx.tenant_id, id).await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let event_id = matrix.send_text(&acting_as, &ch.matrix_room_id, &body.body).await?;
    Ok((StatusCode::CREATED, Json(json!({ "event_id": event_id }))))
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>> {
    let pool   = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo   = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo.get(ctx.tenant_id, id).await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let value = matrix.list_messages(&acting_as, &ch.matrix_room_id, q.limit).await?;
    Ok(Json(value))
}

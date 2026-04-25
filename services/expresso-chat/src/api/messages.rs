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

/// Cap por mensagem individual. Matrix CS API aceita textos grandes mas
/// `m.room.message` realístico é chat — 32 KiB cobre paste de stack-trace
/// e bloco de log; acima disso é abuso (spam-cannon, exfil, fanout DoS
/// pra todos os membros do canal via federation).
pub const MAX_MESSAGE_BODY_BYTES: usize = 32 * 1024;

/// Cap pra paginação de histórico. Matrix `/messages` aceita `limit`
/// arbitrário e devolve eventos completos (com state events embutidos),
/// fácil de inflar resposta. Default 50 já era pequeno; clampa pra 200.
pub const MAX_LIST_LIMIT: u32 = 200;
pub const DEFAULT_LIST_LIMIT: u32 = 50;

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

fn default_limit() -> u32 { DEFAULT_LIST_LIMIT }

async fn send(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<SendBody>,
) -> Result<(StatusCode, Json<Value>)> {
    validate_message_body(&body.body)?;
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
    Query(mut q): Query<ListQuery>,
) -> Result<Json<Value>> {
    if q.limit == 0 || q.limit > MAX_LIST_LIMIT {
        q.limit = q.limit.clamp(1, MAX_LIST_LIMIT);
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
    let value = matrix.list_messages(&acting_as, &ch.matrix_room_id, q.limit).await?;
    Ok(Json(value))
}

/// Gate em send: empty já era rejeitado; agora rejeita oversize ANTES
/// de tocar Matrix (evita gastar fanout/federation budget em payload abusivo).
fn validate_message_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(ChatError::BadRequest("body required".into()));
    }
    if body.len() > MAX_MESSAGE_BODY_BYTES {
        return Err(ChatError::BadRequest(format!(
            "message body too large: {} bytes (max {})",
            body.len(), MAX_MESSAGE_BODY_BYTES
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_body() {
        let err = format!("{:?}", validate_message_body("").unwrap_err());
        assert!(err.contains("body required"), "got: {err}");
    }

    #[test]
    fn rejects_whitespace_body() {
        let err = format!("{:?}", validate_message_body("   \r\n  ").unwrap_err());
        assert!(err.contains("body required"), "got: {err}");
    }

    #[test]
    fn accepts_small_body() {
        assert!(validate_message_body("hello world").is_ok());
    }

    #[test]
    fn rejects_oversize_body() {
        let s = "x".repeat(MAX_MESSAGE_BODY_BYTES + 1);
        let err = format!("{:?}", validate_message_body(&s).unwrap_err());
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn boundary_body_accepted() {
        let s = "x".repeat(MAX_MESSAGE_BODY_BYTES);
        assert!(validate_message_body(&s).is_ok());
    }

    #[test]
    fn list_limit_constants_sane() {
        assert!(DEFAULT_LIST_LIMIT < MAX_LIST_LIMIT);
        assert!(MAX_LIST_LIMIT >= 50);
    }
}

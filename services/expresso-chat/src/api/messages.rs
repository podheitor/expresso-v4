//! Messages — thin proxy to Matrix CS API.
//!
//! POST   /api/v1/channels/:id/messages                    → send m.room.message (text)
//! GET    /api/v1/channels/:id/messages                    → list recent events (raw chunk JSON)
//! GET    /api/v1/channels/:id/messages/search?q=&limit=   → substring-search recent messages
//! PUT    /api/v1/channels/:id/messages/:event_id          → edit (m.replace)
//! DELETE /api/v1/channels/:id/messages/:event_id          → delete (redact)
//! POST   /api/v1/channels/:id/messages/:event_id/replies  → reply in thread (m.thread)
//! GET    /api/v1/channels/:id/messages/:event_id/replies  → list thread (raw chunk JSON)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{ChannelRepo, ReadMarkerRepo};
use crate::error::{ChatError, Result};
use crate::events::ChatEvent;
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
        // `search` is a static segment — register before `:event_id` so matchit
        // doesn't capture it as an event id.
        .route("/api/v1/channels/:id/messages/search", get(search))
        .route(
            "/api/v1/channels/:id/messages/:event_id",
            axum::routing::put(edit).delete(remove),
        )
        .route(
            "/api/v1/channels/:id/messages/:event_id/replies",
            post(reply).get(list_thread),
        )
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

fn default_limit() -> u32 {
    DEFAULT_LIST_LIMIT
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    /// Case-insensitive substring to match against message bodies.
    pub q: String,
    /// How many recent messages to scan (the search window). Clamped like list.
    #[serde(default = "default_limit")]
    pub limit: u32,
}

/// GET /api/v1/channels/:id/messages/search?q=&limit= — substring-search the
/// channel's recent messages (membership required). Scans the latest `limit`
/// events and returns the matches; deep history beyond the window isn't
/// searched (see `MatrixClient::search_messages`).
async fn search(
    state: State<AppState>,
    ctx: RequestCtx,
    id: Path<Uuid>,
    q: Query<SearchQuery>,
) -> Result<Json<Value>> {
    let r = search_inner(state, ctx, id, q).await;
    crate::metrics::record_result(crate::metrics::OP_MESSAGE_LIST, &r);
    r
}

async fn search_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Query(mut q): Query<SearchQuery>,
) -> Result<Json<Value>> {
    if q.q.trim().is_empty() {
        return Err(ChatError::BadRequest("q must not be empty".into()));
    }
    if q.limit == 0 || q.limit > MAX_LIST_LIMIT {
        q.limit = q.limit.clamp(1, MAX_LIST_LIMIT);
    }
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let value = matrix
        .search_messages(&acting_as, &ch.matrix_room_id, q.q.trim(), q.limit)
        .await?;
    Ok(Json(value))
}

async fn send(
    state: State<AppState>,
    ctx: RequestCtx,
    id: Path<Uuid>,
    body: Json<SendBody>,
) -> Result<(StatusCode, Json<Value>)> {
    let r = send_inner(state, ctx, id, body).await;
    crate::metrics::record_result(crate::metrics::OP_MESSAGE_SEND, &r);
    r
}

async fn send_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<SendBody>,
) -> Result<(StatusCode, Json<Value>)> {
    validate_message_body(&body.body)?;
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let event_id = matrix
        .send_text(&acting_as, &ch.matrix_room_id, &body.body)
        .await?;

    // Fan out to live SSE subscribers of this channel (best-effort; durable
    // copy already lives in Matrix). Skipped silently when nobody is connected.
    state.bus().publish(crate::events::ChatEvent::message(
        ctx.tenant_id,
        id,
        ctx.user_id,
        event_id.clone(),
        body.body,
    ));

    // Unread bookkeeping: bump channel activity (others now see it unread),
    // then mark it read for the sender (their own message isn't "unread" to
    // them). Best-effort — a failure here must not fail an accepted send.
    let marks = ReadMarkerRepo::new(pool);
    if let Err(e) = marks.bump_activity(ctx.tenant_id, id).await {
        tracing::warn!(error = %e, channel = %id, "chat: bump_activity failed");
    }
    if let Err(e) = marks.mark_read(ctx.tenant_id, id, ctx.user_id).await {
        tracing::warn!(error = %e, channel = %id, "chat: sender mark_read failed");
    }

    Ok((StatusCode::CREATED, Json(json!({ "event_id": event_id }))))
}

async fn list(
    state: State<AppState>,
    ctx: RequestCtx,
    id: Path<Uuid>,
    q: Query<ListQuery>,
) -> Result<Json<Value>> {
    let r = list_inner(state, ctx, id, q).await;
    crate::metrics::record_result(crate::metrics::OP_MESSAGE_LIST, &r);
    r
}

async fn list_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Query(mut q): Query<ListQuery>,
) -> Result<Json<Value>> {
    if q.limit == 0 || q.limit > MAX_LIST_LIMIT {
        q.limit = q.limit.clamp(1, MAX_LIST_LIMIT);
    }
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let value = matrix
        .list_messages(&acting_as, &ch.matrix_room_id, q.limit)
        .await?;
    Ok(Json(value))
}

#[derive(Debug, Deserialize)]
pub struct EditBody {
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    pub reason: Option<String>,
}

/// PUT …/messages/:event_id — edit a message via an m.replace relation. The HS
/// rejects edits to events the caller didn't author, surfacing as 502 Matrix.
async fn edit(
    state: State<AppState>,
    ctx: RequestCtx,
    path: Path<(Uuid, String)>,
    body: Json<EditBody>,
) -> Result<(StatusCode, Json<Value>)> {
    let r = edit_inner(state, ctx, path, body).await;
    crate::metrics::record_result(crate::metrics::OP_MESSAGE_EDIT, &r);
    r
}

async fn edit_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, event_id)): Path<(Uuid, String)>,
    Json(body): Json<EditBody>,
) -> Result<(StatusCode, Json<Value>)> {
    validate_message_body(&body.body)?;
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let edit_event_id = matrix
        .edit_text(&acting_as, &ch.matrix_room_id, &event_id, &body.body)
        .await?;

    state.bus().publish(ChatEvent::edit(
        ctx.tenant_id,
        id,
        ctx.user_id,
        event_id,
        body.body,
    ));

    Ok((StatusCode::OK, Json(json!({ "event_id": edit_event_id }))))
}

/// DELETE …/messages/:event_id — redact a message. The HS enforces redaction
/// permission (sender, or sufficient power level), surfacing as 502 Matrix.
async fn remove(
    state: State<AppState>,
    ctx: RequestCtx,
    path: Path<(Uuid, String)>,
    q: Query<DeleteQuery>,
) -> Result<(StatusCode, Json<Value>)> {
    let r = remove_inner(state, ctx, path, q).await;
    crate::metrics::record_result(crate::metrics::OP_MESSAGE_DELETE, &r);
    r
}

async fn remove_inner(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, event_id)): Path<(Uuid, String)>,
    Query(q): Query<DeleteQuery>,
) -> Result<(StatusCode, Json<Value>)> {
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let redaction_id = matrix
        .redact(
            &acting_as,
            &ch.matrix_room_id,
            &event_id,
            q.reason.as_deref(),
        )
        .await?;

    state
        .bus()
        .publish(ChatEvent::delete(ctx.tenant_id, id, ctx.user_id, event_id));

    Ok((StatusCode::OK, Json(json!({ "event_id": redaction_id }))))
}

#[derive(Debug, Deserialize)]
pub struct ReplyBody {
    pub body: String,
}

/// POST …/messages/:event_id/replies — post a threaded reply rooted at
/// `:event_id` (m.thread). Body validation mirrors `send`; the reply is fanned
/// out to live subscribers and bumps channel activity like a top-level message.
async fn reply(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, event_id)): Path<(Uuid, String)>,
    Json(body): Json<ReplyBody>,
) -> Result<(StatusCode, Json<Value>)> {
    validate_message_body(&body.body)?;
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let reply_event_id = matrix
        .reply_in_thread(&acting_as, &ch.matrix_room_id, &event_id, &body.body)
        .await?;

    state.bus().publish(ChatEvent::reply(
        ctx.tenant_id,
        id,
        ctx.user_id,
        event_id,
        reply_event_id.clone(),
        body.body,
    ));

    let marks = ReadMarkerRepo::new(pool);
    if let Err(e) = marks.bump_activity(ctx.tenant_id, id).await {
        tracing::warn!(error = %e, channel = %id, "chat: bump_activity failed");
    }
    if let Err(e) = marks.mark_read(ctx.tenant_id, id, ctx.user_id).await {
        tracing::warn!(error = %e, channel = %id, "chat: sender mark_read failed");
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({ "event_id": reply_event_id })),
    ))
}

/// GET …/messages/:event_id/replies — list the thread rooted at `:event_id`.
async fn list_thread(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, event_id)): Path<(Uuid, String)>,
    Query(mut q): Query<ListQuery>,
) -> Result<Json<Value>> {
    if q.limit == 0 || q.limit > MAX_LIST_LIMIT {
        q.limit = q.limit.clamp(1, MAX_LIST_LIMIT);
    }
    let pool = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo
        .get(ctx.tenant_id, id)
        .await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as = matrix.mxid_for(ctx.user_id);
    let value = matrix
        .list_thread(&acting_as, &ch.matrix_room_id, &event_id, q.limit)
        .await?;
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
            body.len(),
            MAX_MESSAGE_BODY_BYTES
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_body_deserializes() {
        let b: ReplyBody = serde_json::from_str(r#"{"body":"in thread"}"#).unwrap();
        assert_eq!(b.body, "in thread");
    }

    #[test]
    fn reply_reuses_message_body_validation() {
        // reply() calls validate_message_body, so empty/oversize replies are rejected.
        assert!(validate_message_body("").is_err());
        assert!(validate_message_body("ok").is_ok());
    }

    #[test]
    fn reply_body_missing_field_fails_deser() {
        let result: std::result::Result<ReplyBody, _> = serde_json::from_str(r#"{}"#);
        assert!(result.is_err());
    }

    #[test]
    fn edit_body_deserializes() {
        let b: EditBody = serde_json::from_str(r#"{"body":"fixed typo"}"#).unwrap();
        assert_eq!(b.body, "fixed typo");
    }

    #[test]
    fn edit_reuses_message_body_validation() {
        // edit() calls validate_message_body, so empty edits are rejected too.
        assert!(validate_message_body("").is_err());
        assert!(validate_message_body("ok").is_ok());
    }

    #[test]
    fn delete_query_reason_optional() {
        let q: DeleteQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.reason.is_none());
        let q: DeleteQuery = serde_json::from_str(r#"{"reason":"spam"}"#).unwrap();
        assert_eq!(q.reason.as_deref(), Some("spam"));
    }

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

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn list_limit_constants_sane() {
        assert!(DEFAULT_LIST_LIMIT < MAX_LIST_LIMIT);
        assert!(MAX_LIST_LIMIT >= 50);
    }

    #[test]
    fn accepts_unicode_body() {
        assert!(validate_message_body("Olá, mundo! 🎉").is_ok());
    }

    #[test]
    fn list_query_default_limit() {
        let q: ListQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(q.limit, DEFAULT_LIST_LIMIT);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_list_limit_not_exceeded_by_default() {
        assert!(DEFAULT_LIST_LIMIT <= MAX_LIST_LIMIT);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn default_list_limit_is_positive() {
        assert!(DEFAULT_LIST_LIMIT > 0);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_list_limit_is_at_most_200() {
        assert!(MAX_LIST_LIMIT <= 200);
    }

    #[test]
    fn max_list_limit_exact_value_is_200() {
        assert_eq!(MAX_LIST_LIMIT, 200);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_list_limit_is_positive() {
        assert!(MAX_LIST_LIMIT > 0);
    }

    #[test]
    fn send_body_text_field_deserializes() {
        let b: SendBody = serde_json::from_str(r#"{"body":"hello"}"#).unwrap();
        assert_eq!(b.body, "hello");
    }

    #[test]
    fn send_body_unicode_preserved() {
        let b: SendBody = serde_json::from_str(r#"{"body":"Olá 🌍"}"#).unwrap();
        assert!(b.body.contains("Olá"));
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn default_limit_is_less_than_max_limit() {
        assert!(DEFAULT_LIST_LIMIT < MAX_LIST_LIMIT);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_message_body_bytes_constant_is_positive() {
        assert!(MAX_MESSAGE_BODY_BYTES > 0);
    }

    #[test]
    fn default_limit_is_50() {
        assert_eq!(DEFAULT_LIST_LIMIT, 50);
    }

    #[test]
    fn max_message_body_bytes_is_32kb() {
        assert_eq!(MAX_MESSAGE_BODY_BYTES, 32 * 1024);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_list_limit_exceeds_default_limit() {
        assert!(MAX_LIST_LIMIT > DEFAULT_LIST_LIMIT);
    }

    #[test]
    fn send_body_missing_body_field_fails_deser() {
        let result: Result<SendBody, _> = serde_json::from_str(r#"{}"#);
        assert!(result.is_err());
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn default_list_limit_is_less_than_or_equal_to_max() {
        assert!(DEFAULT_LIST_LIMIT <= MAX_LIST_LIMIT);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_message_body_bytes_is_nonzero() {
        assert!(MAX_MESSAGE_BODY_BYTES > 0);
    }

    #[test]
    fn default_limit_equals_fifty() {
        assert_eq!(DEFAULT_LIST_LIMIT, 50);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_message_body_bytes_is_less_than_one_mib() {
        assert!(MAX_MESSAGE_BODY_BYTES < 1024 * 1024);
    }

    #[test]
    fn validate_message_body_single_char_accepted() {
        assert!(validate_message_body("x").is_ok());
    }
}

//! Channels REST API.
//!
//! POST   /api/v1/channels              → create room on Synapse + persist metadata
//! GET    /api/v1/channels              → list current user's channels
//! GET    /api/v1/channels/:id          → channel detail
//! POST   /api/v1/channels/:id/members  → invite a user (Matrix invite + DB row)
//! GET    /api/v1/channels/:id/members  → list members
//! DELETE /api/v1/channels/:id          → archive (soft delete; Matrix room stays)

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Channel, ChannelKind, ChannelMember, ChannelRepo, MemberRole, NewChannel};
use crate::error::{ChatError, Result};
use crate::matrix::{CreateRoomRequest, RoomPreset};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/channels", post(create).get(list))
        .route("/api/v1/channels/:id", get(get_one).delete(archive))
        .route("/api/v1/channels/:id/members", post(add_member).get(list_members))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name:    String,
    pub topic:   Option<String>,
    pub kind:    Option<ChannelKind>,
    pub team_id: Option<Uuid>,
    /// Additional users (beyond the creator) to invite at creation.
    #[serde(default)]
    pub invite:  Vec<Uuid>,
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<Channel>)> {
    if body.name.trim().is_empty() {
        return Err(ChatError::BadRequest("name required".into()));
    }
    let pool   = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;

    let kind = body.kind.unwrap_or(ChannelKind::Team);
    let preset = match kind {
        ChannelKind::Direct => RoomPreset::TrustedPrivateChat,
        ChannelKind::Announcement => RoomPreset::PublicChat,
        _ => RoomPreset::PrivateChat,
    };
    let invites_mxid: Vec<String> = body.invite.iter().map(|u| matrix.mxid_for(*u)).collect();
    let acting_as = matrix.mxid_for(ctx.user_id);

    let room = matrix.create_room(&acting_as, &CreateRoomRequest {
        name:   &body.name,
        topic:  body.topic.as_deref(),
        preset,
        invite: &invites_mxid,
    }).await?;

    let repo = ChannelRepo::new(pool);
    let ch = repo.create(ctx.tenant_id, ctx.user_id, NewChannel {
        matrix_room_id: room.room_id,
        name:           body.name,
        topic:          body.topic,
        kind,
        team_id:        body.team_id,
    }).await?;

    // Mirror explicit invitees into the DB member index.
    for u in body.invite {
        repo.add_member(ctx.tenant_id, ch.id, u, MemberRole::Member).await?;
    }

    Ok((StatusCode::CREATED, Json(ch)))
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    req_headers: HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let (total, max_updated): (i64, Option<OffsetDateTime>) = sqlx::query_as(
        r#"SELECT COUNT(*), MAX(c.updated_at) FROM chat_channels c
           JOIN chat_channel_members m ON m.channel_id = c.id
           WHERE c.tenant_id = $1 AND m.user_id = $2 AND c.is_archived = FALSE"#,
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let rows = ChannelRepo::new(pool).list_for_user(ctx.tenant_id, ctx.user_id).await?;
    let mut resp = (
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(rows),
    ).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    req_headers: axum::http::HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let ch = repo.get(ctx.tenant_id, id).await
        .map_err(|_| ChatError::ChannelNotFound(id))?;
    let etag = format!("\"{}-{}\"", ch.updated_at.unix_timestamp(), ch.id);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    let lm = ch.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if ch.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(ch).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

async fn archive(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    let ch   = repo.get(ctx.tenant_id, id).await
        .map_err(|_| ChatError::ChannelNotFound(id))?;
    // Archive allowed for the original creator OR any owner/admin member.
    if ch.created_by != ctx.user_id {
        match repo.member_role(ctx.tenant_id, id, ctx.user_id).await? {
            Some(MemberRole::Owner) | Some(MemberRole::Admin) => {}
            _ => return Err(ChatError::Forbidden),
        }
    }
    repo.archive(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct AddMemberBody {
    pub user_id: Uuid,
    pub role:    Option<MemberRole>,
}

async fn add_member(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberBody>,
) -> Result<StatusCode> {
    let pool   = state.db_or_unavailable()?;
    let matrix = state.matrix_or_unavailable()?;
    let repo   = ChannelRepo::new(pool);
    // Invitation requires owner/admin role (RBAC hardening).
    match repo.member_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(MemberRole::Owner) | Some(MemberRole::Admin) => {}
        Some(_) => return Err(ChatError::Forbidden),
        None    => return Err(ChatError::NotMember),
    }
    let ch = repo.get(ctx.tenant_id, id).await
        .map_err(|_| ChatError::ChannelNotFound(id))?;

    let acting_as      = matrix.mxid_for(ctx.user_id);
    let invitee_mxid   = matrix.mxid_for(body.user_id);
    matrix.invite_user(&acting_as, &ch.matrix_room_id, &invitee_mxid).await?;

    repo.add_member(
        ctx.tenant_id, id, body.user_id,
        body.role.unwrap_or(MemberRole::Member)
    ).await?;
    Ok(StatusCode::CREATED)
}

async fn list_members(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let repo = ChannelRepo::new(pool);
    if !repo.is_member(ctx.tenant_id, id, ctx.user_id).await? {
        return Err(ChatError::NotMember);
    }
    let max_joined: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(joined_at) FROM chat_channel_members WHERE tenant_id = $1 AND channel_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    if let Some(ts) = max_joined {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let rows = repo.list_members(ctx.tenant_id, id).await?;
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_joined {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_body_deser_minimal() {
        let json = r#"{"name":"general"}"#;
        let b: CreateBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.name, "general");
        assert!(b.topic.is_none());
        assert!(b.kind.is_none());
        assert!(b.invite.is_empty());
    }

    #[test]
    fn add_member_body_deser_role_optional() {
        let uid = Uuid::new_v4();
        let json = format!(r#"{{"user_id":"{uid}"}}"#);
        let b: AddMemberBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.user_id, uid);
        assert!(b.role.is_none());
    }
}

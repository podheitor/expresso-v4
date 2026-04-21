//! Meetings REST API.
//!
//! POST   /api/v1/meetings                    → create meeting + moderator JWT
//! GET    /api/v1/meetings                    → list current user's meetings
//! GET    /api/v1/meetings/:id                → meeting detail (participant-only)
//! DELETE /api/v1/meetings/:id                → archive (creator OR moderator)
//! POST   /api/v1/meetings/:id/tokens         → mint a JWT for the caller (or for
//!                                              a target user, moderator-only)
//! POST   /api/v1/meetings/:id/participants   → add participant (moderator-only)
//! GET    /api/v1/meetings/:id/participants   → list participants

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Meeting, MeetingParticipant, MeetingRepo, NewMeeting, ParticipantRole};
use crate::error::{MeetError, Result};
use crate::jitsi::IssueRequest;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/meetings", post(create).get(list))
        .route("/api/v1/meetings/:id", get(get_one).delete(archive))
        .route("/api/v1/meetings/:id/tokens", post(mint_token))
        .route("/api/v1/meetings/:id/participants", post(add_participant).get(list_participants))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub title:         String,
    pub channel_id:    Option<Uuid>,
    pub room_name:     Option<String>,            // let client override slug
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub scheduled_for: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub ends_at:       Option<OffsetDateTime>,
    pub is_recurring:  Option<bool>,
    pub lobby_enabled: Option<bool>,
    pub password:      Option<String>,
    /// Extra participants (beyond the creator) to pre-add as participants.
    #[serde(default)]
    pub invite:        Vec<Uuid>,
    /// Request moderator JWT in the response body (default true).
    pub return_token:  Option<bool>,
    /// Whether the moderator may start a recording (default false).
    pub allow_recording: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub meeting:  Meeting,
    pub join_url: Option<String>,
    pub token:    Option<String>,
    pub expires_at_epoch: Option<i64>,
    pub domain:   String,
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<CreateResponse>)> {
    if body.title.trim().is_empty() {
        return Err(MeetError::BadRequest("title required".into()));
    }
    let pool  = state.db_or_unavailable()?;
    let jitsi = state.jitsi_or_unavailable()?;

    let room_name = body.room_name.clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| jitsi.generate_room_name());

    let repo = MeetingRepo::new(pool);
    let meeting = repo.create(ctx.tenant_id, ctx.user_id, NewMeeting {
        room_name:     room_name.clone(),
        title:         body.title,
        channel_id:    body.channel_id,
        scheduled_for: body.scheduled_for,
        ends_at:       body.ends_at,
        is_recurring:  body.is_recurring,
        lobby_enabled: body.lobby_enabled,
        password:      body.password,
    }).await?;

    for u in body.invite {
        repo.add_participant(ctx.tenant_id, meeting.id, u, ParticipantRole::Participant).await?;
    }

    let mut resp = CreateResponse {
        meeting,
        join_url: None,
        token:    None,
        expires_at_epoch: None,
        domain:   jitsi.domain().to_string(),
    };
    if body.return_token.unwrap_or(true) {
        let t = jitsi.mint(&IssueRequest {
            room:           &room_name,
            user_id:        ctx.user_id,
            display_name:   &ctx.display_name,
            email:          &ctx.email,
            moderator:      true,
            allow_recording: body.allow_recording.unwrap_or(false),
        })?;
        resp.join_url = Some(t.join_url);
        resp.token    = Some(t.token);
        resp.expires_at_epoch = Some(t.expires_at_epoch);
    }
    Ok((StatusCode::CREATED, Json(resp)))
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<Meeting>>> {
    let pool = state.db_or_unavailable()?;
    let rows = MeetingRepo::new(pool).list_for_user(ctx.tenant_id, ctx.user_id).await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Meeting>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    if repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?.is_none() {
        return Err(MeetError::NotParticipant);
    }
    let m = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;
    Ok(Json(m))
}

async fn archive(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let m    = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;
    // Allowed: original creator OR any moderator participant.
    if m.created_by != ctx.user_id {
        match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
            Some(ParticipantRole::Moderator) => {}
            _ => return Err(MeetError::Forbidden),
        }
    }
    repo.archive(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct TokenBody {
    /// When set (moderator-only), mint for another user in the tenant.
    pub user_id:       Option<Uuid>,
    /// Override moderator flag. Only a moderator may set this to true.
    pub as_moderator:  Option<bool>,
    /// Recording flag (moderator-only).
    pub allow_recording: Option<bool>,
    /// Override display name (e.g. guest tokens). Defaults to caller's.
    pub display_name:  Option<String>,
    pub email:         Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub room:     String,
    pub domain:   String,
    pub token:    String,
    pub join_url: String,
    pub expires_at_epoch: i64,
    pub moderator: bool,
}

async fn mint_token(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<TokenBody>,
) -> Result<Json<TokenResponse>> {
    let pool  = state.db_or_unavailable()?;
    let jitsi = state.jitsi_or_unavailable()?;
    let repo  = MeetingRepo::new(pool);

    let caller_role = repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;
    let caller_is_mod = matches!(caller_role, ParticipantRole::Moderator);

    // Minting for someone else = moderator-only; same for as_moderator=true
    // and for allow_recording=true.
    let target = body.user_id.unwrap_or(ctx.user_id);
    if target != ctx.user_id && !caller_is_mod {
        return Err(MeetError::Forbidden);
    }
    let want_moderator = body.as_moderator.unwrap_or(target == ctx.user_id && caller_is_mod);
    if want_moderator && !caller_is_mod {
        return Err(MeetError::Forbidden);
    }
    let want_recording = body.allow_recording.unwrap_or(false);
    if want_recording && !caller_is_mod {
        return Err(MeetError::Forbidden);
    }

    // When minting for another user, they must already be a participant
    // (moderator must add them first). Keeps the ACL surface tight.
    if target != ctx.user_id
        && repo.participant_role(ctx.tenant_id, id, target).await?.is_none()
    {
        return Err(MeetError::BadRequest("target is not a participant".into()));
    }

    let m = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;

    let display_name = body.display_name.as_deref().unwrap_or(&ctx.display_name);
    let email        = body.email.as_deref().unwrap_or(&ctx.email);

    let issued = jitsi.mint(&IssueRequest {
        room:            &m.room_name,
        user_id:         target,
        display_name,
        email,
        moderator:       want_moderator,
        allow_recording: want_recording,
    })?;

    Ok(Json(TokenResponse {
        room:      issued.room,
        domain:    issued.domain,
        token:     issued.token,
        join_url:  issued.join_url,
        expires_at_epoch: issued.expires_at_epoch,
        moderator: want_moderator,
    }))
}

#[derive(Debug, Deserialize)]
pub struct AddParticipantBody {
    pub user_id: Uuid,
    pub role:    Option<ParticipantRole>,
}

async fn add_participant(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<AddParticipantBody>,
) -> Result<(StatusCode, Json<Value>)> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    // Ensure meeting exists (and is tenant-scoped).
    let _ = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;

    let role = body.role.unwrap_or(ParticipantRole::Participant);
    repo.add_participant(ctx.tenant_id, id, body.user_id, role).await?;
    Ok((StatusCode::CREATED, Json(json!({"added": body.user_id}))))
}

async fn list_participants(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<MeetingParticipant>>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    if repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?.is_none() {
        return Err(MeetError::NotParticipant);
    }
    let rows = repo.list_participants(ctx.tenant_id, id).await?;
    Ok(Json(rows))
}

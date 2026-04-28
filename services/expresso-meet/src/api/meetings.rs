//! Meetings REST API.
//!
//! POST   /api/v1/meetings                    → create meeting + moderator JWT
//! GET    /api/v1/meetings                    → list current user's meetings (?archived=true for archived)
//! GET    /api/v1/meetings/:id                → meeting detail (participant-only)
//! PATCH  /api/v1/meetings/:id                → update title/schedule/lobby/password (moderator-only)
//! DELETE /api/v1/meetings/:id                → archive (creator OR moderator)
//! POST   /api/v1/meetings/:id/restore        → restore archived meeting (creator OR moderator)
//! POST   /api/v1/meetings/:id/tokens         → mint a JWT for the caller (or for
//!                                              a target user, moderator-only)
//! POST   /api/v1/meetings/:id/participants   → add participant (moderator-only)
//! GET    /api/v1/meetings/:id/participants   → list participants
//! DELETE /api/v1/meetings/:id/participants/:user_id → remove participant (moderator-only)
//! GET    /api/v1/meetings/:id/participants/count    → participant count (participant-only)
//! GET    /api/v1/meetings/:id/participants/:user_id → get participant detail (participant-only)
//! PATCH  /api/v1/meetings/:id/participants/:user_id → promote/demote role (moderator-only)

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::types::Json as SqlxJson;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Meeting, MeetingParticipant, MeetingRepo, NewMeeting, ParticipantRole};
use crate::error::{MeetError, Result};
use crate::jitsi::IssueRequest;
use crate::state::AppState;
use crate::webhook;

/// Cap pra title — vai pro DB e UI; não tem necessidade de ser longo.
pub const MAX_TITLE_BYTES: usize = 200;

/// Cap pra room_name — vira parte da URL do Jitsi e claim `room` no JWT.
/// Jitsi tolera nomes longos mas isso infla token e cria URLs absurdas.
pub const MAX_ROOM_NAME_BYTES: usize = 64;

/// Cap pra password do meeting — vai pro DB. Não é hashed (compartilhado
/// com participantes), então não-secreto, mas cap previne payload abuso.
pub const MAX_PASSWORD_BYTES: usize = 128;

/// Cap pro display_name/email override em mint_token — vão direto pro JWT.
/// Nome longo infla token; email longo é abuso (RFC 5321 já limita 254).
pub const MAX_DISPLAY_NAME_BYTES: usize = 100;
pub const MAX_EMAIL_BYTES: usize = 254;

/// Cap pra invite list. Cada UUID dispara um INSERT de participante;
/// 100 cobre uso real (call-de-equipe gigante) e bloqueia loop de abuso.
pub const MAX_INVITE_LEN: usize = 100;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/meetings", post(create).get(list))
        .route("/api/v1/meetings/:id", get(get_one).patch(update).delete(archive))
        .route("/api/v1/meetings/:id/restore", post(restore))
        .route("/api/v1/meetings/:id/tokens", post(mint_token))
        .route("/api/v1/meetings/:id/participants", post(add_participant).get(list_participants))
        .route("/api/v1/meetings/:id/participants/count", get(count_participants))
        .route("/api/v1/meetings/:id/participants/:user_id", get(get_participant).delete(remove_participant).patch(update_participant_role))
        .route("/api/v1/meetings/:id/recording/start", post(recording_start))
        .route("/api/v1/meetings/:id/recording/stop",  post(recording_stop))
        .route("/api/v1/meetings/:id/chat",            get(get_chat))
        .route("/api/v1/meetings/:id/lobby",           get(list_lobby).post(join_lobby))
        .route("/api/v1/meetings/:id/lobby/approve/:user_id", post(approve_lobby))
        .route("/api/v1/meetings/:id/lobby/:user_id",  delete(remove_from_lobby))
        .route("/api/v1/meetings/:id/polls",           post(create_poll).get(list_polls))
        .route("/api/v1/meetings/:id/polls/:poll_id",  get(get_poll).delete(close_poll))
        .route("/api/v1/meetings/:id/polls/:poll_id/vote", post(cast_vote))
        .route("/api/v1/meetings/:id/breakouts",            post(create_breakout).get(list_breakouts))
        .route("/api/v1/meetings/:id/breakouts/:room_id",   get(get_breakout).delete(delete_breakout))
        .route("/api/v1/meetings/:id/breakouts/:room_id/participants", post(assign_breakout_participant).delete(remove_breakout_participant))
        .route("/api/v1/meetings/:id/transcript",           post(create_transcript).get(list_transcripts))
        .route("/api/v1/meetings/:id/transcript/search",   get(search_transcripts))
        .route("/api/v1/meetings/:id/recordings",          post(create_recording).get(list_recordings))
        .route("/api/v1/meetings/:id/recordings/:rec_id",  get(get_recording).delete(delete_recording))
}

/// Aceita apenas chars URL-safe pra room_name (ASCII alphanum + `-` + `_`).
/// Bloqueia `/`, `..`, ` ` e qualquer coisa que vire confuso na join URL ou
/// que precise URL-encoding no claim do JWT. Jitsi internamente normaliza,
/// mas defesa-em-profundidade aqui evita surpresas downstream.
fn valid_room_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ROOM_NAME_BYTES
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
    if body.title.len() > MAX_TITLE_BYTES {
        return Err(MeetError::BadRequest(format!(
            "title too long: {} bytes (max {})", body.title.len(), MAX_TITLE_BYTES
        )));
    }
    if let Some(p) = body.password.as_ref() {
        if p.len() > MAX_PASSWORD_BYTES {
            return Err(MeetError::BadRequest(format!(
                "password too long: {} bytes (max {})", p.len(), MAX_PASSWORD_BYTES
            )));
        }
    }
    if body.invite.len() > MAX_INVITE_LEN {
        return Err(MeetError::BadRequest(format!(
            "too many invitees: {} (max {})", body.invite.len(), MAX_INVITE_LEN
        )));
    }
    let pool  = state.db_or_unavailable()?;
    let jitsi = state.jitsi_or_unavailable()?;

    let room_name = body.room_name.clone()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| jitsi.generate_room_name());
    if !valid_room_name(&room_name) {
        return Err(MeetError::BadRequest(
            "invalid room_name: ASCII alphanumeric + '-' '_' only, max 64 bytes".into()
        ));
    }

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

    webhook::dispatch(
        state.webhook(),
        "meeting.created",
        ctx.tenant_id,
        serde_json::to_value(&meeting).unwrap_or_default(),
    );

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

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    archived: bool,
    /// Only meetings with scheduled_for >= after (RFC 3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    after:    Option<OffsetDateTime>,
    /// Only meetings with scheduled_for <= before (RFC 3339).
    #[serde(default, with = "time::serde::rfc3339::option")]
    before:   Option<OffsetDateTime>,
}

async fn list(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<ListQuery>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let archived_flag = if q.archived { "TRUE" } else { "FALSE" };
    let max_updated: Option<OffsetDateTime> = sqlx::query_scalar(&format!(
        "SELECT MAX(m.updated_at) FROM meetings m \
         JOIN meeting_participants mp ON mp.meeting_id = m.id \
         WHERE m.tenant_id = $1 AND mp.user_id = $2 AND m.is_archived = {archived_flag}",
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
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
    let repo = MeetingRepo::new(pool);
    let rows = if q.archived {
        repo.list_archived_for_user(ctx.tenant_id, ctx.user_id).await?
    } else if q.after.is_some() || q.before.is_some() {
        repo.list_for_user_filtered(ctx.tenant_id, ctx.user_id, q.after, q.before).await?
    } else {
        repo.list_for_user(ctx.tenant_id, ctx.user_id).await?
    };
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn get_one(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    if repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?.is_none() {
        return Err(MeetError::NotParticipant);
    }
    let m = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;
    let etag = format!("\"{}-{}\"", m.updated_at.unix_timestamp(), m.id);
    let lm   = m.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if m.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(m).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    pub title:         Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub scheduled_for: Option<OffsetDateTime>,
    pub clear_scheduled_for: Option<bool>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub ends_at:       Option<OffsetDateTime>,
    pub clear_ends_at: Option<bool>,
    pub lobby_enabled: Option<bool>,
    pub password:      Option<String>,
    pub clear_password: Option<bool>,
    pub is_recurring:  Option<bool>,
}

async fn update(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<UpdateBody>,
) -> Result<Json<Meeting>> {
    if let Some(ref t) = body.title {
        if t.trim().is_empty() {
            return Err(MeetError::BadRequest("title cannot be empty".into()));
        }
        if t.len() > MAX_TITLE_BYTES {
            return Err(MeetError::BadRequest(format!(
                "title too long: {} bytes (max {})", t.len(), MAX_TITLE_BYTES
            )));
        }
    }

    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);

    // Only moderators may update meeting details.
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        _ => return Err(MeetError::Forbidden),
    }

    let scheduled_for = if body.clear_scheduled_for.unwrap_or(false) {
        Some(None)
    } else {
        body.scheduled_for.map(Some)
    };
    let ends_at = if body.clear_ends_at.unwrap_or(false) {
        Some(None)
    } else {
        body.ends_at.map(Some)
    };
    let password = if body.clear_password.unwrap_or(false) {
        Some(None)
    } else {
        body.password.map(Some)
    };

    let updated = repo.update(
        ctx.tenant_id, id,
        body.title,
        scheduled_for,
        ends_at,
        body.lobby_enabled,
        password,
        body.is_recurring,
    ).await?;

    match updated {
        Some(m) => Ok(Json(m)),
        None    => Err(MeetError::MeetingNotFound(id)),
    }
}

/// POST /api/v1/meetings/:id/restore — reativa reunião arquivada (creator ou moderador).
async fn restore(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Meeting>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let m = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;
    if m.created_by != ctx.user_id {
        match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
            Some(ParticipantRole::Moderator) => {}
            _ => return Err(MeetError::Forbidden),
        }
    }
    match repo.restore(ctx.tenant_id, id).await? {
        Some(m) => {
            webhook::dispatch(
                state.webhook(),
                "meeting.restored",
                ctx.tenant_id,
                serde_json::to_value(&m).unwrap_or_default(),
            );
            Ok(Json(m))
        }
        None => Err(MeetError::MeetingNotFound(id)),
    }
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
    webhook::dispatch(
        state.webhook(),
        "meeting.archived",
        ctx.tenant_id,
        serde_json::to_value(&m).unwrap_or_default(),
    );
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

    if let Some(d) = body.display_name.as_deref() {
        if d.len() > MAX_DISPLAY_NAME_BYTES {
            return Err(MeetError::BadRequest(format!(
                "display_name too long: {} bytes (max {})", d.len(), MAX_DISPLAY_NAME_BYTES
            )));
        }
    }
    if let Some(e) = body.email.as_deref() {
        if e.len() > MAX_EMAIL_BYTES {
            return Err(MeetError::BadRequest(format!(
                "email too long: {} bytes (max {})", e.len(), MAX_EMAIL_BYTES
            )));
        }
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

/// GET /api/v1/meetings/:id/participants/count — total participant count (caller must be participant).
async fn count_participants(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    if repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?.is_none() {
        return Err(MeetError::NotParticipant);
    }
    let n = repo.count_participants(ctx.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "count": n })))
}

/// GET /api/v1/meetings/:id/participants/:user_id — detail of one participant (must be a participant).
async fn get_participant(
    State(state):        State<AppState>,
    ctx:                 RequestCtx,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    req_headers:         HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    if repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?.is_none() {
        return Err(MeetError::NotParticipant);
    }
    let p = repo.get_participant(ctx.tenant_id, id, user_id).await?
        .ok_or(MeetError::BadRequest("participant not found".into()))?;
    let etag = format!("\"{}-{}\"", p.invited_at.unix_timestamp(), p.user_id);
    let lm   = p.invited_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if p.invited_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(p).into_response();
    resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

/// DELETE /api/v1/meetings/:id/participants/:user_id — moderator-only; creator cannot be removed.
async fn remove_participant(
    State(state):        State<AppState>,
    ctx:                 RequestCtx,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);

    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }

    let m = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;
    if m.created_by == user_id {
        return Err(MeetError::BadRequest("cannot remove the meeting creator".into()));
    }

    repo.remove_participant(ctx.tenant_id, id, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct UpdateRoleBody {
    role: ParticipantRole,
}

/// PATCH /api/v1/meetings/:id/participants/:user_id — promote or demote a participant (moderator-only).
/// The meeting creator's role cannot be demoted below moderator.
async fn update_participant_role(
    State(state):        State<AppState>,
    ctx:                 RequestCtx,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
    Json(body):          Json<UpdateRoleBody>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);

    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }

    // Creator must always remain a moderator.
    let m = repo.get(ctx.tenant_id, id).await
        .map_err(|_| MeetError::MeetingNotFound(id))?;
    if m.created_by == user_id && body.role != ParticipantRole::Moderator {
        return Err(MeetError::BadRequest("cannot demote the meeting creator".into()));
    }

    let found = repo.set_participant_role(ctx.tenant_id, id, user_id, body.role).await?;
    if !found {
        return Err(MeetError::BadRequest("participant not found".into()));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct ListParticipantsQuery {
    #[serde(default = "default_participants_limit")]
    limit:  i64,
    #[serde(default)]
    offset: i64,
}
fn default_participants_limit() -> i64 { 50 }

async fn list_participants(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(q):     Query<ListParticipantsQuery>,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    if repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?.is_none() {
        return Err(MeetError::NotParticipant);
    }
    let max_invited: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT MAX(invited_at) FROM meeting_participants WHERE tenant_id = $1 AND meeting_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap_or(None);
    if let Some(ts) = max_invited {
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
    let limit  = q.limit.clamp(1, 200);
    let offset = q.offset.max(0);
    let rows = repo.list_participants_paged(ctx.tenant_id, id, limit, offset).await?;
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_invited {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

/// POST /api/v1/meetings/:id/recording/start — mark recording as started (moderator-only).
/// Returns 409 if already recording, 404 if meeting not found or archived.
async fn recording_start(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<crate::domain::Meeting>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        _ => return Err(MeetError::Forbidden),
    }
    let m = repo.start_recording(ctx.tenant_id, id).await?
        .ok_or(MeetError::Conflict("meeting is already recording or not found".into()))?;
    tracing::info!(target: "audit",
        event = "meet.recording.started",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, meeting_id = %id);
    Ok(Json(m))
}

/// POST /api/v1/meetings/:id/recording/stop — mark recording as stopped (moderator-only).
/// Returns 409 if not currently recording, 404 if meeting not found or archived.
async fn recording_stop(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<crate::domain::Meeting>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        _ => return Err(MeetError::Forbidden),
    }
    let m = repo.stop_recording(ctx.tenant_id, id).await?
        .ok_or(MeetError::Conflict("meeting is not recording or not found".into()))?;
    tracing::info!(target: "audit",
        event = "meet.recording.stopped",
        tenant_id = %ctx.tenant_id, user_id = %ctx.user_id, meeting_id = %id);
    Ok(Json(m))
}

/// GET /api/v1/meetings/:id/chat — return the chat channel linked to this meeting.
///
/// Returns the channel metadata from `chat_channels` so the client can connect
/// to expresso-chat for message history. 404 when the meeting has no channel.
async fn get_chat(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Value>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    // Must be a participant to access chat.
    repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;

    let meeting = repo.get(ctx.tenant_id, id).await?;
    let channel_id = meeting.channel_id.ok_or_else(|| MeetError::BadRequest(
        "this meeting has no linked chat channel".into(),
    ))?;

    let row: Option<(Uuid, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, matrix_room_id, name, topic \
         FROM chat_channels \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(channel_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(MeetError::Database)?;

    let (ch_id, matrix_room_id, name, topic) = row.ok_or(MeetError::MeetingNotFound(id))?;
    Ok(Json(json!({
        "channel_id":     ch_id,
        "matrix_room_id": matrix_room_id,
        "name":           name,
        "topic":          topic,
    })))
}

/// GET /api/v1/meetings/:id/lobby — list users waiting in the waiting room.
///
/// Requires moderator role. Returns waiting users ordered by `joined_at`.
async fn list_lobby(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Value>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let role = repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;
    if role != ParticipantRole::Moderator {
        return Err(MeetError::Forbidden);
    }
    // Verify meeting exists and lobby is enabled.
    let meeting = repo.get(ctx.tenant_id, id).await?;
    if !meeting.lobby_enabled {
        return Err(MeetError::BadRequest("lobby is not enabled for this meeting".into()));
    }

    #[derive(sqlx::FromRow, Serialize)]
    struct LobbyEntry {
        user_id:   Uuid,
        #[serde(with = "time::serde::rfc3339")]
        joined_at: OffsetDateTime,
    }
    let entries: Vec<LobbyEntry> = sqlx::query_as(
        "SELECT user_id, joined_at FROM meeting_lobby \
         WHERE meeting_id = $1 AND tenant_id = $2 \
         ORDER BY joined_at ASC",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await
    .map_err(MeetError::Database)?;

    Ok(Json(json!({ "meeting_id": id, "waiting": entries })))
}

/// POST /api/v1/meetings/:id/lobby — join the waiting room.
///
/// Any authenticated user (not necessarily a participant yet) can knock.
/// Idempotent: inserting twice just updates joined_at via ON CONFLICT DO NOTHING.
async fn join_lobby(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<(StatusCode, Json<Value>)> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let meeting = repo.get(ctx.tenant_id, id).await?;
    if !meeting.lobby_enabled {
        return Err(MeetError::BadRequest("lobby is not enabled for this meeting".into()));
    }
    sqlx::query(
        "INSERT INTO meeting_lobby (meeting_id, tenant_id, user_id) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(pool)
    .await
    .map_err(MeetError::Database)?;

    tracing::info!(target: "audit",
        event = "meet.lobby.join",
        tenant_id = %ctx.tenant_id, meeting_id = %id, user_id = %ctx.user_id);
    Ok((StatusCode::CREATED, Json(json!({ "meeting_id": id, "status": "waiting" }))))
}

/// POST /api/v1/meetings/:id/lobby/approve/:user_id — admit a user from the waiting room.
///
/// Moderator-only. Removes the user from lobby and adds them as a participant.
async fn approve_lobby(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let role = repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;
    if role != ParticipantRole::Moderator {
        return Err(MeetError::Forbidden);
    }
    // Remove from lobby.
    let r = sqlx::query(
        "DELETE FROM meeting_lobby WHERE meeting_id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(MeetError::Database)?;

    if r.rows_affected() == 0 {
        return Err(MeetError::BadRequest("user is not in the waiting room".into()));
    }
    // Add as participant (idempotent — skip if already a participant).
    sqlx::query(
        "INSERT INTO meeting_participants (meeting_id, tenant_id, user_id, role) \
         VALUES ($1, $2, $3, 'participant') ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(MeetError::Database)?;

    tracing::info!(target: "audit",
        event = "meet.lobby.approve",
        tenant_id = %ctx.tenant_id, meeting_id = %id,
        moderator_id = %ctx.user_id, approved_user_id = %user_id);
    Ok(Json(json!({ "meeting_id": id, "user_id": user_id, "status": "admitted" })))
}

/// DELETE /api/v1/meetings/:id/lobby/:user_id — dismiss a user from the waiting room.
///
/// Moderator-only. Removes the user from lobby without admitting them.
async fn remove_from_lobby(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let role = repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;
    if role != ParticipantRole::Moderator {
        return Err(MeetError::Forbidden);
    }
    sqlx::query(
        "DELETE FROM meeting_lobby WHERE meeting_id = $1 AND tenant_id = $2 AND user_id = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(MeetError::Database)?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── Poll / Vote ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CreatePollBody {
    question: String,
    /// List of option labels, min 2.
    options:  Vec<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct Poll {
    id:         Uuid,
    meeting_id: Uuid,
    created_by: Uuid,
    question:   String,
    options:    SqlxJson<Vec<String>>,
    is_closed:  bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct CastVoteBody {
    /// Zero-based index into the poll's options array.
    option_idx: i32,
}

/// POST /api/v1/meetings/:id/polls — create a poll (moderator-only).
async fn create_poll(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CreatePollBody>,
) -> Result<(StatusCode, Json<Poll>)> {
    if body.question.trim().is_empty() {
        return Err(MeetError::BadRequest("question must not be empty".into()));
    }
    if body.options.len() < 2 {
        return Err(MeetError::BadRequest("polls require at least 2 options".into()));
    }
    if body.options.len() > 20 {
        return Err(MeetError::BadRequest("polls allow at most 20 options".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let role = repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;
    if role != ParticipantRole::Moderator {
        return Err(MeetError::Forbidden);
    }
    // Ensure meeting exists and is not archived.
    let meeting = repo.get(ctx.tenant_id, id).await?;
    if meeting.is_archived {
        return Err(MeetError::BadRequest("cannot create poll in an archived meeting".into()));
    }
    let opts_json = serde_json::to_value(&body.options)
        .map_err(|e| MeetError::BadRequest(e.to_string()))?;
    let row: Poll = sqlx::query_as(
        "INSERT INTO meeting_polls (meeting_id, tenant_id, created_by, question, options) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, meeting_id, created_by, question, options, is_closed, created_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&body.question)
    .bind(&opts_json)
    .fetch_one(pool)
    .await
    .map_err(MeetError::Database)?;

    tracing::info!(target: "audit",
        event = "meet.poll.created",
        tenant_id = %ctx.tenant_id, meeting_id = %id, poll_id = %row.id);
    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /api/v1/meetings/:id/polls — list all polls for this meeting (participant-only).
async fn list_polls(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Value>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;

    let polls: Vec<Poll> = sqlx::query_as(
        "SELECT id, meeting_id, created_by, question, options, is_closed, created_at \
         FROM meeting_polls WHERE meeting_id = $1 AND tenant_id = $2 \
         ORDER BY created_at ASC",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await
    .map_err(MeetError::Database)?;

    Ok(Json(json!({ "meeting_id": id, "polls": polls })))
}

/// GET /api/v1/meetings/:id/polls/:poll_id — poll detail with vote tallies (participant-only).
async fn get_poll(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, poll_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Value>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;

    let poll: Option<Poll> = sqlx::query_as(
        "SELECT id, meeting_id, created_by, question, options, is_closed, created_at \
         FROM meeting_polls WHERE id = $1 AND meeting_id = $2 AND tenant_id = $3",
    )
    .bind(poll_id)
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(MeetError::Database)?;
    let poll = poll.ok_or(MeetError::MeetingNotFound(poll_id))?;

    // Tally votes per option.
    #[derive(sqlx::FromRow)]
    struct Tally { option_idx: i32, cnt: i64 }
    let tallies: Vec<Tally> = sqlx::query_as(
        "SELECT option_idx, COUNT(*)::BIGINT AS cnt \
         FROM meeting_poll_votes WHERE poll_id = $1 \
         GROUP BY option_idx ORDER BY option_idx",
    )
    .bind(poll_id)
    .fetch_all(pool)
    .await
    .map_err(MeetError::Database)?;

    let mut tally_map: Vec<i64> = vec![0; poll.options.0.len()];
    for t in &tallies {
        let idx = t.option_idx as usize;
        if idx < tally_map.len() { tally_map[idx] = t.cnt; }
    }

    // Caller's own vote if any.
    let my_vote: Option<i32> = sqlx::query_scalar(
        "SELECT option_idx FROM meeting_poll_votes \
         WHERE poll_id = $1 AND user_id = $2",
    )
    .bind(poll_id)
    .bind(ctx.user_id)
    .fetch_optional(pool)
    .await
    .map_err(MeetError::Database)?;

    Ok(Json(json!({
        "poll":    poll,
        "tallies": tally_map,
        "my_vote": my_vote,
    })))
}

/// DELETE /api/v1/meetings/:id/polls/:poll_id — close a poll (moderator-only).
///
/// Closing prevents further votes but retains results.
async fn close_poll(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, poll_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    let role = repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;
    if role != ParticipantRole::Moderator {
        return Err(MeetError::Forbidden);
    }
    let r = sqlx::query(
        "UPDATE meeting_polls SET is_closed = true \
         WHERE id = $1 AND meeting_id = $2 AND tenant_id = $3",
    )
    .bind(poll_id)
    .bind(id)
    .bind(ctx.tenant_id)
    .execute(pool)
    .await
    .map_err(MeetError::Database)?;

    if r.rows_affected() == 0 {
        return Err(MeetError::MeetingNotFound(poll_id));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/v1/meetings/:id/polls/:poll_id/vote — cast or change vote (participant-only).
///
/// Uses UPSERT: voting again with a different option_idx replaces the previous vote.
async fn cast_vote(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, poll_id)): Path<(Uuid, Uuid)>,
    Json(body):   Json<CastVoteBody>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    repo.participant_role(ctx.tenant_id, id, ctx.user_id).await?
        .ok_or(MeetError::NotParticipant)?;

    // Load poll — verify it's open and option is valid.
    let poll: Option<Poll> = sqlx::query_as(
        "SELECT id, meeting_id, created_by, question, options, is_closed, created_at \
         FROM meeting_polls WHERE id = $1 AND meeting_id = $2 AND tenant_id = $3",
    )
    .bind(poll_id)
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await
    .map_err(MeetError::Database)?;
    let poll = poll.ok_or(MeetError::MeetingNotFound(poll_id))?;

    if poll.is_closed {
        return Err(MeetError::BadRequest("this poll is closed".into()));
    }
    if body.option_idx < 0 || (body.option_idx as usize) >= poll.options.0.len() {
        return Err(MeetError::BadRequest(format!(
            "option_idx {} out of range (0–{})",
            body.option_idx,
            poll.options.0.len().saturating_sub(1),
        )));
    }

    sqlx::query(
        "INSERT INTO meeting_poll_votes (poll_id, tenant_id, user_id, option_idx) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (poll_id, user_id) DO UPDATE SET \
            option_idx = EXCLUDED.option_idx, voted_at = now()",
    )
    .bind(poll_id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(body.option_idx)
    .execute(pool)
    .await
    .map_err(MeetError::Database)?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_name_accepts_ascii_alphanum() {
        assert!(valid_room_name("standup-2026-04"));
        assert!(valid_room_name("room_42"));
        assert!(valid_room_name("ABC123"));
    }

    #[test]
    fn room_name_rejects_special_chars() {
        assert!(!valid_room_name(""));
        assert!(!valid_room_name("a/b"));        // path traversal
        assert!(!valid_room_name(".."));         // path traversal
        assert!(!valid_room_name("a b"));        // space
        assert!(!valid_room_name("café"));       // unicode (would need URL-encode)
        assert!(!valid_room_name("a\nb"));       // newline injection
    }

    #[test]
    fn room_name_rejects_oversize() {
        let s = "a".repeat(MAX_ROOM_NAME_BYTES + 1);
        assert!(!valid_room_name(&s));
    }

    #[test]
    fn room_name_accepts_boundary() {
        let s = "a".repeat(MAX_ROOM_NAME_BYTES);
        assert!(valid_room_name(&s));
    }

    #[test]
    fn caps_are_sane() {
        assert!(MAX_TITLE_BYTES >= 50);
        assert!(MAX_INVITE_LEN  >= 10);
        assert!(MAX_EMAIL_BYTES >= 254);  // RFC 5321 baseline
    }
}

// ── Breakout rooms ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
struct BreakoutRoom {
    id:         Uuid,
    meeting_id: Uuid,
    tenant_id:  Uuid,
    name:       String,
    created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
struct BreakoutParticipant {
    room_id:    Uuid,
    user_id:    Uuid,
    tenant_id:  Uuid,
    #[serde(with = "time::serde::rfc3339")]
    assigned_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct BreakoutBody {
    name: String,
}

#[derive(Debug, Deserialize)]
struct BreakoutParticipantBody {
    user_id: Uuid,
}

#[derive(Debug, Serialize)]
struct BreakoutRoomDetail {
    #[serde(flatten)]
    room:         BreakoutRoom,
    participants: Vec<Uuid>,
}

/// POST /api/v1/meetings/:id/breakouts — create sub-room (moderator-only)
async fn create_breakout(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<BreakoutBody>,
) -> Result<(StatusCode, Json<BreakoutRoom>)> {
    if body.name.trim().is_empty() {
        return Err(MeetError::BadRequest("name must not be empty".into()));
    }
    if body.name.len() > 100 {
        return Err(MeetError::BadRequest("name must be <= 100 characters".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    let room: BreakoutRoom = sqlx::query_as(
        "INSERT INTO meeting_breakout_rooms (meeting_id, tenant_id, name, created_by) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, meeting_id, tenant_id, name, created_by, created_at",
    )
    .bind(id).bind(ctx.tenant_id).bind(&body.name).bind(ctx.user_id)
    .fetch_one(pool)
    .await?;
    Ok((StatusCode::CREATED, Json(room)))
}

/// GET /api/v1/meetings/:id/breakouts — list breakout rooms with participants
async fn list_breakouts(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Vec<BreakoutRoomDetail>>> {
    let pool = state.db_or_unavailable()?;
    let _ = MeetingRepo::new(pool).get(ctx.tenant_id, id).await.map_err(|_| MeetError::MeetingNotFound(id))?;
    let rooms: Vec<BreakoutRoom> = sqlx::query_as(
        "SELECT id, meeting_id, tenant_id, name, created_by, created_at \
         FROM meeting_breakout_rooms \
         WHERE meeting_id = $1 AND tenant_id = $2 \
         ORDER BY created_at ASC",
    )
    .bind(id).bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;

    let mut result = Vec::with_capacity(rooms.len());
    for room in rooms {
        let participants: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT user_id FROM meeting_breakout_participants \
             WHERE room_id = $1 AND tenant_id = $2",
        )
        .bind(room.id).bind(ctx.tenant_id)
        .fetch_all(pool)
        .await?;
        result.push(BreakoutRoomDetail {
            participants: participants.into_iter().map(|(u,)| u).collect(),
            room,
        });
    }
    Ok(Json(result))
}

/// GET /api/v1/meetings/:id/breakouts/:room_id
async fn get_breakout(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, room_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<BreakoutRoomDetail>> {
    let pool = state.db_or_unavailable()?;
    let room: Option<BreakoutRoom> = sqlx::query_as(
        "SELECT id, meeting_id, tenant_id, name, created_by, created_at \
         FROM meeting_breakout_rooms \
         WHERE id = $1 AND meeting_id = $2 AND tenant_id = $3",
    )
    .bind(room_id).bind(id).bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    let room = room.ok_or(MeetError::NotFound)?;
    let participants: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM meeting_breakout_participants \
         WHERE room_id = $1 AND tenant_id = $2",
    )
    .bind(room.id).bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(BreakoutRoomDetail {
        participants: participants.into_iter().map(|(u,)| u).collect(),
        room,
    }))
}

/// DELETE /api/v1/meetings/:id/breakouts/:room_id (moderator-only)
async fn delete_breakout(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, room_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    let r = sqlx::query(
        "DELETE FROM meeting_breakout_rooms \
         WHERE id = $1 AND meeting_id = $2 AND tenant_id = $3",
    )
    .bind(room_id).bind(id).bind(ctx.tenant_id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(MeetError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/v1/meetings/:id/breakouts/:room_id/participants — assign participant (moderator-only)
async fn assign_breakout_participant(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, room_id)): Path<(Uuid, Uuid)>,
    Json(body):   Json<BreakoutParticipantBody>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    sqlx::query(
        "INSERT INTO meeting_breakout_participants (room_id, meeting_id, tenant_id, user_id) \
         VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
    )
    .bind(room_id).bind(id).bind(ctx.tenant_id).bind(body.user_id)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/meetings/:id/breakouts/:room_id/participants — remove participant (moderator-only)
async fn remove_breakout_participant(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, room_id)): Path<(Uuid, Uuid)>,
    Json(body):   Json<BreakoutParticipantBody>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    sqlx::query(
        "DELETE FROM meeting_breakout_participants \
         WHERE room_id = $1 AND meeting_id = $2 AND tenant_id = $3 AND user_id = $4",
    )
    .bind(room_id).bind(id).bind(ctx.tenant_id).bind(body.user_id)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── Transcript metadata ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
struct Transcript {
    pub id:         Uuid,
    pub meeting_id: Uuid,
    pub tenant_id:  Uuid,
    pub url:        String,
    pub language:   Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub starts_at:  Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ends_at:    Option<OffsetDateTime>,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct CreateTranscriptBody {
    url:       String,
    language:  Option<String>,
    starts_at: Option<OffsetDateTime>,
    ends_at:   Option<OffsetDateTime>,
}

/// GET /api/v1/meetings/:id/transcript — list transcripts (participant-only).
async fn list_transcripts(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Vec<Transcript>>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(_) => {}
        None    => return Err(MeetError::NotParticipant),
    }
    let rows: Vec<Transcript> = sqlx::query_as(
        "SELECT id, meeting_id, tenant_id, url, language, starts_at, ends_at, created_by, created_at \
         FROM meeting_transcripts \
         WHERE tenant_id = $1 AND meeting_id = $2 \
         ORDER BY created_at ASC",
    )
    .bind(ctx.tenant_id).bind(id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

/// POST /api/v1/meetings/:id/transcript — attach transcript metadata (moderator-only).
async fn create_transcript(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CreateTranscriptBody>,
) -> Result<(StatusCode, Json<Transcript>)> {
    if body.url.is_empty() {
        return Err(MeetError::BadRequest("url is required".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    let row: Transcript = sqlx::query_as(
        "INSERT INTO meeting_transcripts \
             (meeting_id, tenant_id, url, language, starts_at, ends_at, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, meeting_id, tenant_id, url, language, starts_at, ends_at, created_by, created_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&body.url)
    .bind(&body.language)
    .bind(body.starts_at)
    .bind(body.ends_at)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Debug, Deserialize)]
struct TranscriptSearchQuery {
    q: Option<String>,
}

/// GET /api/v1/meetings/:id/transcript/search?q= — fulltext search over transcript metadata (participant-only).
///
/// Matches against `url` and `language` fields using PostgreSQL ILIKE.
/// Returns the same `Transcript` rows as the listing endpoint, filtered by `q`.
async fn search_transcripts(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Query(qs):    Query<TranscriptSearchQuery>,
) -> Result<Json<Vec<Transcript>>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(_) => {}
        None    => return Err(MeetError::NotParticipant),
    }

    let pattern = qs.q.as_deref().unwrap_or("").trim().to_string();

    let rows: Vec<Transcript> = if pattern.is_empty() {
        sqlx::query_as(
            "SELECT id, meeting_id, tenant_id, url, language, starts_at, ends_at, created_by, created_at \
             FROM meeting_transcripts \
             WHERE tenant_id = $1 AND meeting_id = $2 \
             ORDER BY created_at ASC",
        )
        .bind(ctx.tenant_id)
        .bind(id)
        .fetch_all(pool)
        .await?
    } else {
        let like = format!("%{}%", pattern.replace('%', "\\%").replace('_', "\\_"));
        sqlx::query_as(
            "SELECT id, meeting_id, tenant_id, url, language, starts_at, ends_at, created_by, created_at \
             FROM meeting_transcripts \
             WHERE tenant_id = $1 AND meeting_id = $2 \
               AND (url ILIKE $3 OR language ILIKE $3) \
             ORDER BY created_at ASC",
        )
        .bind(ctx.tenant_id)
        .bind(id)
        .bind(&like)
        .fetch_all(pool)
        .await?
    };

    Ok(Json(rows))
}

// ── Recording metadata (sprint #413) ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
struct Recording {
    id:         Uuid,
    meeting_id: Uuid,
    tenant_id:  Uuid,
    url:        String,
    duration_s: Option<i32>,
    size_bytes: Option<i64>,
    format:     Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    starts_at:  Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    ends_at:    Option<OffsetDateTime>,
    created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct CreateRecordingBody {
    url:        String,
    duration_s: Option<i32>,
    size_bytes: Option<i64>,
    format:     Option<String>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    starts_at:  Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    ends_at:    Option<OffsetDateTime>,
}

const RECORDING_COLS: &str =
    "id, meeting_id, tenant_id, url, duration_s, size_bytes, format, starts_at, ends_at, created_by, created_at";

/// GET /api/v1/meetings/:id/recordings — list recording metadata (participant-only).
async fn list_recordings(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Vec<Recording>>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(_) => {}
        None    => return Err(MeetError::NotParticipant),
    }
    let rows: Vec<Recording> = sqlx::query_as(
        &format!("SELECT {RECORDING_COLS} FROM meeting_recordings \
                  WHERE tenant_id = $1 AND meeting_id = $2 ORDER BY created_at ASC"),
    )
    .bind(ctx.tenant_id).bind(id)
    .fetch_all(pool)
    .await?;
    Ok(Json(rows))
}

/// POST /api/v1/meetings/:id/recordings — attach recording metadata (moderator-only).
async fn create_recording(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CreateRecordingBody>,
) -> Result<(StatusCode, Json<Recording>)> {
    if body.url.is_empty() {
        return Err(MeetError::BadRequest("url is required".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    let row: Recording = sqlx::query_as(
        &format!("INSERT INTO meeting_recordings \
                      (meeting_id, tenant_id, url, duration_s, size_bytes, format, starts_at, ends_at, created_by) \
                  VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                  RETURNING {RECORDING_COLS}"),
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&body.url)
    .bind(body.duration_s)
    .bind(body.size_bytes)
    .bind(&body.format)
    .bind(body.starts_at)
    .bind(body.ends_at)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /api/v1/meetings/:id/recordings/:rec_id — get a specific recording (participant-only).
async fn get_recording(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, rec_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Recording>> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(_) => {}
        None    => return Err(MeetError::NotParticipant),
    }
    let row: Option<Recording> = sqlx::query_as(
        &format!("SELECT {RECORDING_COLS} FROM meeting_recordings \
                  WHERE tenant_id = $1 AND meeting_id = $2 AND id = $3"),
    )
    .bind(ctx.tenant_id).bind(id).bind(rec_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some(r) => Ok(Json(r)),
        None    => Err(MeetError::NotFound),
    }
}

/// DELETE /api/v1/meetings/:id/recordings/:rec_id — remove recording metadata (moderator-only).
async fn delete_recording(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, rec_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = MeetingRepo::new(pool);
    match repo.participant_role(ctx.tenant_id, id, ctx.user_id).await? {
        Some(ParticipantRole::Moderator) => {}
        Some(_) => return Err(MeetError::Forbidden),
        None    => return Err(MeetError::NotParticipant),
    }
    let r = sqlx::query(
        "DELETE FROM meeting_recordings WHERE tenant_id = $1 AND meeting_id = $2 AND id = $3",
    )
    .bind(ctx.tenant_id).bind(id).bind(rec_id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(MeetError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

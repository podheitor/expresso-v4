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
        .route("/api/v1/meetings/:id", get(get_one).delete(archive))
        .route("/api/v1/meetings/:id/tokens", post(mint_token))
        .route("/api/v1/meetings/:id/participants", post(add_participant).get(list_participants))
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

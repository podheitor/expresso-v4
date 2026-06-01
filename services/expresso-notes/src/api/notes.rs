//! Notes REST API.
//!
//! Routes (all scoped to the authenticated user):
//!   GET    /api/v1/notes[?archived=true][&notebook=:id|none]
//!                                          → list own notes (optionally by notebook)
//!   GET    /api/v1/notes/shared            → list notes shared with me
//!   POST   /api/v1/notes                    → create
//!   GET    /api/v1/notes/:id                → fetch one (own or shared)
//!   PATCH  /api/v1/notes/:id                → partial update
//!   DELETE /api/v1/notes/:id                → delete
//!
//! Notes are full-text indexed (`kind = "note"`) so they appear in unified
//! search alongside mail/drive/calendar/contacts/tasks.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::note::NotebookFilter;
use crate::domain::{NewNote, Note, NoteRepo, NotebookRepo, SharedNote, UpdateNote};
use crate::error::{NotesError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/notes", get(list).post(create))
        // `shared` is a static segment — must be registered before `:id` so it
        // doesn't get captured as a note id (matchit prefers static over param).
        .route("/api/v1/notes/shared", get(list_shared))
        .route(
            "/api/v1/notes/:id",
            patch(update).get(get_one).delete(delete),
        )
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    #[serde(default)]
    archived: bool,
    /// Filter by notebook: a uuid scopes to that notebook, `none` returns only
    /// loose notes, absent returns all. `notebook=none` is the "no notebook"
    /// sentinel since query strings can't carry a typed null.
    notebook: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Note>>> {
    let pool = state.db_or_unavailable()?;
    let filter = match q.notebook.as_deref() {
        None => NotebookFilter::Any,
        Some("none") => NotebookFilter::Loose,
        Some(s) => {
            let nb = Uuid::parse_str(s)
                .map_err(|_| NotesError::BadRequest("invalid notebook id".into()))?;
            NotebookFilter::In(nb)
        }
    };
    let notes = NoteRepo::new(pool)
        .list(ctx.tenant_id, ctx.user_id, q.archived, filter)
        .await?;
    Ok(Json(notes))
}

/// GET /api/v1/notes/shared — notes shared with the caller (a `notes_acl` grant
/// exists), each tagged with the caller's privilege. Own notes come from `list`.
async fn list_shared(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<SharedNote>>> {
    let pool = state.db_or_unavailable()?;
    let notes = NoteRepo::new(pool)
        .list_shared(ctx.tenant_id, ctx.user_id)
        .await?;
    Ok(Json(notes))
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewNote>,
) -> Result<(StatusCode, Json<Note>)> {
    let pool = state.db_or_unavailable()?;
    assert_owns_notebook(pool, &ctx, body.notebook_id).await?;
    let note = NoteRepo::new(pool)
        .create(ctx.tenant_id, ctx.user_id, body)
        .await?;
    index_note(&state, &note);
    Ok((StatusCode::CREATED, Json(note)))
}

/// Reject a note targeting a notebook the caller doesn't own — else the FK
/// would dangle or leak another user's notebook id. `None`/detach is always
/// allowed. The doubly-optional update value flattens to the inner target.
async fn assert_owns_notebook(
    pool: &expresso_core::DbPool,
    ctx: &RequestCtx,
    target: Option<Uuid>,
) -> Result<()> {
    let Some(nb) = target else { return Ok(()) };
    if NotebookRepo::new(pool)
        .owns(ctx.tenant_id, ctx.user_id, nb)
        .await?
    {
        Ok(())
    } else {
        Err(NotesError::BadRequest(format!("unknown notebook: {nb}")))
    }
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Note>> {
    let pool = state.db_or_unavailable()?;
    let repo = NoteRepo::new(pool);
    assert_can_read(&repo, ctx.tenant_id, id, ctx.user_id).await?;
    let note = repo.get(ctx.tenant_id, id).await?;
    Ok(Json(note))
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateNote>,
) -> Result<Json<Note>> {
    let pool = state.db_or_unavailable()?;
    let repo = NoteRepo::new(pool);
    assert_can_write(&repo, ctx.tenant_id, id, ctx.user_id).await?;
    // Only a present `notebook_id: <uuid>` needs a check; `null` (detach) and
    // absent both flatten to `None` here and are always allowed.
    assert_owns_notebook(pool, &ctx, body.notebook_id.flatten()).await?;
    let note = repo.update(ctx.tenant_id, id, body).await?;
    index_note(&state, &note);
    Ok(Json(note))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = NoteRepo::new(pool);
    assert_can_write(&repo, ctx.tenant_id, id, ctx.user_id).await?;
    repo.delete(ctx.tenant_id, id).await?;
    deindex_note(&state, id);
    Ok(StatusCode::NO_CONTENT)
}

/// Read gate: OWNER/READ/WRITE/ADMIN may view. Absence of any grant is a 404
/// (the note isn't visible to this user — don't leak its existence).
async fn assert_can_read(
    repo: &NoteRepo<'_>,
    tenant: Uuid,
    note_id: Uuid,
    user_id: Uuid,
) -> Result<()> {
    match repo.access_level(tenant, note_id, user_id).await? {
        Some(_) => Ok(()),
        None => Err(crate::error::NotesError::NoteNotFound(note_id)),
    }
}

/// Write gate: OWNER/WRITE/ADMIN may edit/delete; a READ grant is 403; no grant
/// is 404.
async fn assert_can_write(
    repo: &NoteRepo<'_>,
    tenant: Uuid,
    note_id: Uuid,
    user_id: Uuid,
) -> Result<()> {
    match repo
        .access_level(tenant, note_id, user_id)
        .await?
        .as_deref()
    {
        Some("OWNER" | "WRITE" | "ADMIN") => Ok(()),
        Some(_) => Err(crate::error::NotesError::Forbidden),
        None => Err(crate::error::NotesError::NoteNotFound(note_id)),
    }
}

/// Fire-and-forget index of a note (`kind = "note"`). No-op without search.
/// Title is the subject, body the searchable body. Mirrors the drive/contacts
/// indexers — a search outage never blocks a note write.
fn index_note(state: &AppState, n: &Note) {
    let search_url = state.search_url();
    if search_url.is_empty() {
        return;
    }
    let doc = serde_json::json!({
        "document_id": n.id.to_string(),
        "tenant_id":   n.tenant_id.to_string(),
        "subject":     n.title,
        "body":        n.body,
        "kind":        "note",
    });
    let url = format!("{}/api/v1/index", search_url);
    let token = state.search_token().to_string();
    tokio::spawn(async move {
        let mut req = reqwest::Client::new().post(url).json(&doc);
        if !token.is_empty() {
            req = req.bearer_auth(&token);
        }
        let _ = req.send().await;
    });
}

/// Remove a note from the index on delete (document_id is the note UUID).
fn deindex_note(state: &AppState, id: Uuid) {
    let search_url = state.search_url();
    if search_url.is_empty() {
        return;
    }
    let url = format!("{}/api/v1/index/{}", search_url, id);
    let token = state.search_token().to_string();
    tokio::spawn(async move {
        let mut req = reqwest::Client::new().delete(url);
        if !token.is_empty() {
            req = req.bearer_auth(&token);
        }
        let _ = req.send().await;
    });
}

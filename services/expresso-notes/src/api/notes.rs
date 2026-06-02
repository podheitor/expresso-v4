//! Notes REST API.
//!
//! Routes (all scoped to the authenticated user):
//!   GET    /api/v1/notes[?archived=true][&notebook=:id|none][&tag=:tag]
//!                                          → list own notes (by notebook or tag)
//!   GET    /api/v1/notes/shared            → list notes shared with me
//!   GET    /api/v1/notes/search?q=&limit=  → substring search own notes
//!   POST   /api/v1/notes                    → create
//!   GET    /api/v1/notes/:id                → fetch one (own or shared)
//!   PATCH  /api/v1/notes/:id                → partial update
//!   DELETE /api/v1/notes/:id                → delete
//!   GET    /api/v1/notes/:id/tags           → list the note's tags
//!   PUT    /api/v1/notes/:id/tags           → replace the note's tag set
//!   GET    /api/v1/notes/tags/stats         → tag usage counts (most-used first)
//!   PATCH  /api/v1/notes/tags/:tag          → rename a tag across own notes
//!   POST   /api/v1/notes/tags/:tag/merge    → merge a tag into another
//!   GET    /api/v1/notes/:id/versions       → list content history (newest first)
//!   POST   /api/v1/notes/:id/versions/:n/restore → restore that version's content
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
use crate::domain::{
    NewNote, Note, NoteRepo, NoteSnapshot, NoteTagRepo, NoteVersion, NoteVersionRepo, NotebookRepo,
    SharedNote, TagCount, TagPairCount, UpdateNote,
};
use crate::error::{NotesError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/notes", get(list).post(create))
        // `shared` is a static segment — must be registered before `:id` so it
        // doesn't get captured as a note id (matchit prefers static over param).
        .route("/api/v1/notes/shared", get(list_shared))
        // `search` static segment — also before `:id` (matchit static-first).
        .route("/api/v1/notes/search", get(search))
        // Tag-wide ops (`tags` static, before `:id`): stats, rename + merge
        // across all of the caller's notes. `stats` is static so matchit prefers
        // it over the `:tag` param on the same prefix.
        .route("/api/v1/notes/tags/stats", get(tag_stats))
        .route("/api/v1/notes/tags/co-occurrence", get(tag_co_occurrence))
        .route("/api/v1/notes/tags/:tag", patch(rename_tag))
        .route(
            "/api/v1/notes/tags/:tag/merge",
            axum::routing::post(merge_tag),
        )
        .route(
            "/api/v1/notes/:id",
            patch(update).get(get_one).delete(delete),
        )
        .route("/api/v1/notes/:id/tags", get(get_tags).put(set_tags))
        .route("/api/v1/notes/:id/versions", get(list_versions))
        .route(
            "/api/v1/notes/:id/versions/:version_no/restore",
            axum::routing::post(restore_version),
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
    /// Filter by tag (exact match). Takes precedence over `notebook` when both
    /// are present.
    tag: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Note>>> {
    let pool = state.db_or_unavailable()?;
    let repo = NoteRepo::new(pool);
    // Tag filter wins when set — it's the narrower, explicit selection.
    if let Some(tag) = q.tag.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        let notes = repo
            .list_by_tag(ctx.tenant_id, ctx.user_id, q.archived, tag)
            .await?;
        return Ok(Json(notes));
    }
    let filter = match q.notebook.as_deref() {
        None => NotebookFilter::Any,
        Some("none") => NotebookFilter::Loose,
        Some(s) => {
            let nb = Uuid::parse_str(s)
                .map_err(|_| NotesError::BadRequest("invalid notebook id".into()))?;
            NotebookFilter::In(nb)
        }
    };
    let notes = repo
        .list(ctx.tenant_id, ctx.user_id, q.archived, filter)
        .await?;
    Ok(Json(notes))
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

/// GET /api/v1/notes/search?q=&limit= — substring search over the caller's own
/// notes (title + body). Mirrors the REST search in contacts/calendar/drive.
/// `q` empty → 400. `limit` clamped 1..200 (default 50).
async fn search(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<Note>>> {
    let query = q.q.trim();
    if query.is_empty() {
        return Err(NotesError::BadRequest("q must not be empty".into()));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let pool = state.db_or_unavailable()?;
    let notes = NoteRepo::new(pool)
        .search(ctx.tenant_id, ctx.user_id, query, limit)
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
    // Snapshot prior content before a content-touching edit, so it's restorable.
    // View-only changes (pin/archive/notebook) don't create a version.
    if touches_content(&body) {
        let current = repo.get(ctx.tenant_id, id).await?;
        NoteVersionRepo::new(pool)
            .snapshot(ctx.tenant_id, id, &snapshot_of(&current), ctx.user_id)
            .await?;
    }
    let note = repo.update(ctx.tenant_id, id, body).await?;
    index_note(&state, &note);
    Ok(Json(note))
}

/// True when an update changes content (title/body/color) — the only edits worth
/// versioning. `color` is doubly-optional, so any presence counts.
fn touches_content(u: &UpdateNote) -> bool {
    u.title.is_some() || u.body.is_some() || u.color.is_some()
}

fn snapshot_of(n: &Note) -> NoteSnapshot {
    NoteSnapshot {
        title: n.title.clone(),
        body: n.body.clone(),
        color: n.color.clone(),
    }
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

#[derive(Debug, Deserialize)]
struct SetTagsBody {
    tags: Vec<String>,
}

/// GET /api/v1/notes/:id/tags — the note's tags. Read-gated like `get_one`.
async fn get_tags(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<String>>> {
    let pool = state.db_or_unavailable()?;
    assert_can_read(&NoteRepo::new(pool), ctx.tenant_id, id, ctx.user_id).await?;
    let tags = NoteTagRepo::new(pool).list(ctx.tenant_id, id).await?;
    Ok(Json(tags))
}

/// PUT /api/v1/notes/:id/tags — replace the note's whole tag set. Write-gated.
async fn set_tags(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<SetTagsBody>,
) -> Result<Json<Vec<String>>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(&NoteRepo::new(pool), ctx.tenant_id, id, ctx.user_id).await?;
    let tags = NoteTagRepo::new(pool)
        .set(ctx.tenant_id, id, &body.tags)
        .await?;
    Ok(Json(tags))
}

#[derive(Debug, Deserialize)]
struct RenameTagBody {
    /// New tag name to rename `:tag` to (across all the caller's notes).
    new: String,
}

#[derive(Debug, Deserialize)]
struct MergeTagBody {
    /// Destination tag `:tag` is merged into (across all the caller's notes).
    into: String,
}

/// GET /api/v1/notes/tags/stats — usage count per tag across the caller's own
/// notes, most-used first. No per-note gate: it only ever counts the caller's
/// own notes via the repo join.
async fn tag_stats(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<TagCount>>> {
    let pool = state.db_or_unavailable()?;
    let stats = NoteTagRepo::new(pool)
        .count_by_tag(ctx.tenant_id, ctx.user_id)
        .await?;
    Ok(Json(stats))
}

/// Query for the co-occurrence endpoint: how many tag pairs to return.
#[derive(Debug, Deserialize)]
struct CoOccurrenceQuery {
    limit: Option<i64>,
}

/// GET /api/v1/notes/tags/co-occurrence?limit= — pairs of tags that appear
/// together on the caller's own notes, most-co-occurring first. Like tag_stats,
/// it only ever counts the caller's own notes via the repo join. `limit` is
/// clamped to 1..=200 (default 50).
async fn tag_co_occurrence(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(q): Query<CoOccurrenceQuery>,
) -> Result<Json<Vec<TagPairCount>>> {
    let pool = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let pairs = NoteTagRepo::new(pool)
        .co_occurrence(ctx.tenant_id, ctx.user_id, limit)
        .await?;
    Ok(Json(pairs))
}

/// PATCH /api/v1/notes/tags/:tag — rename a tag across all the caller's notes.
/// Body `{"new": "..."}`. Returns how many notes had the old tag removed.
async fn rename_tag(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(tag): Path<String>,
    Json(body): Json<RenameTagBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let n = NoteTagRepo::new(pool)
        .rename_across_notes(ctx.tenant_id, ctx.user_id, &tag, &body.new)
        .await?;
    Ok(Json(serde_json::json!({ "renamed_notes": n })))
}

/// POST /api/v1/notes/tags/:tag/merge — merge a tag into another across all the
/// caller's notes. Body `{"into": "..."}`. Same operation as rename, but framed
/// as consolidation (the destination usually already exists).
async fn merge_tag(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(tag): Path<String>,
    Json(body): Json<MergeTagBody>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let n = NoteTagRepo::new(pool)
        .rename_across_notes(ctx.tenant_id, ctx.user_id, &tag, &body.into)
        .await?;
    Ok(Json(serde_json::json!({ "merged_notes": n })))
}

/// GET /api/v1/notes/:id/versions — the note's content history, newest first.
/// Read-gated.
async fn list_versions(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<NoteVersion>>> {
    let pool = state.db_or_unavailable()?;
    assert_can_read(&NoteRepo::new(pool), ctx.tenant_id, id, ctx.user_id).await?;
    let versions = NoteVersionRepo::new(pool).list(ctx.tenant_id, id).await?;
    Ok(Json(versions))
}

/// POST /api/v1/notes/:id/versions/:version_no/restore — restore a prior
/// version's content onto the live note. Write-gated. The restore is itself a
/// content edit, so the current content is snapshotted first (the undo is
/// reversible). Returns the updated note.
async fn restore_version(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, version_no)): Path<(Uuid, i32)>,
) -> Result<Json<Note>> {
    let pool = state.db_or_unavailable()?;
    let repo = NoteRepo::new(pool);
    assert_can_write(&repo, ctx.tenant_id, id, ctx.user_id).await?;
    let versions = NoteVersionRepo::new(pool);
    let target = versions.get(ctx.tenant_id, id, version_no).await?;
    let current = repo.get(ctx.tenant_id, id).await?;
    versions
        .snapshot(ctx.tenant_id, id, &snapshot_of(&current), ctx.user_id)
        .await?;
    let restore = UpdateNote {
        title: Some(target.title),
        body: Some(target.body),
        color: Some(target.color),
        ..Default::default()
    };
    let note = repo.update(ctx.tenant_id, id, restore).await?;
    index_note(&state, &note);
    Ok(Json(note))
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

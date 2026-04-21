//! Global Address List — cross-tenant search over users + contacts.
//!
//! Merges:
//! - `users` rows within the same tenant (directory entries).
//! - `contacts` rows across all addressbooks belonging to the caller.
//!
//! Future: include tenant-wide "shared" addressbooks once the sharing
//! model lands.

use axum::{
    extract::{Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{ContactRepo, vcard};
use crate::error::{ContactsError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/gal/search", get(search))
        .route("/api/v1/gal/save",   post(save_directory))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q:     String,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "source")]
pub enum GalEntry {
    #[serde(rename = "directory")]
    Directory {
        user_id:      Uuid,
        email:        String,
        display_name: String,
        given_name:   Option<String>,
        family_name:  Option<String>,
    },
    #[serde(rename = "contact")]
    Contact {
        contact_id:     Uuid,
        addressbook_id: Uuid,
        email:          Option<String>,
        full_name:      Option<String>,
        organization:   Option<String>,
    },
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub entries: Vec<GalEntry>,
}

async fn search(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Query(q): Query<SearchQuery>,
) -> Result<Json<SearchResponse>> {
    let pool  = state.db_or_unavailable()?;
    let limit = q.limit.unwrap_or(20).clamp(1, 100);
    // ILIKE pattern; caller-supplied string escaped by sqlx binding.
    let pat = format!("%{}%", q.q);

    // Directory (users within tenant).
    let dir_rows = sqlx::query(
        "SELECT id, email, display_name, given_name, family_name
         FROM users
         WHERE tenant_id = $1
           AND is_active = true
           AND (email ILIKE $2 OR display_name ILIKE $2)
         ORDER BY display_name
         LIMIT $3",
    )
    .bind(ctx.tenant_id)
    .bind(&pat)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    // Contacts (caller's addressbooks).
    let con_rows = sqlx::query(
        "SELECT c.id, c.addressbook_id, c.email_primary, c.full_name, c.organization
         FROM contacts c
         JOIN addressbooks a ON a.id = c.addressbook_id
         WHERE c.tenant_id = $1
           AND a.owner_user_id = $2
           AND (c.full_name ILIKE $3 OR c.email_primary ILIKE $3 OR c.organization ILIKE $3)
         ORDER BY c.full_name
         LIMIT $4",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&pat)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut entries = Vec::with_capacity(dir_rows.len() + con_rows.len());
    for r in dir_rows {
        entries.push(GalEntry::Directory {
            user_id:      r.get("id"),
            email:        r.get("email"),
            display_name: r.get("display_name"),
            given_name:   r.try_get("given_name").ok(),
            family_name:  r.try_get("family_name").ok(),
        });
    }
    for r in con_rows {
        entries.push(GalEntry::Contact {
            contact_id:     r.get("id"),
            addressbook_id: r.get("addressbook_id"),
            email:          r.try_get("email_primary").ok(),
            full_name:      r.try_get("full_name").ok(),
            organization:   r.try_get("organization").ok(),
        });
    }

    Ok(Json(SearchResponse { entries }))
}


/// Request body for POST /api/v1/gal/save — identify a directory user to
/// materialize as a personal contact. Either `user_id` (uuid) or `email`
/// (tenant-scoped) must be provided.
#[derive(Debug, Deserialize)]
pub struct SaveRequest {
    #[serde(default)]
    pub user_id: Option<Uuid>,
    #[serde(default)]
    pub email:   Option<String>,
    /// Optional target addressbook. Default: caller's `is_default=true`
    /// addressbook, auto-created as "Pessoal" if none exists.
    #[serde(default)]
    pub addressbook_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct SaveResponse {
    pub contact_id:     Uuid,
    pub addressbook_id: Uuid,
    pub uid:            String,
    pub created:        bool,
}

/// POST /api/v1/gal/save — copy a directory user into the caller's personal
/// addressbook as a vCard. Idempotent: uses a stable UID `dir:<user_id>` so
/// re-saving updates the existing contact in place.
async fn save_directory(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(req): Json<SaveRequest>,
) -> Result<Json<SaveResponse>> {
    if req.user_id.is_none() && req.email.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
        return Err(ContactsError::BadRequest("`user_id` or `email` required".into()));
    }
    let pool = state.db_or_unavailable()?;

    // Lookup the directory user within tenant.
    let user_row = if let Some(uid) = req.user_id {
        sqlx::query(
            "SELECT id, email, display_name, given_name, family_name
               FROM users
              WHERE tenant_id = $1 AND id = $2 AND is_active = true",
        )
        .bind(ctx.tenant_id)
        .bind(uid)
        .fetch_optional(pool)
        .await?
    } else {
        let email = req.email.as_deref().unwrap().trim().to_ascii_lowercase();
        sqlx::query(
            "SELECT id, email, display_name, given_name, family_name
               FROM users
              WHERE tenant_id = $1 AND lower(email) = $2 AND is_active = true",
        )
        .bind(ctx.tenant_id)
        .bind(email)
        .fetch_optional(pool)
        .await?
    };

    let row = user_row.ok_or_else(|| ContactsError::BadRequest("directory user not found".into()))?;
    let dir_id:      Uuid            = row.get("id");
    let email:       String          = row.get("email");
    let display:     String          = row.get("display_name");
    let given:       Option<String>  = row.try_get("given_name").ok();
    let family:      Option<String>  = row.try_get("family_name").ok();

    // Resolve target addressbook (explicit → default → create "Pessoal").
    let book_id = resolve_addressbook(pool, ctx.tenant_id, ctx.user_id, req.addressbook_id).await?;

    // Build vCard + upsert via existing replace_by_uid (CardDAV path).
    let uid = format!("dir:{dir_id}");
    let raw = vcard::build_vcard(
        &uid,
        &display,
        family.as_deref(),
        given.as_deref(),
        Some(email.as_str()),
        None,
    );

    let existed = sqlx::query("SELECT 1 FROM contacts WHERE tenant_id = $1 AND addressbook_id = $2 AND uid = $3")
        .bind(ctx.tenant_id)
        .bind(book_id)
        .bind(&uid)
        .fetch_optional(pool)
        .await?
        .is_some();

    let c = ContactRepo::new(pool)
        .replace_by_uid(ctx.tenant_id, book_id, &raw)
        .await?;

    Ok(Json(SaveResponse {
        contact_id:     c.id,
        addressbook_id: c.addressbook_id,
        uid:            c.uid,
        created:        !existed,
    }))
}

/// Pick target addressbook: explicit → default → auto-create "Pessoal".
async fn resolve_addressbook(
    pool:      &expresso_core::DbPool,
    tenant_id: Uuid,
    owner:     Uuid,
    explicit:  Option<Uuid>,
) -> Result<Uuid> {
    if let Some(id) = explicit {
        let (found,): (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM addressbooks WHERE tenant_id = $1 AND id = $2 AND owner_user_id = $3)",
        )
        .bind(tenant_id)
        .bind(id)
        .bind(owner)
        .fetch_one(pool)
        .await?;
        if !found {
            return Err(ContactsError::AddressbookNotFound(id.to_string()));
        }
        return Ok(id);
    }
    // Default
    let def: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM addressbooks
           WHERE tenant_id = $1 AND owner_user_id = $2 AND is_default = true
           LIMIT 1",
    )
    .bind(tenant_id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    if let Some((id,)) = def { return Ok(id); }

    // Auto-create "Pessoal" as default for this owner.
    let (id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO addressbooks (tenant_id, owner_user_id, name, is_default)
         VALUES ($1, $2, 'Pessoal', true)
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(owner)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

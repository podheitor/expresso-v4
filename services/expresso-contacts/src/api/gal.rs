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
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::Result;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/v1/gal/search", get(search))
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

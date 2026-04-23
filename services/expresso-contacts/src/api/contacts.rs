//! Contact REST endpoints — path-addressed by (addressbook_id, id).
//! Create/Update accept raw vCard (`text/vcard`) payload.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Contact, ContactRepo};
use crate::error::Result;
use crate::state::AppState;

/// Gate: require OWNER/WRITE/ADMIN on the addressbook.
async fn assert_can_write(
    pool: &expresso_core::DbPool,
    tenant_id: uuid::Uuid,
    book_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<()> {
    let repo = crate::domain::AddressbookRepo::new(pool);
    let lvl = repo.access_level(tenant_id, book_id, user_id).await?;
    match lvl.as_deref() {
        Some("OWNER") | Some("WRITE") | Some("ADMIN") => Ok(()),
        Some(_) => Err(crate::error::ContactsError::Forbidden),
        None    => Err(crate::error::ContactsError::AddressbookNotFound(book_id.to_string())),
    }
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/addressbooks/:book_id/contacts",
            post(create).get(list),
        )
        .route(
            "/api/v1/addressbooks/:book_id/contacts/:id",
            get(get_one).put(update).delete(delete),
        )
        .route(
            "/api/v1/addressbooks/:book_id/export.vcf",
            get(export_vcf),
        )
        .route(
            "/api/v1/addressbooks/:book_id/import",
            post(import_vcf),
        )
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, book_id, ctx.user_id).await?;
    let c    = ContactRepo::new(pool).create(ctx.tenant_id, book_id, &raw).await?;
    let loc  = format!("/api/v1/addressbooks/{}/contacts/{}", book_id, c.id);
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header(header::LOCATION, loc)
        .header(header::ETAG, format!("\"{}\"", c.etag))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&c).unwrap()))
        .unwrap())
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
) -> Result<Json<Vec<Contact>>> {
    let pool = state.db_or_unavailable()?;
    let cs   = ContactRepo::new(pool).list(ctx.tenant_id, book_id).await?;
    Ok(Json(cs))
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_book_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let c    = ContactRepo::new(pool).get(ctx.tenant_id, id).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::ETAG, format!("\"{}\"", c.etag))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&c).unwrap()))
        .unwrap())
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((book_id, id)): Path<(Uuid, Uuid)>,
    _headers: HeaderMap,
    raw: String,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, book_id, ctx.user_id).await?;
    let c    = ContactRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::ETAG, format!("\"{}\"", c.etag))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&c).unwrap()))
        .unwrap())
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((book_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, book_id, ctx.user_id).await?;
    ContactRepo::new(pool).delete(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}


/// GET /api/v1/addressbooks/:book_id/export.vcf — concat of all contacts'
/// raw vCards as a single text/vcard download.
async fn export_vcf(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
) -> Result<Response> {
    use crate::domain::vcard;
    let pool  = state.db_or_unavailable()?;
    let cs    = ContactRepo::new(pool).list(ctx.tenant_id, book_id).await?;
    let cards: Vec<String> = cs.into_iter().map(|c| c.vcard_raw).collect();
    let body = vcard::concat_vcards(&cards);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/vcard; charset=utf-8")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"addressbook.vcf\"")
        .body(Body::from(body))
        .unwrap())
}

/// POST /api/v1/addressbooks/:book_id/import — body is a file with 1..N
/// BEGIN:VCARD..END:VCARD blocks. Each is upserted by UID. Returns summary.
async fn import_vcf(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    use crate::domain::vcard;
    if raw.trim().is_empty() {
        return Err(crate::error::ContactsError::InvalidVCard("empty body".into()));
    }
    let blocks = vcard::split_vcards(&raw);
    if blocks.is_empty() {
        return Err(crate::error::ContactsError::InvalidVCard("no VCARD blocks found".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = ContactRepo::new(pool);
    let mut imported = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for (idx, block) in blocks.iter().enumerate() {
        match repo.replace_by_uid(ctx.tenant_id, book_id, block).await {
            Ok(_)  => imported += 1,
            Err(e) => errors.push(format!("vcard[{idx}]: {e}")),
        }
    }
    let body = serde_json::json!({
        "imported": imported,
        "failed":   errors.len(),
        "errors":   errors,
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap())
}

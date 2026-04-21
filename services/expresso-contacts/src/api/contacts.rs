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
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
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
    Path((_book_id, id)): Path<(Uuid, Uuid)>,
    _headers: HeaderMap,
    raw: String,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
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
    Path((_book_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    ContactRepo::new(pool).delete(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

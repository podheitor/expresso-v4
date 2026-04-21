//! Addressbook collection REST endpoints (JSON).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Addressbook, AddressbookRepo, NewAddressbook, UpdateAddressbook};
use crate::error::Result;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/addressbooks",       post(create).get(list))
        .route("/api/v1/addressbooks/:id",   get(get_one).delete(delete).patch(update))
        .route("/api/v1/addressbooks/:id/ctag", get(ctag_one))
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewAddressbook>,
) -> Result<(StatusCode, Json<Addressbook>)> {
    let pool = state.db_or_unavailable()?;
    let ab = AddressbookRepo::new(pool).create(ctx.tenant_id, ctx.user_id, body).await?;
    Ok((StatusCode::CREATED, Json(ab)))
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<Addressbook>>> {
    let pool = state.db_or_unavailable()?;
    let abs  = AddressbookRepo::new(pool).list_for_owner(ctx.tenant_id, ctx.user_id).await?;
    Ok(Json(abs))
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Addressbook>> {
    let pool = state.db_or_unavailable()?;
    let ab = AddressbookRepo::new(pool).get(ctx.tenant_id, id).await?;
    Ok(Json(ab))
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAddressbook>,
) -> Result<Json<Addressbook>> {
    let pool = state.db_or_unavailable()?;
    let ab = AddressbookRepo::new(pool).update(ctx.tenant_id, id, body).await?;
    Ok(Json(ab))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    AddressbookRepo::new(pool).delete(ctx.tenant_id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ctag_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>> {
    let pool = state.db_or_unavailable()?;
    let ctag = AddressbookRepo::new(pool).ctag(ctx.tenant_id, id).await?;
    Ok(Json(serde_json::json!({ "id": id, "ctag": ctag })))
}

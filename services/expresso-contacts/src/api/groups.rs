//! Contact group (distribution list) REST endpoints (JSON).
//!
//! GET    /api/v1/contact-groups                       — list caller's groups
//! POST   /api/v1/contact-groups                       — create a group
//! GET    /api/v1/contact-groups/:id                   — fetch one group
//! PATCH  /api/v1/contact-groups/:id                   — rename / re-describe
//! DELETE /api/v1/contact-groups/:id                   — delete a group
//! GET    /api/v1/contact-groups/:id/members           — list member contacts
//! POST   /api/v1/contact-groups/:id/members           — add a member
//! DELETE /api/v1/contact-groups/:id/members/:cid      — remove a member

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::contact::Contact;
use crate::domain::{ContactGroup, ContactGroupRepo, NewContactGroup, UpdateContactGroup};
use crate::error::Result;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/contact-groups", post(create).get(list))
        .route(
            "/api/v1/contact-groups/:id",
            get(get_one).patch(update).delete(delete),
        )
        .route(
            "/api/v1/contact-groups/:id/members",
            get(list_members).post(add_member),
        )
        .route(
            "/api/v1/contact-groups/:id/members/:contact_id",
            axum::routing::delete(remove_member),
        )
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewContactGroup>,
) -> Result<(StatusCode, Json<ContactGroup>)> {
    let pool = state.db_or_unavailable()?;
    let group = ContactGroupRepo::new(pool)
        .create(ctx.tenant_id, ctx.user_id, body)
        .await?;
    Ok((StatusCode::CREATED, Json(group)))
}

async fn list(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<ContactGroup>>> {
    let pool = state.db_or_unavailable()?;
    let groups = ContactGroupRepo::new(pool)
        .list_for_owner(ctx.tenant_id, ctx.user_id)
        .await?;
    Ok(Json(groups))
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<ContactGroup>> {
    let pool = state.db_or_unavailable()?;
    let group = ContactGroupRepo::new(pool)
        .get(ctx.tenant_id, ctx.user_id, id)
        .await?;
    Ok(Json(group))
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateContactGroup>,
) -> Result<Json<ContactGroup>> {
    let pool = state.db_or_unavailable()?;
    let group = ContactGroupRepo::new(pool)
        .update(ctx.tenant_id, ctx.user_id, id, body)
        .await?;
    Ok(Json(group))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    ContactGroupRepo::new(pool)
        .delete(ctx.tenant_id, ctx.user_id, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_members(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Contact>>> {
    let pool = state.db_or_unavailable()?;
    let members = ContactGroupRepo::new(pool)
        .list_members(ctx.tenant_id, ctx.user_id, id)
        .await?;
    Ok(Json(members))
}

#[derive(Debug, Deserialize)]
struct AddMemberBody {
    contact_id: Uuid,
}

async fn add_member(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<AddMemberBody>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    ContactGroupRepo::new(pool)
        .add_member(ctx.tenant_id, ctx.user_id, id, body.contact_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((id, contact_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    ContactGroupRepo::new(pool)
        .remove_member(ctx.tenant_id, ctx.user_id, id, contact_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

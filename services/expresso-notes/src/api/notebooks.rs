//! Notebooks (folders) REST API.
//!
//! Routes (all scoped to the authenticated user — notebooks are private):
//!   GET    /api/v1/notebooks        → list own notebooks
//!   POST   /api/v1/notebooks        → create
//!   GET    /api/v1/notebooks/:id    → fetch one
//!   PATCH  /api/v1/notebooks/:id    → rename / recolor
//!   DELETE /api/v1/notebooks/:id    → delete (notes inside detach, not deleted)
//!
//! Notes reference a notebook via `notebook_id`; filtering a note list by
//! notebook is on the notes routes (`GET /api/v1/notes?notebook=:id`).

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{NewNotebook, Notebook, NotebookRepo, UpdateNotebook};
use crate::error::Result;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/notebooks", get(list).post(create))
        .route(
            "/api/v1/notebooks/:id",
            patch(update).get(get_one).delete(delete),
        )
}

async fn list(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<Notebook>>> {
    let pool = state.db_or_unavailable()?;
    let notebooks = NotebookRepo::new(pool)
        .list(ctx.tenant_id, ctx.user_id)
        .await?;
    Ok(Json(notebooks))
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewNotebook>,
) -> Result<(StatusCode, Json<Notebook>)> {
    let pool = state.db_or_unavailable()?;
    let notebook = NotebookRepo::new(pool)
        .create(ctx.tenant_id, ctx.user_id, body)
        .await?;
    Ok((StatusCode::CREATED, Json(notebook)))
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Notebook>> {
    let pool = state.db_or_unavailable()?;
    let notebook = NotebookRepo::new(pool)
        .get(ctx.tenant_id, ctx.user_id, id)
        .await?;
    Ok(Json(notebook))
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateNotebook>,
) -> Result<Json<Notebook>> {
    let pool = state.db_or_unavailable()?;
    let notebook = NotebookRepo::new(pool)
        .update(ctx.tenant_id, ctx.user_id, id, body)
        .await?;
    Ok(Json(notebook))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    NotebookRepo::new(pool)
        .delete(ctx.tenant_id, ctx.user_id, id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

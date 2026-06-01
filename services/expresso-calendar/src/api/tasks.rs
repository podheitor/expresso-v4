//! REST API for calendar tasks (RFC 5545 VTODO).
//!
//! Routes (all under a calendar, sharing its ACL — write ops need WRITE+, reads
//! need any grant):
//!   GET    /api/v1/calendars/:cal_id/tasks          → list
//!   POST   /api/v1/calendars/:cal_id/tasks          → create
//!   GET    /api/v1/calendars/:cal_id/tasks/:id      → fetch one
//!   PATCH  /api/v1/calendars/:cal_id/tasks/:id      → partial update
//!   DELETE /api/v1/calendars/:cal_id/tasks/:id      → delete
//!   GET    /api/v1/calendars/:cal_id/tasks/:id/instances?from=&to= → expand recurrence
//!
//! Tasks are full-text indexed (`kind = "task"`) so they surface in unified
//! search alongside events/mail/drive/contacts.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{NewTask, Task, TaskRepo, UpdateTask};
use crate::error::{CalendarError, Result};
use crate::state::AppState;

use super::events::assert_can_write;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/calendars/:cal_id/tasks",
            get(list_tasks).post(create_task),
        )
        .route(
            "/api/v1/calendars/:cal_id/tasks/:id",
            patch(update_task).get(get_task).delete(delete_task),
        )
        .route(
            "/api/v1/calendars/:cal_id/tasks/:id/instances",
            get(task_instances),
        )
}

#[derive(Debug, serde::Deserialize)]
struct InstancesParams {
    #[serde(with = "time::serde::rfc3339")]
    from: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    to: OffsetDateTime,
}

/// GET /api/v1/calendars/:cal_id/tasks/:id/instances?from=&to= — expand a
/// recurring task's occurrences within `[from, to)` via the shared rrule
/// expander (mirrors events/:id/instances). A one-off task yields its single
/// anchor when in range; a task without dtstart/due yields none. `from < to`
/// required. Returns `{task_id, summary, rrule, count, instances: [rfc3339…]}`.
async fn task_instances(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Query(params): Query<InstancesParams>,
) -> Result<Json<serde_json::Value>> {
    if params.from >= params.to {
        return Err(CalendarError::BadRequest("from must be < to".into()));
    }
    let pool = state.db_or_unavailable()?;
    assert_can_read(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    let task = TaskRepo::new(pool).get(ctx.tenant_id, cal_id, id).await?;
    let instances = task.expand_instances(params.from, params.to);
    let formatted: Vec<String> = instances
        .iter()
        .filter_map(|dt| {
            dt.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .collect();
    Ok(Json(serde_json::json!({
        "task_id":   task.id,
        "summary":   task.summary,
        "rrule":     task.rrule,
        "count":     formatted.len(),
        "instances": formatted,
    })))
}

async fn list_tasks(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
) -> Result<Json<Vec<Task>>> {
    let pool = state.db_or_unavailable()?;
    assert_can_read(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    let tasks = TaskRepo::new(pool).list(ctx.tenant_id, cal_id).await?;
    Ok(Json(tasks))
}

async fn create_task(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(cal_id): Path<Uuid>,
    Json(body): Json<NewTask>,
) -> Result<(StatusCode, Json<Task>)> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    let task = TaskRepo::new(pool)
        .create(ctx.tenant_id, cal_id, body)
        .await?;
    index_task(&state, &task);
    Ok((StatusCode::CREATED, Json(task)))
}

async fn get_task(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Task>> {
    let pool = state.db_or_unavailable()?;
    assert_can_read(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    let task = TaskRepo::new(pool).get(ctx.tenant_id, cal_id, id).await?;
    Ok(Json(task))
}

async fn update_task(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateTask>,
) -> Result<Json<Task>> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    let task = TaskRepo::new(pool)
        .update(ctx.tenant_id, cal_id, id, body)
        .await?;
    index_task(&state, &task);
    Ok(Json(task))
}

async fn delete_task(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((cal_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, cal_id, ctx.user_id).await?;
    TaskRepo::new(pool)
        .delete(ctx.tenant_id, cal_id, id)
        .await?;
    deindex_task(&state, id);
    Ok(StatusCode::NO_CONTENT)
}

/// Read gate: any access level (READ/WRITE/ADMIN/OWNER) passes; absence is a
/// 404 (the calendar isn't visible to this user). RLS already scopes the tenant.
async fn assert_can_read(
    pool: &expresso_core::DbPool,
    tenant_id: Uuid,
    cal_id: Uuid,
    user_id: Uuid,
) -> Result<()> {
    let lvl = crate::domain::CalendarRepo::new(pool)
        .access_level(tenant_id, cal_id, user_id)
        .await?;
    match lvl {
        Some(_) => Ok(()),
        None => Err(crate::error::CalendarError::CalendarNotFound(
            cal_id.to_string(),
        )),
    }
}

/// Fire-and-forget index of a task (`kind = "task"`). No-op without search.
/// Summary is the subject, description the body. Mirrors the events indexer.
fn index_task(state: &AppState, t: &Task) {
    let search_url = state.search_url();
    if search_url.is_empty() {
        return;
    }
    // document_id is the bare task UUID (not namespaced) so the single-id delete
    // endpoint can address it; `kind = "task"` carries the facet instead.
    let doc = serde_json::json!({
        "document_id": t.id.to_string(),
        "tenant_id":   t.tenant_id.to_string(),
        "subject":     t.summary,
        "body":        t.description,
        "kind":        "task",
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

/// Remove a task from the index on delete (document_id is the task UUID).
fn deindex_task(state: &AppState, id: Uuid) {
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

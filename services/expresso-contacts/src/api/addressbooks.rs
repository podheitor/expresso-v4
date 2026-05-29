//! Addressbook collection REST endpoints (JSON).

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Addressbook, AddressbookRepo, NewAddressbook, UpdateAddressbook};
use crate::error::Result;
use crate::events::ContactsEvent;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/v1/addressbooks", post(create).get(list))
        .route(
            "/api/v1/addressbooks/:id",
            get(get_one).delete(delete).patch(update),
        )
        .route("/api/v1/addressbooks/:id/ctag", get(ctag_one))
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<NewAddressbook>,
) -> Result<(StatusCode, Json<Addressbook>)> {
    let pool = state.db_or_unavailable()?;
    let ab = AddressbookRepo::new(pool)
        .create(ctx.tenant_id, ctx.user_id, body)
        .await?;
    state.bus().publish(ContactsEvent::AddressbookCreated {
        tenant_id: ctx.tenant_id,
        addressbook_id: ab.id,
        name: Some(ab.name.clone()),
    });
    Ok((StatusCode::CREATED, Json(ab)))
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    req_headers: HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let (total, max_updated): (i64, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(updated_at) FROM addressbooks WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) =
                    OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822)
                {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let abs = AddressbookRepo::new(pool)
        .list_accessible(ctx.tenant_id, ctx.user_id)
        .await?;
    let mut resp = (
        [(
            header::HeaderName::from_static("x-total-count"),
            total.to_string(),
        )],
        Json(abs),
    )
        .into_response();
    if let Some(ts) = max_updated {
        let lm = ts
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap_or_default();
        resp.headers_mut()
            .insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    req_headers: axum::http::HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let ab = AddressbookRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}-{}\"", ab.updated_at.unix_timestamp(), ab.id);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    let lm = ab
        .updated_at
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) =
                OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822)
            {
                if ab.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(ab).into_response();
    resp.headers_mut()
        .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut()
        .insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAddressbook>,
) -> Result<Json<Addressbook>> {
    let pool = state.db_or_unavailable()?;
    let ab = AddressbookRepo::new(pool)
        .update(ctx.tenant_id, id, body)
        .await?;
    Ok(Json(ab))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    AddressbookRepo::new(pool).delete(ctx.tenant_id, id).await?;
    state.bus().publish(ContactsEvent::AddressbookDeleted {
        tenant_id: ctx.tenant_id,
        addressbook_id: id,
    });
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

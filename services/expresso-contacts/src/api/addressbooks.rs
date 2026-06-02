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
        // Static `shared` before `:id` so matchit doesn't capture it as an id.
        .route("/api/v1/addressbooks/shared", get(list_shared))
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

/// GET /api/v1/addressbooks/shared — addressbooks shared *with* the caller via
/// `addressbook_acl` (excluding owned). The dedicated "shared with me" view;
/// mirrors notes/drive/calendar shared endpoints.
async fn list_shared(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<Addressbook>>> {
    let pool = state.db_or_unavailable()?;
    let abs = AddressbookRepo::new(pool)
        .list_shared(ctx.tenant_id, ctx.user_id)
        .await?;
    Ok(Json(abs))
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

/// Authorize an addressbook mutation. `delete` allows OWNER only (you cannot
/// delete a book merely shared to you); `update` (rename/set-default) allows
/// OWNER or ADMIN. A non-grantee gets a 404 so existence is not revealed; a
/// grantee without enough privilege gets 403. Mirrors contacts.rs::assert_can_write.
async fn assert_owns(
    repo: &AddressbookRepo<'_>,
    tenant_id: Uuid,
    id: Uuid,
    user_id: Uuid,
    allow_admin: bool,
) -> Result<()> {
    let level = repo.access_level(tenant_id, id, user_id).await?;
    owner_decision(level.as_deref(), allow_admin, id)
}

/// Pure privilege decision for an addressbook mutation. `None` (no grant) → 404
/// (hide existence); insufficient privilege → 403; OWNER always passes, ADMIN
/// passes only when `allow_admin` (update, not delete).
fn owner_decision(level: Option<&str>, allow_admin: bool, id: Uuid) -> Result<()> {
    match level {
        Some("OWNER") => Ok(()),
        Some("ADMIN") if allow_admin => Ok(()),
        Some(_) => Err(crate::error::ContactsError::Forbidden),
        None => Err(crate::error::ContactsError::AddressbookNotFound(
            id.to_string(),
        )),
    }
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAddressbook>,
) -> Result<Json<Addressbook>> {
    let pool = state.db_or_unavailable()?;
    let repo = AddressbookRepo::new(pool);
    assert_owns(&repo, ctx.tenant_id, id, ctx.user_id, true).await?;
    let ab = repo.update(ctx.tenant_id, id, body).await?;
    Ok(Json(ab))
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    let repo = AddressbookRepo::new(pool);
    assert_owns(&repo, ctx.tenant_id, id, ctx.user_id, false).await?;
    repo.delete(ctx.tenant_id, id).await?;
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

#[cfg(test)]
mod tests {
    use super::owner_decision;
    use crate::error::ContactsError;
    use uuid::Uuid;

    fn id() -> Uuid {
        Uuid::nil()
    }

    #[test]
    fn owner_passes_update_and_delete() {
        assert!(owner_decision(Some("OWNER"), true, id()).is_ok());
        assert!(owner_decision(Some("OWNER"), false, id()).is_ok());
    }

    #[test]
    fn admin_passes_update_only() {
        assert!(owner_decision(Some("ADMIN"), true, id()).is_ok());
        // delete (allow_admin=false): ADMIN is not enough.
        assert!(matches!(
            owner_decision(Some("ADMIN"), false, id()),
            Err(ContactsError::Forbidden)
        ));
    }

    #[test]
    fn write_and_read_grants_are_forbidden_for_owner_actions() {
        for lvl in ["WRITE", "READ"] {
            assert!(matches!(
                owner_decision(Some(lvl), true, id()),
                Err(ContactsError::Forbidden)
            ));
        }
    }

    #[test]
    fn no_grant_is_not_found() {
        assert!(matches!(
            owner_decision(None, true, id()),
            Err(ContactsError::AddressbookNotFound(_))
        ));
    }
}

//! Admin SSR for calendars + addressbooks across all tenants.
//! RLS bypass: the connection from this service does NOT set `app.tenant_id`,
//! and the policy explicitly allows `app.tenant_id IS NULL` → all rows visible.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::{
    templates::{
        AddressbookAdminEditTpl, AddressbooksAdminTpl,
        CalendarAdminEditTpl, CalendarsAdminTpl, DavRow,
    },
    AdminError, AppState,
};

/// `None` when the caller is super-admin (sees every tenant); otherwise the
/// caller's own tenant_id (list views show only that tenant). Used as a
/// nullable bind so a single query covers both cases.
async fn caller_tenant_scope(
    st:      &AppState,
    headers: &axum::http::HeaderMap,
) -> Option<uuid::Uuid> {
    let p = crate::auth::principal_for(st, headers).await;
    if crate::auth::is_super_admin(&p.roles) { None } else { p.tenant_id }
}

fn to_dav_row(
    id: uuid::Uuid,
    tenant_id: uuid::Uuid,
    tenant_name: String,
    owner_email: String,
    name: String,
    description: Option<String>,
    color: Option<String>,
    is_default: bool,
    ctag: i64,
) -> DavRow {
    DavRow {
        id:           id.to_string(),
        tenant_id:    tenant_id.to_string(),
        tenant_name,
        owner_email,
        name,
        description: description.unwrap_or_default(),
        color:       color.unwrap_or_default(),
        is_default,
        ctag,
    }
}

// ── Calendars ──

pub async fn calendars_list(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, AdminError> {
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let scope = caller_tenant_scope(&st, &headers).await;
    let rows = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, String, String, String, Option<String>, Option<String>, bool, i64)>(
        r#"SELECT c.id, c.tenant_id, t.name AS tenant_name, u.email AS owner_email,
                  c.name, c.description, c.color, c.is_default, c.ctag
             FROM calendars c
             JOIN tenants t ON t.id = c.tenant_id
             JOIN users   u ON u.id = c.owner_user_id
            WHERE $1::UUID IS NULL OR c.tenant_id = $1
            ORDER BY t.name, u.email, c.is_default DESC, c.name"#,
    ).bind(scope).fetch_all(pool).await.map_err(|e| AdminError(e.into()))?;

    let rows = rows.into_iter().map(|(id, tid, tname, oe, n, d, col, dflt, ct)|
        to_dav_row(id, tid, tname, oe, n, d, col, dflt, ct)
    ).collect();
    Ok(CalendarsAdminTpl { current: "calendars", rows })
}

pub async fn calendar_edit_form(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((tenant_id, id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny);
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let row = sqlx::query_as::<_, (String, String, String, Option<String>, Option<String>, bool)>(
        r#"SELECT t.name, u.email, c.name, c.description, c.color, c.is_default
             FROM calendars c
             JOIN tenants t ON t.id = c.tenant_id
             JOIN users   u ON u.id = c.owner_user_id
            WHERE c.tenant_id = $1 AND c.id = $2"#,
    ).bind(tenant_id).bind(id).fetch_optional(pool).await.map_err(|e| AdminError(e.into()))?;
    let Some((tname, oe, name, desc, color, dflt)) = row else {
        return Ok(Redirect::to("/calendars").into_response());
    };
    Ok(CalendarAdminEditTpl {
        current: "calendars",
        tenant_id: tenant_id.to_string(),
        id: id.to_string(),
        tenant_name: tname,
        owner_email: oe,
        name,
        description: desc.unwrap_or_default(),
        color: color.unwrap_or_default(),
        is_default: dflt,
        error: None,
    }.into_response())
}

#[derive(Deserialize)]
pub struct CalendarEditForm {
    pub name:        String,
    pub description: String,
    pub color:       String,
    #[serde(default)]
    pub is_default:  Option<String>,
}

pub async fn calendar_edit_action(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((tenant_id, id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(f): Form<CalendarEditForm>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny);
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let dflt = f.is_default.is_some();
    let desc = if f.description.trim().is_empty() { None } else { Some(f.description.trim().to_string()) };
    let color = if f.color.trim().is_empty() { None } else { Some(f.color.trim().to_string()) };
    sqlx::query(
        r#"UPDATE calendars
              SET name = $3, description = $4, color = $5, is_default = $6
            WHERE tenant_id = $1 AND id = $2"#,
    ).bind(tenant_id).bind(id).bind(f.name.trim()).bind(desc).bind(&color).bind(dflt)
     .execute(pool).await.map_err(|e| AdminError(e.into()))?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST,
        &format!("/calendars/{tenant_id}/{id}/edit"),
        "admin.calendar.update", Some("calendar"), Some(id.to_string()), Some(302),
        serde_json::json!({ "tenant_id": tenant_id, "name": f.name, "is_default": dflt, "color": color }),
    ).await;
    Ok(Redirect::to("/calendars").into_response())
}

pub async fn calendar_delete_action(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((tenant_id, id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny.into_response());
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    sqlx::query("DELETE FROM calendars WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id).bind(id)
        .execute(pool).await.map_err(|e| AdminError(e.into()))?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST,
        &format!("/calendars/{tenant_id}/{id}/delete"),
        "admin.calendar.delete", Some("calendar"), Some(id.to_string()), Some(302),
        serde_json::json!({ "tenant_id": tenant_id }),
    ).await;
    Ok(Redirect::to("/calendars").into_response())
}

// ── Addressbooks ──

pub async fn addressbooks_list(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
) -> Result<impl IntoResponse, AdminError> {
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let scope = caller_tenant_scope(&st, &headers).await;
    let rows = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, String, String, String, Option<String>, bool, i64)>(
        r#"SELECT a.id, a.tenant_id, t.name AS tenant_name, u.email AS owner_email,
                  a.name, a.description, a.is_default, a.ctag
             FROM addressbooks a
             JOIN tenants t ON t.id = a.tenant_id
             JOIN users   u ON u.id = a.owner_user_id
            WHERE $1::UUID IS NULL OR a.tenant_id = $1
            ORDER BY t.name, u.email, a.is_default DESC, a.name"#,
    ).bind(scope).fetch_all(pool).await.map_err(|e| AdminError(e.into()))?;
    let rows = rows.into_iter().map(|(id, tid, tname, oe, n, d, dflt, ct)|
        to_dav_row(id, tid, tname, oe, n, d, None, dflt, ct)
    ).collect();
    Ok(AddressbooksAdminTpl { current: "addressbooks", rows })
}

pub async fn addressbook_edit_form(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((tenant_id, id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny);
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let row = sqlx::query_as::<_, (String, String, String, Option<String>)>(
        r#"SELECT t.name, u.email, a.name, a.description
             FROM addressbooks a
             JOIN tenants t ON t.id = a.tenant_id
             JOIN users   u ON u.id = a.owner_user_id
            WHERE a.tenant_id = $1 AND a.id = $2"#,
    ).bind(tenant_id).bind(id).fetch_optional(pool).await.map_err(|e| AdminError(e.into()))?;
    let Some((tname, oe, name, desc)) = row else {
        return Ok(Redirect::to("/addressbooks").into_response());
    };
    Ok(AddressbookAdminEditTpl {
        current: "addressbooks",
        tenant_id: tenant_id.to_string(),
        id: id.to_string(),
        tenant_name: tname,
        owner_email: oe,
        name,
        description: desc.unwrap_or_default(),
        error: None,
    }.into_response())
}

#[derive(Deserialize)]
pub struct AddressbookEditForm {
    pub name:        String,
    pub description: String,
}

pub async fn addressbook_edit_action(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((tenant_id, id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(f): Form<AddressbookEditForm>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny);
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    let desc = if f.description.trim().is_empty() { None } else { Some(f.description.trim().to_string()) };
    sqlx::query(
        r#"UPDATE addressbooks
              SET name = $3, description = $4
            WHERE tenant_id = $1 AND id = $2"#,
    ).bind(tenant_id).bind(id).bind(f.name.trim()).bind(desc)
     .execute(pool).await.map_err(|e| AdminError(e.into()))?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST,
        &format!("/addressbooks/{tenant_id}/{id}/edit"),
        "admin.addressbook.update", Some("addressbook"), Some(id.to_string()), Some(302),
        serde_json::json!({ "tenant_id": tenant_id, "name": f.name }),
    ).await;
    Ok(Redirect::to("/addressbooks").into_response())
}

pub async fn addressbook_delete_action(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((tenant_id, id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny.into_response());
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;
    sqlx::query("DELETE FROM addressbooks WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id).bind(id)
        .execute(pool).await.map_err(|e| AdminError(e.into()))?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST,
        &format!("/addressbooks/{tenant_id}/{id}/delete"),
        "admin.addressbook.delete", Some("addressbook"), Some(id.to_string()), Some(302),
        serde_json::json!({ "tenant_id": tenant_id }),
    ).await;
    Ok(Redirect::to("/addressbooks").into_response())
}

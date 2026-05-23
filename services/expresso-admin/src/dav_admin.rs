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

/// Confirmação anti-fat-finger pra delete de calendar/addressbook. Mesmo
/// padrão dos sprints #119 e #123: o admin redigita o `name` da coleção; sem
/// match, audit `*.rejected` e 400. Calendários carregam eventos em cascata,
/// addressbooks carregam contatos — perda silenciosa por click acidental é
/// inaceitável.
#[derive(Deserialize)]
pub struct DavDeleteForm { pub confirm_name: String }

pub async fn calendar_delete_action(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path((tenant_id, id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Form(f): Form<DavDeleteForm>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny.into_response());
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;

    let actual: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM calendars WHERE tenant_id = $1 AND id = $2"
    ).bind(tenant_id).bind(id).fetch_optional(pool).await.map_err(|e| AdminError(e.into()))?;
    let Some((name,)) = actual else {
        return Err(AdminError(anyhow::anyhow!("calendar not found")));
    };
    if f.confirm_name.trim() != name {
        crate::audit::record(
            &st, &headers, &axum::http::Method::POST,
            &format!("/calendars/{tenant_id}/{id}/delete"),
            "admin.calendar.delete.rejected", Some("calendar"), Some(id.to_string()), Some(400),
            serde_json::json!({ "tenant_id": tenant_id, "reason": "confirm_name_mismatch" }),
        ).await;
        return Err(AdminError(anyhow::anyhow!(
            "confirmation failed: re-type the calendar name exactly to confirm delete"
        )));
    }

    sqlx::query("DELETE FROM calendars WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id).bind(id)
        .execute(pool).await.map_err(|e| AdminError(e.into()))?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST,
        &format!("/calendars/{tenant_id}/{id}/delete"),
        "admin.calendar.delete", Some("calendar"), Some(id.to_string()), Some(302),
        serde_json::json!({ "tenant_id": tenant_id, "name": name }),
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
    Form(f): Form<DavDeleteForm>,
) -> Result<impl IntoResponse, AdminError> {
    if let Some(deny) = crate::auth::require_tenant_match(&st, &headers, tenant_id).await {
        return Ok(deny.into_response());
    }
    let pool = st.db.as_ref().ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))?;

    let actual: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM addressbooks WHERE tenant_id = $1 AND id = $2"
    ).bind(tenant_id).bind(id).fetch_optional(pool).await.map_err(|e| AdminError(e.into()))?;
    let Some((name,)) = actual else {
        return Err(AdminError(anyhow::anyhow!("addressbook not found")));
    };
    if f.confirm_name.trim() != name {
        crate::audit::record(
            &st, &headers, &axum::http::Method::POST,
            &format!("/addressbooks/{tenant_id}/{id}/delete"),
            "admin.addressbook.delete.rejected", Some("addressbook"), Some(id.to_string()), Some(400),
            serde_json::json!({ "tenant_id": tenant_id, "reason": "confirm_name_mismatch" }),
        ).await;
        return Err(AdminError(anyhow::anyhow!(
            "confirmation failed: re-type the addressbook name exactly to confirm delete"
        )));
    }

    sqlx::query("DELETE FROM addressbooks WHERE tenant_id = $1 AND id = $2")
        .bind(tenant_id).bind(id)
        .execute(pool).await.map_err(|e| AdminError(e.into()))?;
    crate::audit::record(
        &st, &headers, &axum::http::Method::POST,
        &format!("/addressbooks/{tenant_id}/{id}/delete"),
        "admin.addressbook.delete", Some("addressbook"), Some(id.to_string()), Some(302),
        serde_json::json!({ "tenant_id": tenant_id, "name": name }),
    ).await;
    Ok(Redirect::to("/addressbooks").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn to_dav_row_maps_fields() {
        let id = Uuid::new_v4();
        let tid = Uuid::new_v4();
        let row = to_dav_row(id, tid, "Tenant".into(), "owner@ex.com".into(), "Work".into(), None, None, true, 42);
        assert_eq!(row.id, id.to_string());
        assert_eq!(row.tenant_id, tid.to_string());
        assert_eq!(row.name, "Work");
        assert!(row.is_default);
        assert_eq!(row.ctag, 42);
    }

    #[test]
    fn to_dav_row_optional_description_defaults_empty() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "o@ex.com".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.description, "");
        assert_eq!(row.color, "");
    }

    #[test]
    fn to_dav_row_optional_fields_set() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "o@ex.com".into(), "N".into(), Some("desc".into()), Some("#ff0".into()), false, 1);
        assert_eq!(row.description, "desc");
        assert_eq!(row.color, "#ff0");
    }

    #[test]
    fn to_dav_row_ctag_zero() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "o@x.com".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.ctag, 0);
    }

    #[test]
    fn calendar_edit_form_is_default_none_means_false() {
        let f = CalendarEditForm {
            name: "Test".into(),
            description: "".into(),
            color: "#fff".into(),
            is_default: None,
        };
        assert!(f.is_default.is_none());
    }

    #[test]
    fn to_dav_row_is_default_true() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "o@x.com".into(), "N".into(), None, None, true, 1);
        assert!(row.is_default);
    }

    #[test]
    fn to_dav_row_color_defaults_empty() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "o@x.com".into(), "N".into(), None, None, false, 0);
        assert!(row.color.is_empty());
    }

    #[test]
    fn to_dav_row_name_preserved() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "o@x.com".into(), "My Calendar".into(), None, None, false, 0);
        assert_eq!(row.name, "My Calendar");
    }

    #[test]
    fn to_dav_row_owner_email_preserved() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "owner@example.com".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.owner_email, "owner@example.com");
    }

    #[test]
    fn to_dav_row_is_default_false_by_default() {
        let row = to_dav_row(Uuid::new_v4(), Uuid::new_v4(), "T".into(), "e@x".into(), "N".into(), None, None, false, 0);
        assert!(!row.is_default);
    }

    #[test]
    fn to_dav_row_tenant_id_preserved() {
        let tid = Uuid::new_v4();
        let row = to_dav_row(Uuid::new_v4(), tid, "T".into(), "e@x".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.tenant_id, tid.to_string());
    }

    #[test]
    fn to_dav_row_calendar_name_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "My Calendar".into(), "u@x".into(), "u".into(), None, None, false, 0);
        assert_eq!(row.name, "My Calendar");
    }

    #[test]
    fn to_dav_row_ctag_value_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "t".into(), "e@e".into(), "n".into(), None, None, false, 42);
        assert_eq!(row.ctag, 42);
    }

    #[test]
    fn to_dav_row_owner_email_address_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "Cal".into(), "owner@corp.com".into(), "u".into(), None, None, false, 0);
        assert_eq!(row.owner_email, "owner@corp.com");
    }

    #[test]
    fn to_dav_row_is_default_true_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "Cal".into(), "o@corp.com".into(), "u".into(), None, None, true, 0);
        assert!(row.is_default);
    }

    #[test]
    fn to_dav_row_description_with_value_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "T".into(), "e@x.com".into(), "N".into(), Some("Team calendar".into()), None, false, 0);
        assert_eq!(row.description, "Team calendar");
    }

    #[test]
    fn to_dav_row_nil_description_defaults_to_empty() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "T".into(), "e@x.com".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.description, "");
    }

    #[test]
    fn to_dav_row_tenant_name_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "Acme Corp".into(), "a@b.com".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.tenant_name, "Acme Corp");
    }

    #[test]
    fn to_dav_row_color_none_defaults_empty() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "T".into(), "e@x.com".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.color, "");
    }

    #[test]
    fn to_dav_row_color_set_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "T".into(), "e@x.com".into(), "N".into(), None, Some("#0000ff".into()), false, 0);
        assert_eq!(row.color, "#0000ff");
    }

    #[test]
    fn to_dav_row_id_is_uuid_string() {
        let id = Uuid::nil();
        let row = to_dav_row(id, Uuid::nil(), "T".into(), "e@x.com".into(), "N".into(), None, None, false, 0);
        assert_eq!(row.id, "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn to_dav_row_nonzero_ctag_preserved() {
        let row = to_dav_row(Uuid::nil(), Uuid::nil(), "T".into(), "e@x.com".into(), "Cal".into(), None, None, false, 99);
        assert_eq!(row.ctag, 99);
    }
}

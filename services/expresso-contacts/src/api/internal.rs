//! Internal contacts-write endpoints for the ActiveSync bridge.
//!
//! ActiveSync runs in the mail service but must not write `contacts` directly —
//! that would bypass this service's domain logic (vCard parse, etag, email/addr
//! child-table sync). The EAS Sync handler translates a device's Contact item to
//! a minimal vCard and calls these LAN-trusted `/internal/*` endpoints, which
//! apply the write through `ContactRepo`. Mirrors the calendar service's
//! internal bridge.
//!
//! POST   /internal/contacts/items      — upsert a vCard by UID
//! DELETE /internal/contacts/items/:id  — delete a contact by id

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::ContactRepo;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/internal/contacts/items", post(upsert_contact))
        .route(
            "/internal/contacts/items/:id",
            axum::routing::delete(delete_contact),
        )
}

#[derive(Debug, Deserialize)]
pub struct UpsertContactBody {
    pub tenant_id: Uuid,
    pub addressbook_id: Uuid,
    /// Full vCard body (the EAS bridge builds this from the device's Contact
    /// item). Parsed + validated by `ContactRepo::replace_by_uid`.
    pub vcard_raw: String,
}

#[derive(Debug, Serialize)]
pub struct ContactRef {
    pub id: Uuid,
    pub uid: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteQuery {
    pub tenant_id: Uuid,
}

/// Upsert (create-or-replace by UID) a contact. Mirrors a CardDAV PUT.
async fn upsert_contact(
    State(state): State<AppState>,
    Json(body): Json<UpsertContactBody>,
) -> Result<Json<ContactRef>, Response> {
    let pool = state
        .db_or_unavailable()
        .map_err(IntoResponse::into_response)?;
    let c = ContactRepo::new(pool)
        .replace_by_uid(body.tenant_id, body.addressbook_id, &body.vcard_raw)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
    Ok(Json(ContactRef {
        id: c.id,
        uid: c.uid,
    }))
}

async fn delete_contact(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<DeleteQuery>,
) -> Result<StatusCode, Response> {
    let pool = state
        .db_or_unavailable()
        .map_err(IntoResponse::into_response)?;
    ContactRepo::new(pool)
        .delete(q.tenant_id, id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_body_deser() {
        let t = Uuid::new_v4();
        let a = Uuid::new_v4();
        let json = format!(
            r#"{{"tenant_id":"{t}","addressbook_id":"{a}","vcard_raw":"BEGIN:VCARD\nEND:VCARD"}}"#
        );
        let b: UpsertContactBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.tenant_id, t);
        assert_eq!(b.addressbook_id, a);
        assert!(b.vcard_raw.contains("VCARD"));
    }

    #[test]
    fn contact_ref_serializes() {
        let r = ContactRef {
            id: Uuid::nil(),
            uid: "c-1@x".into(),
        };
        assert!(serde_json::to_string(&r).unwrap().contains("c-1@x"));
    }
}

//! Fire-and-forget contact indexing into expresso-search (`kind = "contact"`).
//!
//! Contacts join mail and drive in the unified search index. Indexing is
//! best-effort: a search outage must never block or fail a contact write, so
//! every call spawns a detached task and ignores the result (mirrors
//! expresso-drive's `index_file_content` / expresso-mail's ingest path).

use uuid::Uuid;

use crate::domain::contact::Contact;
use crate::state::AppState;

/// Push a contact's searchable fields to the index, keyed by contact id. No-op
/// when search is unconfigured. `full_name` is the subject, `email_primary` the
/// from-address facet, and the remaining identity fields form the body so a
/// search on org/given/family/phone still hits.
pub fn index_contact(state: &AppState, c: &Contact) {
    let search_url = state.search_url();
    if search_url.is_empty() {
        return;
    }
    let body = searchable_body(c);
    let doc = serde_json::json!({
        "document_id": c.id.to_string(),
        "tenant_id":   c.tenant_id.to_string(),
        "subject":     c.full_name,
        "from_addr":   c.email_primary,
        "body":        body,
        "kind":        "contact",
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

/// The free-text body for a contact: its identity fields (given/family/org/
/// email/phone) joined by spaces, skipping absent ones. `full_name` is indexed
/// separately as the subject, so it's omitted here.
fn searchable_body(c: &Contact) -> String {
    [
        c.given_name.as_deref(),
        c.family_name.as_deref(),
        c.organization.as_deref(),
        c.email_primary.as_deref(),
        c.phone_primary.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
}

/// Remove a contact's document from the index by id. No-op when search is
/// unconfigured. Called on delete so search never returns deleted contacts.
pub fn deindex_contact(state: &AppState, contact_id: Uuid) {
    let search_url = state.search_url();
    if search_url.is_empty() {
        return;
    }
    let url = format!("{}/api/v1/index/{}", search_url, contact_id);
    let token = state.search_token().to_string();
    tokio::spawn(async move {
        let mut req = reqwest::Client::new().delete(url);
        if !token.is_empty() {
            req = req.bearer_auth(&token);
        }
        let _ = req.send().await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn contact_with(
        given: Option<&str>,
        family: Option<&str>,
        org: Option<&str>,
        email: Option<&str>,
        phone: Option<&str>,
    ) -> Contact {
        Contact {
            id: Uuid::nil(),
            addressbook_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            uid: "u@example.com".into(),
            etag: "e".into(),
            vcard_raw: String::new(),
            full_name: Some("Full Name".into()),
            family_name: family.map(str::to_owned),
            given_name: given.map(str::to_owned),
            organization: org.map(str::to_owned),
            email_primary: email.map(str::to_owned),
            phone_primary: phone.map(str::to_owned),
            created_at: datetime!(2026-05-22 08:00:00 UTC),
            updated_at: datetime!(2026-05-22 08:00:00 UTC),
        }
    }

    #[test]
    fn body_joins_present_identity_fields() {
        let c = contact_with(
            Some("Alice"),
            Some("Smith"),
            Some("Acme"),
            Some("a@x.org"),
            Some("+5511"),
        );
        assert_eq!(searchable_body(&c), "Alice Smith Acme a@x.org +5511");
    }

    #[test]
    fn body_skips_absent_fields() {
        let c = contact_with(Some("Bob"), None, None, Some("b@x.org"), None);
        assert_eq!(searchable_body(&c), "Bob b@x.org");
    }

    #[test]
    fn body_empty_when_only_full_name() {
        let c = contact_with(None, None, None, None, None);
        assert_eq!(searchable_body(&c), "");
    }
}

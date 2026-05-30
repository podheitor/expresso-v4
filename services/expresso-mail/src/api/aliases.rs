//! Email aliases — tenant-scoped `alias -> target` address forwarding.
//!
//! GET    /api/v1/mail/aliases        — list aliases for the tenant
//! POST   /api/v1/mail/aliases        — create an alias
//! GET    /api/v1/mail/aliases/:id    — fetch a single alias
//! PUT    /api/v1/mail/aliases/:id    — update target / enabled
//! DELETE /api/v1/mail/aliases/:id    — remove an alias
//!
//! Backed by the `email_aliases` table (tenant-scoped, RLS-enforced,
//! `UNIQUE (tenant_id, alias)`). An alias maps an inbound address to a
//! canonical or external `target`; this module manages the rules. The
//! delivery-side resolution (rewriting RCPT TO) is a separate concern and
//! not wired here yet.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;
use expresso_core::begin_tenant_tx;

/// RFC 5321 §4.5.3.1.3 caps a forward/reverse path at 256 octets incl. the
/// angle brackets, leaving 254 for the address itself.
pub const MAX_ADDRESS_BYTES: usize = 254;
/// Per-tenant cap to keep the alias table (and any delivery-time scan of it)
/// bounded against a runaway or hostile tenant.
pub const MAX_ALIASES_PER_TENANT: usize = 1000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/aliases", get(list_aliases).post(create_alias))
        .route(
            "/mail/aliases/:id",
            get(get_alias).put(update_alias).delete(delete_alias),
        )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alias {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub alias: String,
    pub target: String,
    pub is_enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateAliasBody {
    pub alias: String,
    pub target: String,
    pub is_enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAliasBody {
    pub target: String,
    pub is_enabled: Option<bool>,
}

fn row_to_alias(r: &sqlx::postgres::PgRow) -> Alias {
    Alias {
        id: r.get("id"),
        tenant_id: r.get("tenant_id"),
        alias: r.get("alias"),
        target: r.get("target"),
        is_enabled: r.get("is_enabled"),
    }
}

const SELECT_COLS: &str = "id, tenant_id, alias, target, is_enabled";

async fn list_aliases(State(state): State<AppState>, ctx: RequestCtx) -> Result<Json<Vec<Alias>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM email_aliases \
         WHERE tenant_id = $1 ORDER BY alias ASC"
    ))
    .bind(ctx.tenant_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows.iter().map(row_to_alias).collect()))
}

async fn get_alias(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<Alias>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM email_aliases WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.as_ref()
        .map(row_to_alias)
        .map(Json)
        .ok_or(MailError::NotFound)
}

async fn create_alias(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<CreateAliasBody>,
) -> Result<(StatusCode, Json<Alias>)> {
    let alias = normalize_address(&body.alias)?;
    let target = normalize_address(&body.target)?;
    let enabled = body.is_enabled.unwrap_or(true);

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let count: i64 = sqlx::query("SELECT count(*) AS n FROM email_aliases WHERE tenant_id = $1")
        .bind(ctx.tenant_id)
        .fetch_one(&mut *tx)
        .await?
        .get("n");
    if count as usize >= MAX_ALIASES_PER_TENANT {
        return Err(MailError::BadRequest(format!(
            "at most {MAX_ALIASES_PER_TENANT} aliases per tenant"
        )));
    }

    let existing: Option<Uuid> =
        sqlx::query("SELECT id FROM email_aliases WHERE tenant_id = $1 AND alias = $2")
            .bind(ctx.tenant_id)
            .bind(&alias)
            .fetch_optional(&mut *tx)
            .await?
            .map(|r| r.get("id"));
    if existing.is_some() {
        return Err(MailError::Conflict(format!(
            "alias already exists: {alias}"
        )));
    }

    let row = sqlx::query(&format!(
        "INSERT INTO email_aliases (tenant_id, alias, target, is_enabled) \
         VALUES ($1, $2, $3, $4) RETURNING {SELECT_COLS}"
    ))
    .bind(ctx.tenant_id)
    .bind(&alias)
    .bind(&target)
    .bind(enabled)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(row_to_alias(&row))))
}

async fn update_alias(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateAliasBody>,
) -> Result<Json<Alias>> {
    let target = normalize_address(&body.target)?;

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(&format!(
        "UPDATE email_aliases \
            SET target = $3, is_enabled = COALESCE($4, is_enabled) \
          WHERE id = $1 AND tenant_id = $2 \
          RETURNING {SELECT_COLS}"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&target)
    .bind(body.is_enabled)
    .fetch_optional(&mut *tx)
    .await?;
    let row = row.ok_or(MailError::NotFound)?;
    tx.commit().await?;
    Ok(Json(row_to_alias(&row)))
}

async fn delete_alias(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let r = sqlx::query("DELETE FROM email_aliases WHERE id = $1 AND tenant_id = $2")
        .bind(id)
        .bind(ctx.tenant_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    if r.rows_affected() == 0 {
        return Err(MailError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Validate and trim an email address for use as an alias or target.
///
/// Rejects empty, oversize (RFC 5321), CR/LF-bearing (header-injection guard),
/// and obviously malformed (`@`-less or edge-`@`) addresses. Returns the
/// trimmed form so storage is canonical.
fn normalize_address(raw: &str) -> Result<String> {
    let addr = raw.trim();
    if addr.is_empty() {
        return Err(MailError::BadRequest("address must not be empty".into()));
    }
    if addr.len() > MAX_ADDRESS_BYTES {
        return Err(MailError::BadRequest(format!(
            "address too long: {} bytes (max {MAX_ADDRESS_BYTES})",
            addr.len()
        )));
    }
    if addr.contains(['\r', '\n']) {
        return Err(MailError::BadRequest(
            "address must not contain CR/LF".into(),
        ));
    }
    let at = addr.find('@');
    let valid = match at {
        Some(i) => i > 0 && i < addr.len() - 1 && addr[i + 1..].contains('.'),
        None => false,
    };
    if !valid {
        return Err(MailError::BadRequest(format!("invalid address: {addr}")));
    }
    Ok(addr.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_address_bytes_is_rfc5321_limit() {
        assert_eq!(MAX_ADDRESS_BYTES, 254);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_aliases_per_tenant_positive() {
        assert!(MAX_ALIASES_PER_TENANT > 0);
    }

    #[test]
    fn normalize_accepts_simple_address() {
        assert_eq!(
            normalize_address("alias@example.com").unwrap(),
            "alias@example.com"
        );
    }

    #[test]
    fn normalize_trims_surrounding_whitespace() {
        assert_eq!(normalize_address("  a@b.com  ").unwrap(), "a@b.com");
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_address("").is_err());
    }

    #[test]
    fn normalize_rejects_whitespace_only() {
        assert!(normalize_address("   ").is_err());
    }

    #[test]
    fn normalize_rejects_no_at_sign() {
        assert!(normalize_address("noatsign.com").is_err());
    }

    #[test]
    fn normalize_rejects_leading_at() {
        assert!(normalize_address("@example.com").is_err());
    }

    #[test]
    fn normalize_rejects_trailing_at() {
        assert!(normalize_address("user@").is_err());
    }

    #[test]
    fn normalize_rejects_domain_without_dot() {
        // A bare hostname (no dot) is not a deliverable mail domain here.
        assert!(normalize_address("user@localhost").is_err());
    }

    #[test]
    fn normalize_rejects_oversize() {
        let long = format!("{}@example.com", "a".repeat(250));
        assert!(normalize_address(&long).is_err());
    }

    #[test]
    fn normalize_rejects_cr() {
        assert!(normalize_address("a@b.com\rEvil: x").is_err());
    }

    #[test]
    fn normalize_rejects_lf() {
        assert!(normalize_address("a@b.com\nBcc: victim@x.com").is_err());
    }

    #[test]
    fn normalize_accepts_subdomain() {
        assert!(normalize_address("user@sub.domain.com").is_ok());
    }

    #[test]
    fn normalize_accepts_plus_addressing() {
        assert!(normalize_address("user+tag@example.com").is_ok());
    }

    #[test]
    fn normalize_accepts_address_at_byte_cap() {
        let local = "a".repeat(MAX_ADDRESS_BYTES - "@x.com".len());
        let addr = format!("{local}@x.com");
        assert_eq!(addr.len(), MAX_ADDRESS_BYTES);
        assert!(normalize_address(&addr).is_ok());
    }

    #[test]
    fn alias_serde_roundtrip() {
        let a = Alias {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            alias: "a@x.com".into(),
            target: "b@y.com".into(),
            is_enabled: true,
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: Alias = serde_json::from_str(&s).unwrap();
        assert_eq!(back.alias, "a@x.com");
        assert_eq!(back.target, "b@y.com");
        assert!(back.is_enabled);
    }

    #[test]
    fn alias_serde_json_contains_target_key() {
        let a = Alias {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            alias: "a@x.com".into(),
            target: "b@y.com".into(),
            is_enabled: true,
        };
        assert!(serde_json::to_string(&a).unwrap().contains("target"));
    }

    #[test]
    fn create_body_is_enabled_optional() {
        let b: CreateAliasBody =
            serde_json::from_str(r#"{"alias":"a@x.com","target":"b@y.com"}"#).unwrap();
        assert!(b.is_enabled.is_none());
    }

    #[test]
    fn create_body_is_enabled_false() {
        let b: CreateAliasBody =
            serde_json::from_str(r#"{"alias":"a@x.com","target":"b@y.com","is_enabled":false}"#)
                .unwrap();
        assert_eq!(b.is_enabled, Some(false));
    }

    #[test]
    fn create_body_fields_preserved() {
        let b: CreateAliasBody =
            serde_json::from_str(r#"{"alias":"hello@x.com","target":"box@y.com"}"#).unwrap();
        assert_eq!(b.alias, "hello@x.com");
        assert_eq!(b.target, "box@y.com");
    }

    #[test]
    fn update_body_target_required() {
        let b: UpdateAliasBody = serde_json::from_str(r#"{"target":"new@y.com"}"#).unwrap();
        assert_eq!(b.target, "new@y.com");
        assert!(b.is_enabled.is_none());
    }
}

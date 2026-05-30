//! Tenant domains — the mail domains a tenant owns.
//!
//! GET    /api/v1/mail/domains          — list the tenant's domains
//! POST   /api/v1/mail/domains          — claim a domain (starts unverified)
//! GET    /api/v1/mail/domains/:id      — fetch a single domain
//! POST   /api/v1/mail/domains/:id/verify — mark verified (DNS TXT check is a
//!                                          follow-up; this flips the state)
//! DELETE /api/v1/mail/domains/:id      — release a domain
//!
//! Backed by the `tenant_domains` table (RLS-enforced, `UNIQUE (domain)` so a
//! domain is claimable by at most one tenant globally). A claimed domain starts
//! unverified with a random `verify_token` the tenant publishes as a DNS TXT
//! record; verification proves ownership before the domain is trusted for
//! delivery/DKIM/alias validation.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;
use expresso_core::begin_tenant_tx;

/// RFC 1035 caps a domain name at 253 octets.
pub const MAX_DOMAIN_BYTES: usize = 253;
/// Per-tenant cap to bound the table against a runaway tenant.
pub const MAX_DOMAINS_PER_TENANT: usize = 100;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/domains", post(create_domain).get(list_domains))
        .route("/mail/domains/:id", get(get_domain).delete(delete_domain))
        .route("/mail/domains/:id/verify", post(verify_domain))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantDomain {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub domain: String,
    pub is_verified: bool,
    pub verify_token: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateDomainBody {
    pub domain: String,
}

fn row_to_domain(r: &sqlx::postgres::PgRow) -> TenantDomain {
    TenantDomain {
        id: r.get("id"),
        tenant_id: r.get("tenant_id"),
        domain: r.get("domain"),
        is_verified: r.get("is_verified"),
        verify_token: r.get("verify_token"),
    }
}

const SELECT_COLS: &str = "id, tenant_id, domain, is_verified, verify_token";

async fn list_domains(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vec<TenantDomain>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM tenant_domains \
         WHERE tenant_id = $1 ORDER BY domain ASC"
    ))
    .bind(ctx.tenant_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows.iter().map(row_to_domain).collect()))
}

async fn get_domain(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<TenantDomain>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM tenant_domains WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.as_ref()
        .map(row_to_domain)
        .map(Json)
        .ok_or(MailError::NotFound)
}

async fn create_domain(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(body): Json<CreateDomainBody>,
) -> Result<(StatusCode, Json<TenantDomain>)> {
    let domain = normalize_domain(&body.domain)?;
    let token = format!("expresso-verify={}", Uuid::new_v4().simple());

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let count: i64 = sqlx::query("SELECT count(*) AS n FROM tenant_domains WHERE tenant_id = $1")
        .bind(ctx.tenant_id)
        .fetch_one(&mut *tx)
        .await?
        .get("n");
    if count as usize >= MAX_DOMAINS_PER_TENANT {
        return Err(MailError::BadRequest(format!(
            "at most {MAX_DOMAINS_PER_TENANT} domains per tenant"
        )));
    }

    // UNIQUE(domain) is global — surface a clash as 409 rather than a 500.
    let taken: Option<Uuid> = sqlx::query("SELECT id FROM tenant_domains WHERE domain = $1")
        .bind(&domain)
        .fetch_optional(&mut *tx)
        .await?
        .map(|r| r.get("id"));
    if taken.is_some() {
        return Err(MailError::Conflict(format!(
            "domain already claimed: {domain}"
        )));
    }

    let row = sqlx::query(&format!(
        "INSERT INTO tenant_domains (tenant_id, domain, verify_token) \
         VALUES ($1, $2, $3) RETURNING {SELECT_COLS}"
    ))
    .bind(ctx.tenant_id)
    .bind(&domain)
    .bind(&token)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(row_to_domain(&row))))
}

async fn verify_domain(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<Json<TenantDomain>> {
    // Fetch the pending domain + its expected token.
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM tenant_domains WHERE id = $1 AND tenant_id = $2"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    let current = row.as_ref().map(row_to_domain).ok_or(MailError::NotFound)?;
    tx.commit().await?;

    // Already verified — idempotent success, no DNS round-trip.
    if current.is_verified {
        return Ok(Json(current));
    }

    // Prove ownership: the tenant must publish verify_token as a TXT record on
    // the domain. A miss/transient failure is a clean 400, not a 500.
    let ok = expresso_mail_auth::verify_domain_txt(&current.domain, &current.verify_token)
        .await
        .map_err(|e| MailError::BadRequest(format!("dns verification error: {e}")))?;
    if !ok {
        return Err(MailError::BadRequest(format!(
            "TXT record not found; publish \"{}\" on {} and retry",
            current.verify_token, current.domain
        )));
    }

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(&format!(
        "UPDATE tenant_domains \
            SET is_verified = true, verified_at = now() \
          WHERE id = $1 AND tenant_id = $2 \
          RETURNING {SELECT_COLS}"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    let row = row.ok_or(MailError::NotFound)?;
    tx.commit().await?;
    Ok(Json(row_to_domain(&row)))
}

async fn delete_domain(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let r = sqlx::query("DELETE FROM tenant_domains WHERE id = $1 AND tenant_id = $2")
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

/// Validate and canonicalize a domain name: trim, lowercase, reject empty,
/// oversize (RFC 1035), CR/LF (defense-in-depth), and obviously malformed
/// (no dot, leading/trailing dot or hyphen, `@`-bearing) names.
fn normalize_domain(raw: &str) -> Result<String> {
    let domain = raw.trim().to_ascii_lowercase();
    if domain.is_empty() {
        return Err(MailError::BadRequest("domain must not be empty".into()));
    }
    if domain.len() > MAX_DOMAIN_BYTES {
        return Err(MailError::BadRequest(format!(
            "domain too long: {} bytes (max {MAX_DOMAIN_BYTES})",
            domain.len()
        )));
    }
    if domain.contains(['\r', '\n', '@', ' ']) {
        return Err(MailError::BadRequest(
            "domain contains invalid characters".into(),
        ));
    }
    let labels: Vec<&str> = domain.split('.').collect();
    let valid = labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    if !valid {
        return Err(MailError::BadRequest(format!("invalid domain: {domain}")));
    }
    Ok(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_domain_bytes_is_rfc1035_limit() {
        assert_eq!(MAX_DOMAIN_BYTES, 253);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn max_domains_per_tenant_positive() {
        assert!(MAX_DOMAINS_PER_TENANT > 0);
    }

    #[test]
    fn normalize_accepts_simple_domain() {
        assert_eq!(normalize_domain("example.com").unwrap(), "example.com");
    }

    #[test]
    fn normalize_lowercases_and_trims() {
        assert_eq!(normalize_domain("  Example.COM  ").unwrap(), "example.com");
    }

    #[test]
    fn normalize_accepts_subdomain() {
        assert!(normalize_domain("mail.example.com").is_ok());
    }

    #[test]
    fn normalize_accepts_hyphen_in_label() {
        assert!(normalize_domain("my-corp.example.com").is_ok());
    }

    #[test]
    fn normalize_rejects_empty() {
        assert!(normalize_domain("").is_err());
    }

    #[test]
    fn normalize_rejects_whitespace_only() {
        assert!(normalize_domain("   ").is_err());
    }

    #[test]
    fn normalize_rejects_no_dot() {
        assert!(normalize_domain("localhost").is_err());
    }

    #[test]
    fn normalize_rejects_leading_dot() {
        assert!(normalize_domain(".example.com").is_err());
    }

    #[test]
    fn normalize_rejects_trailing_dot() {
        assert!(normalize_domain("example.com.").is_err());
    }

    #[test]
    fn normalize_rejects_at_sign() {
        assert!(normalize_domain("user@example.com").is_err());
    }

    #[test]
    fn normalize_rejects_leading_hyphen_label() {
        assert!(normalize_domain("-bad.example.com").is_err());
    }

    #[test]
    fn normalize_rejects_trailing_hyphen_label() {
        assert!(normalize_domain("bad-.example.com").is_err());
    }

    #[test]
    fn normalize_rejects_cr_lf() {
        assert!(normalize_domain("a.com\r\nEvil: x").is_err());
    }

    #[test]
    fn normalize_rejects_underscore() {
        assert!(normalize_domain("bad_label.example.com").is_err());
    }

    #[test]
    fn normalize_rejects_oversize() {
        let long = format!("{}.com", "a".repeat(300));
        assert!(normalize_domain(&long).is_err());
    }

    #[test]
    fn normalize_rejects_oversize_label() {
        let long_label = format!("{}.com", "a".repeat(64));
        assert!(normalize_domain(&long_label).is_err());
    }

    #[test]
    fn create_body_domain_preserved() {
        let b: CreateDomainBody = serde_json::from_str(r#"{"domain":"acme.com"}"#).unwrap();
        assert_eq!(b.domain, "acme.com");
    }

    #[test]
    fn domain_serde_roundtrip() {
        let d = TenantDomain {
            id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            domain: "acme.com".into(),
            is_verified: false,
            verify_token: "expresso-verify=abc".into(),
        };
        let s = serde_json::to_string(&d).unwrap();
        let back: TenantDomain = serde_json::from_str(&s).unwrap();
        assert_eq!(back.domain, "acme.com");
        assert!(!back.is_verified);
        assert!(back.verify_token.starts_with("expresso-verify="));
    }
}

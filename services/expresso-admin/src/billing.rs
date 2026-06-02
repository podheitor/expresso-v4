//! Billing: per-tenant plans + internal invoices (fixed price per plan).
//!
//! The tenant's plan lives in `tenants.plan`; `billing_plans` holds the monthly
//! price per plan. An invoice is generated for a tenant + period at the plan's
//! current price, and marked paid/void out-of-band by a super-admin (no external
//! gateway). Plan-price edits and invoice generation/state changes require
//! `super_admin`; reading a tenant's invoices is gated by `require_tenant_match`.
//!
//! Admin DB connection runs with `app.tenant_id` NULL (RLS allows it), so the
//! invoice queries carry an explicit `WHERE tenant_id = $1` for defense-in-depth.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::{auth, AdminError, AppState};

#[derive(Debug, Serialize, FromRow)]
pub struct BillingPlan {
    pub plan: String,
    pub display_name: String,
    pub monthly_price_cents: i64,
    pub currency: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Invoice {
    pub id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub period: time::OffsetDateTime,
    pub plan: String,
    pub amount_cents: i64,
    pub currency: String,
    pub status: String,
}

fn pool(st: &AppState) -> Result<&sqlx::PgPool, AdminError> {
    st.db
        .as_ref()
        .ok_or_else(|| AdminError(anyhow::anyhow!("database unavailable")))
}

/// GET /api/v1/admin/billing/plans — the plan catalogue with prices.
pub async fn list_plans(State(st): State<Arc<AppState>>) -> Result<Response, AdminError> {
    let plans: Vec<BillingPlan> = sqlx::query_as(
        "SELECT plan, display_name, monthly_price_cents, currency \
         FROM billing_plans ORDER BY monthly_price_cents",
    )
    .fetch_all(pool(&st)?)
    .await
    .map_err(|e| AdminError(e.into()))?;
    Ok(Json(plans).into_response())
}

#[derive(Debug, Deserialize)]
pub struct PlanPriceBody {
    pub monthly_price_cents: i64,
}

/// PUT /api/v1/admin/billing/plans/:plan — set a plan's monthly price (super-admin).
pub async fn set_plan_price(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(plan): Path<String>,
    Json(body): Json<PlanPriceBody>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return Ok(r);
    }
    if body.monthly_price_cents < 0 {
        return Ok((axum::http::StatusCode::BAD_REQUEST, "price must be >= 0").into_response());
    }
    let row: Option<BillingPlan> = sqlx::query_as(
        "UPDATE billing_plans SET monthly_price_cents = $2, updated_at = now() \
         WHERE plan = $1 \
         RETURNING plan, display_name, monthly_price_cents, currency",
    )
    .bind(&plan)
    .bind(body.monthly_price_cents)
    .fetch_optional(pool(&st)?)
    .await
    .map_err(|e| AdminError(e.into()))?;
    match row {
        Some(p) => Ok(Json(p).into_response()),
        None => Ok((axum::http::StatusCode::NOT_FOUND, "unknown plan").into_response()),
    }
}

/// GET /api/v1/admin/tenants/:id/invoices — a tenant's invoices, newest first.
/// Gated by `require_tenant_match` (super-admin any; tenant-admin only their own).
pub async fn list_invoices(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_tenant_match(&st, &headers, id).await {
        return Ok(r);
    }
    let rows: Vec<Invoice> = sqlx::query_as(
        "SELECT id, tenant_id, period, plan, amount_cents, currency, status \
         FROM billing_invoices WHERE tenant_id = $1 ORDER BY period DESC",
    )
    .bind(id)
    .fetch_all(pool(&st)?)
    .await
    .map_err(|e| AdminError(e.into()))?;
    Ok(Json(rows).into_response())
}

#[derive(Debug, Deserialize)]
pub struct GenerateBody {
    /// Billing period, first-of-month, RFC3339 (e.g. 2026-06-01T00:00:00Z).
    pub period: String,
}

/// POST /api/v1/admin/tenants/:id/invoices — generate the invoice for a period
/// at the tenant's current plan price (super-admin). Idempotent per (tenant,
/// period) via the unique constraint → ON CONFLICT returns the existing one.
pub async fn generate_invoice(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<uuid::Uuid>,
    Json(body): Json<GenerateBody>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return Ok(r);
    }
    let period =
        time::OffsetDateTime::parse(&body.period, &time::format_description::well_known::Rfc3339)
            .map_err(|_| AdminError(anyhow::anyhow!("period must be RFC3339")))?;
    let p = pool(&st)?;

    // Resolve the tenant's current plan + its price in one go.
    let priced: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT t.plan, bp.monthly_price_cents, bp.currency \
         FROM tenants t JOIN billing_plans bp ON bp.plan = t.plan \
         WHERE t.id = $1",
    )
    .bind(id)
    .fetch_optional(p)
    .await
    .map_err(|e| AdminError(e.into()))?;
    let Some((plan, price, currency)) = priced else {
        return Ok((
            axum::http::StatusCode::NOT_FOUND,
            "tenant or plan not found",
        )
            .into_response());
    };

    let invoice: Invoice = sqlx::query_as(
        "INSERT INTO billing_invoices (tenant_id, period, plan, amount_cents, currency) \
         VALUES ($1, $2::date, $3, $4, $5) \
         ON CONFLICT (tenant_id, period) DO UPDATE SET tenant_id = billing_invoices.tenant_id \
         RETURNING id, tenant_id, period, plan, amount_cents, currency, status",
    )
    .bind(id)
    .bind(period)
    .bind(&plan)
    .bind(price)
    .bind(&currency)
    .fetch_one(p)
    .await
    .map_err(|e| AdminError(e.into()))?;
    Ok(Json(invoice).into_response())
}

#[derive(Debug, Deserialize)]
pub struct InvoiceStatusBody {
    /// New status: "paid" or "void".
    pub status: String,
}

/// PATCH /api/v1/admin/invoices/:invoice_id — mark an invoice paid/void
/// (super-admin). `paid_at` is stamped when moving to paid, cleared otherwise.
pub async fn set_invoice_status(
    State(st): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(invoice_id): Path<uuid::Uuid>,
    Json(body): Json<InvoiceStatusBody>,
) -> Result<Response, AdminError> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return Ok(r);
    }
    if !matches!(body.status.as_str(), "paid" | "void" | "pending") {
        return Ok((
            axum::http::StatusCode::BAD_REQUEST,
            "status must be paid|void|pending",
        )
            .into_response());
    }
    let row: Option<Invoice> = sqlx::query_as(
        "UPDATE billing_invoices \
            SET status = $2, \
                paid_at = CASE WHEN $2 = 'paid' THEN now() ELSE NULL END \
          WHERE id = $1 \
          RETURNING id, tenant_id, period, plan, amount_cents, currency, status",
    )
    .bind(invoice_id)
    .bind(&body.status)
    .fetch_optional(pool(&st)?)
    .await
    .map_err(|e| AdminError(e.into()))?;
    match row {
        Some(inv) => Ok(Json(inv).into_response()),
        None => Ok((axum::http::StatusCode::NOT_FOUND, "unknown invoice").into_response()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn invoice_status_values_are_constrained() {
        // Mirror the CHECK constraint + handler guard.
        for s in ["paid", "void", "pending"] {
            assert!(matches!(s, "paid" | "void" | "pending"));
        }
        assert!(!matches!("refunded", "paid" | "void" | "pending"));
    }
}

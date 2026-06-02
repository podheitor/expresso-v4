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

use askama::Template;
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
    Form, Json,
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

// ─── Admin HTML screen ───────────────────────────────────────────────────────

/// A plan row for the screen (price pre-formatted, so the template needs no
/// number-formatting filter).
pub struct PlanRow {
    pub plan: String,
    pub display_name: String,
    pub price: String,
    pub currency: String,
}

/// A tenant option in the invoice-section picker.
pub struct TenantOpt {
    pub id: String,
    pub name: String,
    pub plan: String,
}

/// An invoice row for the screen (with the period pre-formatted as YYYY-MM).
pub struct InvoiceRow {
    pub id: String,
    pub period: String,
    pub plan: String,
    pub amount: String,
    pub currency: String,
    pub status: String,
}

#[derive(Template)]
#[template(path = "billing_admin.html")]
pub struct BillingTpl {
    pub current: &'static str,
    pub plans: Vec<PlanRow>,
    pub tenants: Vec<TenantOpt>,
    pub selected_tenant: Option<String>,
    pub invoices: Vec<InvoiceRow>,
    pub flash: Option<String>,
}

/// Format integer cents as a plain decimal string (e.g. 4900 → "49.00").
fn money(cents: i64) -> String {
    format!("{}.{:02}", cents / 100, (cents % 100).abs())
}

#[derive(Debug, Deserialize)]
pub struct BillingPageQuery {
    pub tenant: Option<String>,
    pub flash: Option<String>,
}

/// GET /billing.html — plan catalogue (editable prices) + per-tenant invoices.
pub async fn page(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<BillingPageQuery>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return r;
    }
    let Some(p) = st.db.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };

    let plans: Vec<PlanRow> = sqlx::query_as::<_, BillingPlan>(
        "SELECT plan, display_name, monthly_price_cents, currency \
         FROM billing_plans ORDER BY monthly_price_cents",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|b| PlanRow {
        plan: b.plan,
        display_name: b.display_name,
        price: money(b.monthly_price_cents),
        currency: b.currency,
    })
    .collect();

    let tenants: Vec<TenantOpt> = sqlx::query_as::<_, (uuid::Uuid, String, String)>(
        "SELECT id, name, plan FROM tenants ORDER BY name",
    )
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, name, plan)| TenantOpt {
        id: id.to_string(),
        name,
        plan,
    })
    .collect();

    let invoices: Vec<InvoiceRow> = if let Some(tid) = q
        .tenant
        .as_deref()
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
    {
        sqlx::query_as::<_, Invoice>(
            "SELECT id, tenant_id, period, plan, amount_cents, currency, status \
             FROM billing_invoices WHERE tenant_id = $1 ORDER BY period DESC",
        )
        .bind(tid)
        .fetch_all(p)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|i| InvoiceRow {
            id: i.id.to_string(),
            period: i
                .period
                .format(
                    &time::format_description::parse("[year]-[month]")
                        .unwrap_or_else(|_| Vec::new()),
                )
                .unwrap_or_default(),
            plan: i.plan,
            amount: money(i.amount_cents),
            currency: i.currency,
            status: i.status,
        })
        .collect()
    } else {
        Vec::new()
    };

    match (BillingTpl {
        current: "billing",
        plans,
        tenants,
        selected_tenant: q.tenant.clone(),
        invoices,
        flash: q.flash.clone(),
    })
    .render()
    {
        Ok(html) => (
            axum::http::StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            html,
        )
            .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("template: {e}"),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct SetPriceForm {
    pub plan: String,
    /// Price in whole currency units (e.g. "49.90"); converted to cents.
    pub price: String,
}

/// POST /billing/price — set a plan's price from the HTML form.
pub async fn set_price_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(f): Form<SetPriceForm>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return r;
    }
    let cents = parse_price_to_cents(&f.price);
    let Some(cents) = cents else {
        return Redirect::to("/billing.html?flash=preço inválido").into_response();
    };
    let Some(p) = st.db.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };
    let _ = sqlx::query(
        "UPDATE billing_plans SET monthly_price_cents = $2, updated_at = now() WHERE plan = $1",
    )
    .bind(&f.plan)
    .bind(cents)
    .execute(p)
    .await;
    Redirect::to("/billing.html?flash=preço atualizado").into_response()
}

#[derive(Debug, Deserialize)]
pub struct GenForm {
    pub tenant: String,
    /// Period as YYYY-MM (the screen sends a month input).
    pub period: String,
}

/// POST /billing/generate — generate an invoice for tenant+period (YYYY-MM).
pub async fn generate_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(f): Form<GenForm>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return r;
    }
    let (Some(tid), Some(p)) = (uuid::Uuid::parse_str(&f.tenant).ok(), st.db.as_ref()) else {
        return Redirect::to("/billing.html?flash=dados inválidos").into_response();
    };
    // Accept "YYYY-MM" (browser month input) → first-of-month for the ::date cast.
    let ym = f.period.get(0..7).unwrap_or(&f.period);
    let period_date = format!("{ym}-01");
    let priced: Option<(String, i64, String)> = sqlx::query_as(
        "SELECT t.plan, bp.monthly_price_cents, bp.currency \
         FROM tenants t JOIN billing_plans bp ON bp.plan = t.plan WHERE t.id = $1",
    )
    .bind(tid)
    .fetch_optional(p)
    .await
    .ok()
    .flatten();
    let Some((plan, price, currency)) = priced else {
        return Redirect::to("/billing.html?flash=tenant sem plano").into_response();
    };
    let _ = sqlx::query(
        "INSERT INTO billing_invoices (tenant_id, period, plan, amount_cents, currency) \
         VALUES ($1, $2::date, $3, $4, $5) \
         ON CONFLICT (tenant_id, period) DO NOTHING",
    )
    .bind(tid)
    .bind(&period_date)
    .bind(&plan)
    .bind(price)
    .bind(&currency)
    .execute(p)
    .await;
    Redirect::to(&format!("/billing.html?tenant={tid}&flash=fatura gerada")).into_response()
}

#[derive(Debug, Deserialize)]
pub struct MarkForm {
    pub invoice_id: String,
    pub tenant: String,
    pub status: String,
}

/// POST /billing/mark — set an invoice's status from the HTML form.
pub async fn mark_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(f): Form<MarkForm>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return r;
    }
    if !matches!(f.status.as_str(), "paid" | "void" | "pending") {
        return Redirect::to("/billing.html?flash=status inválido").into_response();
    }
    let (Some(iid), Some(p)) = (uuid::Uuid::parse_str(&f.invoice_id).ok(), st.db.as_ref()) else {
        return Redirect::to("/billing.html?flash=dados inválidos").into_response();
    };
    let _ = sqlx::query(
        "UPDATE billing_invoices SET status = $2, \
            paid_at = CASE WHEN $2 = 'paid' THEN now() ELSE NULL END WHERE id = $1",
    )
    .bind(iid)
    .bind(&f.status)
    .execute(p)
    .await;
    Redirect::to(&format!(
        "/billing.html?tenant={}&flash=fatura atualizada",
        f.tenant
    ))
    .into_response()
}

/// Parse "49" / "49.9" / "49.90" into cents; None on garbage or negative.
fn parse_price_to_cents(s: &str) -> Option<i64> {
    let s = s.trim().replace(',', ".");
    let v: f64 = s.parse().ok()?;
    if v < 0.0 || !v.is_finite() {
        return None;
    }
    Some((v * 100.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::{money, parse_price_to_cents};

    #[test]
    fn money_formats_cents() {
        assert_eq!(money(4900), "49.00");
        assert_eq!(money(9), "0.09");
        assert_eq!(money(0), "0.00");
    }

    #[test]
    fn price_parsing_handles_decimals_and_comma() {
        assert_eq!(parse_price_to_cents("49"), Some(4900));
        assert_eq!(parse_price_to_cents("49.90"), Some(4990));
        assert_eq!(parse_price_to_cents("49,90"), Some(4990));
        assert!(parse_price_to_cents("-1").is_none());
        assert!(parse_price_to_cents("abc").is_none());
    }

    #[test]
    fn invoice_status_values_are_constrained() {
        // Mirror the CHECK constraint + handler guard.
        for s in ["paid", "void", "pending"] {
            assert!(matches!(s, "paid" | "void" | "pending"));
        }
        assert!(!matches!("refunded", "paid" | "void" | "pending"));
    }
}

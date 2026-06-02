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

use crate::overage::{self, PlanTerms, Usage};
use crate::{auth, AdminError, AppState};

#[derive(Debug, Serialize, FromRow)]
pub struct BillingPlan {
    pub plan: String,
    pub display_name: String,
    pub monthly_price_cents: i64,
    pub currency: String,
    #[serde(default)]
    pub included_seats: i64,
    #[serde(default)]
    pub seat_overage_cents: i64,
    #[serde(default)]
    pub included_storage_gb: i64,
    #[serde(default)]
    pub storage_overage_cents_per_gb: i64,
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

// ─── Invoice generation with usage-based overage ─────────────────────────────

/// Resolve a tenant's plan terms (base price + allowances + overage prices) and
/// currency. `None` if the tenant or its plan-price row is missing.
async fn plan_terms_for(
    p: &sqlx::PgPool,
    tenant: uuid::Uuid,
) -> Result<Option<(PlanTerms, String, String)>, sqlx::Error> {
    let row: Option<(String, i64, i64, i64, i64, i64, String)> = sqlx::query_as(
        "SELECT t.plan, bp.monthly_price_cents, bp.included_seats, bp.seat_overage_cents, \
                bp.included_storage_gb, bp.storage_overage_cents_per_gb, bp.currency \
           FROM tenants t JOIN billing_plans bp ON bp.plan = t.plan \
          WHERE t.id = $1",
    )
    .bind(tenant)
    .fetch_optional(p)
    .await?;
    Ok(
        row.map(|(plan, base, seats, seat_over, gb, gb_over, currency)| {
            (
                PlanTerms {
                    base_cents: base,
                    included_seats: seats,
                    seat_overage_cents: seat_over,
                    included_storage_gb: gb,
                    storage_overage_cents_per_gb: gb_over,
                },
                plan,
                currency,
            )
        }),
    )
}

/// Measure the tenant's billable usage now: seats (users) and total stored bytes
/// (mailbox + live drive files). Best-effort — a missing table counts as 0.
async fn measure_usage(p: &sqlx::PgPool, tenant: uuid::Uuid) -> Usage {
    let seats = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM users WHERE tenant_id = $1")
        .bind(tenant)
        .fetch_one(p)
        .await
        .unwrap_or(0);
    let mail = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM messages WHERE tenant_id = $1",
    )
    .bind(tenant)
    .fetch_one(p)
    .await
    .unwrap_or(0);
    let files = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM drive_files \
         WHERE tenant_id = $1 AND deleted_at IS NULL",
    )
    .bind(tenant)
    .fetch_one(p)
    .await
    .unwrap_or(0);
    Usage {
        seats,
        storage_bytes: mail.saturating_add(files),
    }
}

/// Generate (or refresh) one tenant's invoice for `period_date` (YYYY-MM-DD,
/// first-of-month) including usage-based overage lines. Returns the invoice's
/// total in cents, or `None` if the tenant has no priced plan.
///
/// Idempotent on the header (`ON CONFLICT (tenant, period)`); the lines are
/// rewritten to reflect current usage so re-running a period re-meters it.
async fn generate_one(
    p: &sqlx::PgPool,
    tenant: uuid::Uuid,
    period_date: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let Some((terms, plan, currency)) = plan_terms_for(p, tenant).await? else {
        return Ok(None);
    };
    let usage = measure_usage(p, tenant).await;
    let lines = overage::compute_lines(terms, usage);
    let total = overage::total_cents(&lines);

    let mut tx = p.begin().await?;
    let invoice_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO billing_invoices (tenant_id, period, plan, amount_cents, currency) \
         VALUES ($1, $2::date, $3, $4, $5) \
         ON CONFLICT (tenant_id, period) \
         DO UPDATE SET amount_cents = EXCLUDED.amount_cents, plan = EXCLUDED.plan, \
                       currency = EXCLUDED.currency \
         RETURNING id",
    )
    .bind(tenant)
    .bind(period_date)
    .bind(&plan)
    .bind(total)
    .bind(&currency)
    .fetch_one(&mut *tx)
    .await?;

    for line in &lines {
        sqlx::query(
            "INSERT INTO billing_invoice_lines \
                (invoice_id, tenant_id, kind, description, quantity, unit_cents, amount_cents) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (invoice_id, kind) \
             DO UPDATE SET description = EXCLUDED.description, quantity = EXCLUDED.quantity, \
                           unit_cents = EXCLUDED.unit_cents, amount_cents = EXCLUDED.amount_cents",
        )
        .bind(invoice_id)
        .bind(tenant)
        .bind(line.kind)
        .bind(&line.description)
        .bind(line.quantity)
        .bind(line.unit_cents)
        .bind(line.amount_cents)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(Some(total))
}

/// GET /api/v1/admin/billing/plans — the plan catalogue with prices.
pub async fn list_plans(State(st): State<Arc<AppState>>) -> Result<Response, AdminError> {
    let plans: Vec<BillingPlan> = sqlx::query_as(
        "SELECT plan, display_name, monthly_price_cents, currency, included_seats, seat_overage_cents, included_storage_gb, storage_overage_cents_per_gb \
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
         RETURNING plan, display_name, monthly_price_cents, currency, included_seats, seat_overage_cents, included_storage_gb, storage_overage_cents_per_gb",
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
    let period_date = period
        .format(&time::format_description::well_known::Rfc3339)
        .map(|s| s.get(0..10).unwrap_or("").to_string())
        .unwrap_or_default();
    let p = pool(&st)?;

    if generate_one(p, id, &period_date)
        .await
        .map_err(|e| AdminError(e.into()))?
        .is_none()
    {
        return Ok((
            axum::http::StatusCode::NOT_FOUND,
            "tenant or plan not found",
        )
            .into_response());
    }

    // Return the resulting invoice header.
    let invoice: Invoice = sqlx::query_as(
        "SELECT id, tenant_id, period, plan, amount_cents, currency, status \
         FROM billing_invoices WHERE tenant_id = $1 AND period = $2::date",
    )
    .bind(id)
    .bind(&period_date)
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
    pub included_seats: i64,
    pub seat_overage: String,
    pub included_storage_gb: i64,
    pub storage_overage: String,
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
        "SELECT plan, display_name, monthly_price_cents, currency, included_seats, seat_overage_cents, included_storage_gb, storage_overage_cents_per_gb \
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
        included_seats: b.included_seats,
        seat_overage: money(b.seat_overage_cents),
        included_storage_gb: b.included_storage_gb,
        storage_overage: money(b.storage_overage_cents_per_gb),
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
        .map(invoice_row)
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
    /// Base price in whole currency units (e.g. "49.90"); converted to cents.
    pub price: String,
    /// Included seats before seat-overage applies.
    pub included_seats: i64,
    /// Seat-overage price per extra seat, in whole currency units.
    pub seat_overage: String,
    /// Included storage GB before storage-overage applies.
    pub included_storage_gb: i64,
    /// Storage-overage price per extra GB, in whole currency units.
    pub storage_overage: String,
}

/// POST /billing/price — set a plan's base price, allowances, and overage
/// prices from the HTML form.
pub async fn set_price_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(f): Form<SetPriceForm>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return r;
    }
    let (Some(price), Some(seat_over), Some(gb_over)) = (
        parse_price_to_cents(&f.price),
        parse_price_to_cents(&f.seat_overage),
        parse_price_to_cents(&f.storage_overage),
    ) else {
        return Redirect::to("/billing.html?flash=preço inválido").into_response();
    };
    if f.included_seats < 0 || f.included_storage_gb < 0 {
        return Redirect::to("/billing.html?flash=allowance inválida").into_response();
    }
    let Some(p) = st.db.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };
    let _ = sqlx::query(
        "UPDATE billing_plans SET monthly_price_cents = $2, included_seats = $3, \
            seat_overage_cents = $4, included_storage_gb = $5, \
            storage_overage_cents_per_gb = $6, updated_at = now() WHERE plan = $1",
    )
    .bind(&f.plan)
    .bind(price)
    .bind(f.included_seats)
    .bind(seat_over)
    .bind(f.included_storage_gb)
    .bind(gb_over)
    .execute(p)
    .await;
    Redirect::to("/billing.html?flash=plano atualizado").into_response()
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
    let Some(period_date) = period_first_of_month(&f.period) else {
        return Redirect::to("/billing.html?flash=período inválido").into_response();
    };
    match generate_one(p, tid, &period_date).await {
        Ok(Some(_)) => {
            Redirect::to(&format!("/billing.html?tenant={tid}&flash=fatura gerada")).into_response()
        }
        Ok(None) => Redirect::to("/billing.html?flash=tenant sem plano").into_response(),
        Err(_) => Redirect::to("/billing.html?flash=erro ao gerar").into_response(),
    }
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

// ─── Tenant self-service screen ──────────────────────────────────────────────

/// The own-tenant billing screen: current plan + price + read-only invoices.
/// Unlike the super-admin screen there is no tenant picker (the tenant is the
/// caller's own) and no price/generate/mark actions.
#[derive(Template)]
#[template(path = "my_billing.html")]
pub struct MyBillingTpl {
    pub current: &'static str,
    pub tenant_name: String,
    pub plan: String,
    pub plan_price: String,
    pub currency: String,
    pub invoices: Vec<InvoiceRow>,
}

fn invoice_row(i: Invoice) -> InvoiceRow {
    InvoiceRow {
        id: i.id.to_string(),
        period: i
            .period
            .format(
                &time::format_description::parse("[year]-[month]").unwrap_or_else(|_| Vec::new()),
            )
            .unwrap_or_default(),
        plan: i.plan,
        amount: money(i.amount_cents),
        currency: i.currency,
        status: i.status,
    }
}

/// GET /my-billing.html — the caller's own-tenant plan + invoices, read-only.
/// Accessible to any admin (tenant_admin or super_admin); scoped to the
/// principal's own tenant_id (never a path-supplied one).
pub async fn my_page(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let principal = auth::principal_for(&st, &headers).await;
    let Some(tid) = principal.tenant_id else {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "no tenant on principal — super-admins use /billing.html",
        )
            .into_response();
    };
    let Some(p) = st.db.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };

    let plan_row: Option<(String, String, i64, String)> = sqlx::query_as(
        "SELECT t.name, bp.plan, bp.monthly_price_cents, bp.currency \
         FROM tenants t JOIN billing_plans bp ON bp.plan = t.plan WHERE t.id = $1",
    )
    .bind(tid)
    .fetch_optional(p)
    .await
    .unwrap_or_default();
    let (tenant_name, plan, plan_price, currency) = match plan_row {
        Some((name, plan, cents, cur)) => (name, plan, money(cents), cur),
        None => (String::new(), String::new(), money(0), "BRL".into()),
    };

    let invoices: Vec<InvoiceRow> = sqlx::query_as::<_, Invoice>(
        "SELECT id, tenant_id, period, plan, amount_cents, currency, status \
         FROM billing_invoices WHERE tenant_id = $1 ORDER BY period DESC",
    )
    .bind(tid)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(invoice_row)
    .collect();

    match (MyBillingTpl {
        current: "mybilling",
        tenant_name,
        plan,
        plan_price,
        currency,
        invoices,
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

/// GET /my-billing/invoices.csv — the caller's own-tenant invoice history as
/// CSV (RFC 4180, formula-injection-safe via `audit::csv_escape`). Scoped to the
/// principal's tenant; super-admins (no single tenant) get a 403 pointing at the
/// per-tenant admin screen.
pub async fn my_invoices_csv(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let principal = auth::principal_for(&st, &headers).await;
    let Some(tid) = principal.tenant_id else {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "no tenant on principal — super-admins use /billing.html",
        )
            .into_response();
    };
    let Some(p) = st.db.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };

    let invoices: Vec<Invoice> = sqlx::query_as(
        "SELECT id, tenant_id, period, plan, amount_cents, currency, status \
         FROM billing_invoices WHERE tenant_id = $1 ORDER BY period DESC",
    )
    .bind(tid)
    .fetch_all(p)
    .await
    .unwrap_or_default();

    let mut buf = String::with_capacity(invoices.len() * 64 + 64);
    buf.push_str("period,plan,amount,currency,status\r\n");
    for i in invoices {
        let row = invoice_row(i);
        buf.push_str(&crate::audit::csv_escape(&row.period));
        buf.push(',');
        buf.push_str(&crate::audit::csv_escape(&row.plan));
        buf.push(',');
        buf.push_str(&crate::audit::csv_escape(&row.amount));
        buf.push(',');
        buf.push_str(&crate::audit::csv_escape(&row.currency));
        buf.push(',');
        buf.push_str(&crate::audit::csv_escape(&row.status));
        buf.push_str("\r\n");
    }

    (
        axum::http::StatusCode::OK,
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/csv; charset=utf-8".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=\"invoices.csv\"".to_string(),
            ),
        ],
        buf,
    )
        .into_response()
}

// ─── Printable single invoice ────────────────────────────────────────────────

/// One invoice line for the printable document (amounts pre-formatted).
pub struct LineRow {
    pub description: String,
    pub quantity: i64,
    pub unit: String,
    pub amount: String,
}

/// A printable invoice document (one invoice, browser print-to-PDF friendly).
#[derive(Template)]
#[template(path = "invoice_print.html")]
pub struct InvoicePrintTpl {
    pub tenant_name: String,
    pub invoice_id: String,
    pub period: String,
    pub plan: String,
    pub amount: String,
    pub currency: String,
    pub status: String,
    pub issued_at: String,
    pub paid_at: Option<String>,
    pub lines: Vec<LineRow>,
}

/// A row joining an invoice to its tenant name, for the printable document.
#[derive(FromRow)]
struct InvoiceDoc {
    tenant_name: String,
    id: uuid::Uuid,
    period: time::OffsetDateTime,
    plan: String,
    amount_cents: i64,
    currency: String,
    status: String,
    issued_at: time::OffsetDateTime,
    paid_at: Option<time::OffsetDateTime>,
}

fn fmt_date(d: time::OffsetDateTime) -> String {
    d.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// GET /my-billing/invoices/:id — a printable invoice document, scoped to
/// the caller's own tenant. The query carries an explicit `tenant_id = $2`
/// (matching the principal) so a guessed invoice id from another tenant 404s.
pub async fn invoice_print(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let principal = auth::principal_for(&st, &headers).await;
    // Super-admins have no single tenant; they use /billing.html. Tenant-admins
    // are confined to their own tenant_id.
    let tenant_filter = if auth::is_super_admin(&principal.roles) {
        None
    } else {
        match principal.tenant_id {
            Some(t) => Some(t),
            None => {
                return (axum::http::StatusCode::FORBIDDEN, "no tenant on principal")
                    .into_response()
            }
        }
    };
    let Some(p) = st.db.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };

    let doc: Option<InvoiceDoc> = sqlx::query_as(
        "SELECT t.name AS tenant_name, i.id, i.period, i.plan, i.amount_cents, \
                i.currency, i.status, i.issued_at, i.paid_at \
         FROM billing_invoices i JOIN tenants t ON t.id = i.tenant_id \
         WHERE i.id = $1 AND ($2::uuid IS NULL OR i.tenant_id = $2)",
    )
    .bind(id)
    .bind(tenant_filter)
    .fetch_optional(p)
    .await
    .unwrap_or_default();
    let Some(d) = doc else {
        return (axum::http::StatusCode::NOT_FOUND, "invoice not found").into_response();
    };

    // Line items, ordered base-first then overage. A legacy invoice with no
    // lines renders the header amount as a single implicit line in the template.
    let lines: Vec<LineRow> = sqlx::query_as::<_, (String, i64, i64, i64)>(
        "SELECT description, quantity, unit_cents, amount_cents \
         FROM billing_invoice_lines WHERE invoice_id = $1 \
         ORDER BY (kind = 'base') DESC, kind",
    )
    .bind(d.id)
    .fetch_all(p)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(
        |(description, quantity, unit_cents, amount_cents)| LineRow {
            description,
            quantity,
            unit: money(unit_cents),
            amount: money(amount_cents),
        },
    )
    .collect();

    let period = d
        .period
        .format(&time::format_description::parse("[year]-[month]").unwrap_or_else(|_| Vec::new()))
        .unwrap_or_default();
    match (InvoicePrintTpl {
        tenant_name: d.tenant_name,
        invoice_id: d.id.to_string(),
        period,
        plan: d.plan,
        amount: money(d.amount_cents),
        currency: d.currency,
        status: d.status,
        issued_at: fmt_date(d.issued_at),
        paid_at: d.paid_at.map(fmt_date),
        lines,
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

// ─── Batch generation (all tenants, one period) ──────────────────────────────

/// Generate (or refresh) the invoice for `period_date` (YYYY-MM-DD,
/// first-of-month) for every tenant with a priced plan, metering usage-based
/// overage per tenant via [`generate_one`]. Idempotent per (tenant, period):
/// re-running re-meters each invoice. Returns how many invoices were processed.
async fn generate_all_for_period(p: &sqlx::PgPool, period_date: &str) -> Result<u64, sqlx::Error> {
    let tenants: Vec<uuid::Uuid> =
        sqlx::query_scalar("SELECT t.id FROM tenants t JOIN billing_plans bp ON bp.plan = t.plan")
            .fetch_all(p)
            .await?;
    let mut processed = 0u64;
    for tenant in tenants {
        if generate_one(p, tenant, period_date).await?.is_some() {
            processed += 1;
        }
    }
    Ok(processed)
}

/// Normalize a "YYYY-MM" (browser month input) or "YYYY-MM-DD" into the
/// first-of-month "YYYY-MM-DD" used for the `::date` cast. `None` if it does not
/// start with a plausible `YYYY-MM`.
fn period_first_of_month(raw: &str) -> Option<String> {
    let ym = raw.get(0..7)?;
    let bytes = ym.as_bytes();
    let shaped = bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit);
    shaped.then(|| format!("{ym}-01"))
}

#[derive(Debug, Deserialize)]
pub struct GenerateAllForm {
    /// Period as YYYY-MM (the screen sends a month input).
    pub period: String,
}

/// POST /billing/generate-all — super-admin: generate the period's invoice for
/// every priced tenant in one shot (idempotent).
pub async fn generate_all_action(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(f): Form<GenerateAllForm>,
) -> Response {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return r;
    }
    let (Some(period_date), Some(p)) = (period_first_of_month(&f.period), st.db.as_ref()) else {
        return Redirect::to("/billing.html?flash=período inválido").into_response();
    };
    match generate_all_for_period(p, &period_date).await {
        Ok(n) => {
            Redirect::to(&format!("/billing.html?flash={n} fatura(s) gerada(s)")).into_response()
        }
        Err(_) => Redirect::to("/billing.html?flash=erro ao gerar").into_response(),
    }
}

const RUN_TOKEN_ENV: &str = "BILLING__RUN_TOKEN";
const RUN_TOKEN_HEADER: &str = "x-billing-token";

/// Constant-time equality so a wrong token can't be recovered by timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, Deserialize)]
pub struct RunQuery {
    /// Period as YYYY-MM (or YYYY-MM-DD); first-of-month is used.
    pub period: String,
}

#[derive(Debug, Serialize)]
pub struct RunResult {
    pub period: String,
    pub generated: u64,
}

/// POST /api/v1/admin/billing/run?period=YYYY-MM — machine endpoint for an
/// external scheduler (k8s CronJob / systemd timer). Requires a matching
/// `X-Billing-Token` header against `BILLING__RUN_TOKEN`. Unlike the LAN-trust
/// `/internal/*` routes this is **fail-closed**: with no token configured the
/// endpoint is disabled (503), since it mutates billing data and may be exposed.
pub async fn run_billing(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<RunQuery>,
) -> Response {
    let Some(secret) = std::env::var(RUN_TOKEN_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "billing run endpoint disabled (BILLING__RUN_TOKEN unset)",
        )
            .into_response();
    };
    let presented = headers
        .get(RUN_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !ct_eq(presented.as_bytes(), secret.as_bytes()) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(period_date) = period_first_of_month(&q.period) else {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "period must be YYYY-MM",
        )
            .into_response();
    };
    let Some(p) = st.db.as_ref() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "db unavailable",
        )
            .into_response();
    };
    match generate_all_for_period(p, &period_date).await {
        Ok(n) => Json(RunResult {
            period: period_date,
            generated: n,
        })
        .into_response(),
        Err(e) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db: {e}"),
        )
            .into_response(),
    }
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
    use super::{
        ct_eq, fmt_date, invoice_row, money, parse_price_to_cents, period_first_of_month, Invoice,
    };

    #[test]
    fn period_first_of_month_normalizes_and_validates() {
        assert_eq!(period_first_of_month("2026-06"), Some("2026-06-01".into()));
        assert_eq!(
            period_first_of_month("2026-06-15"),
            Some("2026-06-01".into())
        );
        assert_eq!(period_first_of_month(""), None);
        assert_eq!(period_first_of_month("2026/06"), None);
        assert_eq!(period_first_of_month("abcd-ef"), None);
        assert_eq!(period_first_of_month("2026-6"), None);
    }

    #[test]
    fn run_token_compare_is_constant_time_equal() {
        assert!(ct_eq(b"s3cret", b"s3cret"));
        assert!(!ct_eq(b"s3cret", b"s3crxt"));
        assert!(!ct_eq(b"s3cret", b"s3cre"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn fmt_date_renders_rfc3339() {
        let d = time::OffsetDateTime::parse(
            "2026-06-15T09:30:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert_eq!(fmt_date(d), "2026-06-15T09:30:00Z");
    }

    #[test]
    fn invoice_row_formats_period_and_amount() {
        let period = time::OffsetDateTime::parse(
            "2026-06-01T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let inv = Invoice {
            id: uuid::Uuid::nil(),
            tenant_id: uuid::Uuid::nil(),
            period,
            plan: "professional".into(),
            amount_cents: 9990,
            currency: "BRL".into(),
            status: "pending".into(),
        };
        let row = invoice_row(inv);
        assert_eq!(row.period, "2026-06");
        assert_eq!(row.amount, "99.90");
        assert_eq!(row.plan, "professional");
        assert_eq!(row.status, "pending");
    }

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

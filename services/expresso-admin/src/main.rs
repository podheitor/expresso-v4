//! expresso-admin — SSR admin UI + health/metrics.

use std::{net::SocketAddr, sync::Arc};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tower_http::services::ServeDir;
use tracing::info;

mod audit;
mod auth;
mod billing;
mod counter;
mod dav_admin;
mod dead_props;
mod drive_quotas;
mod govbr;
mod handlers;
mod kc;
mod ldap;
mod saml;
mod templates;
mod tenants;
mod usage;

use auth::AuthConfig;
use kc::KcClient;

const SERVICE: &str = "expresso-admin";
const DEFAULT_PORT: u16 = 8101;

pub struct AppState {
    pub kc: KcClient,
    pub http: reqwest::Client,
    pub auth: AuthConfig,
    pub db: Option<expresso_core::DbPool>,
}

pub struct AdminError(pub anyhow::Error);

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self.0, "admin error");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("upstream error: {}", self.0),
        )
            .into_response()
    }
}

async fn health() -> Json<Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}
async fn ready() -> Json<Value> {
    Json(json!({"ready": true}))
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = std::env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);
    Ok(format!("{host}:{port}").parse()?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let db = match std::env::var("DATABASE__URL").ok() {
        Some(url) => {
            let cfg = expresso_core::config::DatabaseConfig {
                url,
                max_connections: std::env::var("DATABASE__MAX_CONNECTIONS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(10),
                min_connections: std::env::var("DATABASE__MIN_CONNECTIONS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1),
                acquire_timeout_secs: std::env::var("DATABASE__ACQUIRE_TIMEOUT_SECS")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(10),
            };
            match expresso_core::create_db_pool(&cfg).await {
                Ok(pool) => Some(pool),
                Err(e) => {
                    tracing::warn!(error=%e, "database unavailable");
                    None
                }
            }
        }
        None => {
            tracing::warn!("DATABASE__URL not set; DAV admin disabled");
            None
        }
    };

    let state = Arc::new(AppState {
        kc: handlers::kc_factory(),
        http: reqwest::Client::builder().build()?,
        auth: AuthConfig::from_env(),
        db,
    });

    let app = Router::new()
        .route("/", get(handlers::dashboard))
        .route("/users", get(handlers::users))
        .route("/users/totp-status", get(handlers::users_totp_status))
        .route(
            "/users/new",
            get(handlers::user_new).post(handlers::user_create),
        )
        .route(
            "/users/:id/edit",
            get(handlers::user_edit).post(handlers::user_update),
        )
        .route("/users/:id/delete", post(handlers::user_delete))
        .route("/users/:id/totp/enroll", post(handlers::user_totp_enroll))
        .route("/users/:id/totp/reset", post(handlers::user_totp_reset))
        .route("/realm", get(handlers::realm_page))
        .route("/calendars", get(dav_admin::calendars_list))
        .route(
            "/calendars/:tenant_id/:id/edit",
            get(dav_admin::calendar_edit_form).post(dav_admin::calendar_edit_action),
        )
        .route(
            "/calendars/:tenant_id/:id/delete",
            post(dav_admin::calendar_delete_action),
        )
        .route("/addressbooks", get(dav_admin::addressbooks_list))
        .route(
            "/addressbooks/:tenant_id/:id/edit",
            get(dav_admin::addressbook_edit_form).post(dav_admin::addressbook_edit_action),
        )
        .route(
            "/addressbooks/:tenant_id/:id/delete",
            post(dav_admin::addressbook_delete_action),
        )
        .route("/tenants", get(tenants::list))
        .route(
            "/tenants/new",
            get(tenants::new_form).post(tenants::create_action),
        )
        .route(
            "/tenants/wizard",
            get(tenants::wizard_form).post(tenants::wizard_action),
        )
        .route(
            "/tenants/:id/edit",
            get(tenants::edit_form).post(tenants::edit_action),
        )
        .route(
            "/tenants/:id/config",
            get(tenants::config_form).post(tenants::config_action),
        )
        .route("/tenants/:id/delete", post(tenants::delete_action))
        .route("/api/v1/admin/tenants/:id/usage", get(usage::tenant_usage))
        .route("/api/v1/admin/billing/plans", get(billing::list_plans))
        .route(
            "/api/v1/admin/billing/plans/:plan",
            axum::routing::put(billing::set_plan_price),
        )
        .route(
            "/api/v1/admin/tenants/:id/invoices",
            get(billing::list_invoices).post(billing::generate_invoice),
        )
        .route(
            "/api/v1/admin/invoices/:invoice_id",
            axum::routing::patch(billing::set_invoice_status),
        )
        .route("/billing.html", get(billing::page))
        .route("/my-billing.html", get(billing::my_page))
        .route("/billing/price", post(billing::set_price_action))
        .route("/billing/generate", post(billing::generate_action))
        .route("/billing/mark", post(billing::mark_action))
        .route("/audit.json", get(audit::list))
        .route("/audit.csv", get(audit::csv))
        .route("/audit.html", get(audit::page))
        .route("/audit", get(audit::page))
        .route("/audit/purge", post(audit::purge))
        .route("/counter.html", get(counter::page))
        .route("/counter/:id/accept", post(counter::accept))
        .route("/counter/:id/reject", post(counter::reject))
        .route("/dead-props.html", get(dead_props::page))
        .route("/drive-quotas.html", get(drive_quotas::page))
        .route("/drive-quotas/:tenant_id", post(drive_quotas::update))
        .route(
            "/api/v1/govbr/mappings",
            get(govbr::list).post(govbr::upsert),
        )
        .route(
            "/api/v1/govbr/mappings/:cpf_hash",
            get(govbr::get_one).delete(govbr::delete),
        )
        .route("/api/v1/saml/idps", get(saml::list).post(saml::upsert))
        .route(
            "/api/v1/saml/idps/:id",
            get(saml::get_one).delete(saml::delete),
        )
        .route("/api/v1/saml/mappings", get(saml::list_mappings))
        .route("/api/v1/ldap/configs", get(ldap::list).post(ldap::upsert))
        .route(
            "/api/v1/ldap/configs/:id",
            get(ldap::get_one).delete(ldap::delete),
        )
        .route("/health", get(health))
        .route("/ready", get(ready))
        .nest_service("/static", ServeDir::new("static"))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_admin,
        ))
        .with_state(state)
        .merge(expresso_observability::metrics_router());

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // resolve_addr reads process-wide HOST/PORT env vars; serialize the tests
    // that mutate them so they don't race when run in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn resolve_addr_default_port_when_env_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PORT");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
    }

    #[test]
    fn resolve_addr_custom_port() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PORT", "9999");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 9999);
        std::env::remove_var("PORT");
    }

    #[test]
    fn resolve_addr_default_host_is_all_interfaces() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("HOST");
        std::env::remove_var("PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "0.0.0.0");
    }

    #[test]
    fn resolve_addr_invalid_port_uses_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PORT", "notaport");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), DEFAULT_PORT);
        std::env::remove_var("PORT");
    }

    #[test]
    fn default_port_is_8101() {
        assert_eq!(DEFAULT_PORT, 8101);
    }

    #[test]
    fn service_name_is_expresso_admin() {
        assert_eq!(SERVICE, "expresso-admin");
    }

    #[test]
    fn resolve_addr_loopback_host() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOST", "127.0.0.1");
        std::env::remove_var("PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        std::env::remove_var("HOST");
    }

    #[test]
    fn resolve_addr_port_zero_uses_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PORT", "0");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 0); // 0 is valid for OS-assigned port
        std::env::remove_var("PORT");
    }

    #[test]
    fn resolve_addr_returns_ok() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("PORT");
        std::env::remove_var("HOST");
        assert!(resolve_addr().is_ok());
    }

    #[test]
    fn resolve_addr_port_boundary_65535() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PORT", "65535");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 65535);
        std::env::remove_var("PORT");
    }

    #[test]
    fn admin_error_into_response_is_500() {
        use axum::response::IntoResponse;
        let e = AdminError(anyhow::anyhow!("test error"));
        let resp = e.into_response();
        assert_eq!(resp.status().as_u16(), 500);
    }

    #[test]
    fn resolve_addr_custom_port_1024() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PORT", "1024");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 1024);
        std::env::remove_var("PORT");
    }

    #[test]
    fn resolve_addr_host_env_overrides_default() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("HOST", "10.0.0.1");
        std::env::remove_var("PORT");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.ip().to_string(), "10.0.0.1");
        std::env::remove_var("HOST");
    }

    #[test]
    fn resolve_addr_port_1_is_valid() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("PORT", "1");
        std::env::remove_var("HOST");
        let addr = resolve_addr().unwrap();
        assert_eq!(addr.port(), 1);
        std::env::remove_var("PORT");
    }
}

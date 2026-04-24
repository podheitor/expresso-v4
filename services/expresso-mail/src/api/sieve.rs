//! Inbox rules — per-user Sieve filter script.
//!
//! GET/PUT /api/v1/mail/sieve — returns/upserts raw Sieve source + enabled flag.
//! PUT validates the script by compiling with `sieve::Compiler`; rejects with
//! 400 if compilation fails so users can't break their own delivery pipeline.

use axum::{
    extract::State,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/mail/sieve", get(get_sieve).put(put_sieve))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SieveRules {
    pub enabled: bool,
    pub script:  String,
}

impl Default for SieveRules {
    fn default() -> Self {
        Self { enabled: true, script: String::new() }
    }
}

async fn get_sieve(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<SieveRules>> {
    let row = sqlx::query(
        "SELECT enabled, script
         FROM user_sieve WHERE user_id = $1 AND tenant_id = $2"
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_optional(state.db()).await?;

    let rules = match row {
        Some(r) => SieveRules { enabled: r.get("enabled"), script: r.get("script") },
        None    => SieveRules::default(),
    };
    Ok(Json(rules))
}

async fn put_sieve(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(rules): Json<SieveRules>,
) -> Result<Json<SieveRules>> {
    if !rules.script.is_empty() {
        let compiler = sieve::Compiler::new();
        if let Err(e) = compiler.compile(rules.script.as_bytes()) {
            return Err(MailError::BadRequest(format!("sieve compile error: {e}")));
        }
    }

    sqlx::query(
        "INSERT INTO user_sieve (user_id, tenant_id, enabled, script, updated_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (user_id) DO UPDATE SET
            enabled    = EXCLUDED.enabled,
            script     = EXCLUDED.script,
            updated_at = now()"
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(rules.enabled)
    .bind(&rules.script)
    .execute(state.db()).await?;

    Ok(Json(rules))
}

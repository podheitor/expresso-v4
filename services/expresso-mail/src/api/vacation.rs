//! Out-of-office (RFC 5230 Sieve `vacation` extension).
//!
//! GET/PUT /api/v1/mail/vacation — per-user config.
//! On PUT, server re-renders `sieve_script` from structured fields so
//! local-delivery/ingest can execute the rule without parsing the form.

use axum::{
    extract::State,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/mail/vacation", get(get_vacation).put(put_vacation))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Vacation {
    pub enabled:        bool,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub starts_at:      Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option", default)]
    pub ends_at:        Option<OffsetDateTime>,
    pub subject:        String,
    pub body:           String,
    pub interval_days:  i32,
    #[serde(default)]
    pub sieve_script:   String,
}

impl Default for Vacation {
    fn default() -> Self {
        Self {
            enabled:       false,
            starts_at:     None,
            ends_at:       None,
            subject:       "Out of office".into(),
            body:          String::new(),
            interval_days: 7,
            sieve_script:  String::new(),
        }
    }
}

/// Render a Sieve script from the vacation settings.
/// Disabled → empty script. Encodes quotes + backslashes in payloads.
pub fn render_script(v: &Vacation) -> String {
    if !v.enabled {
        return String::new();
    }
    let subj = escape(&v.subject);
    let body = escape(&v.body);
    let days = v.interval_days.clamp(1, 365);
    let mut s = String::new();
    s.push_str("require [\"vacation\"];\n");
    s.push_str(&format!(
        "vacation :days {days} :subject \"{subj}\" \"{body}\";\n"
    ));
    s
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

async fn get_vacation(
    State(state): State<AppState>,
    ctx: RequestCtx,
) -> Result<Json<Vacation>> {
    let pool = state.db();
    let row  = sqlx::query(
        "SELECT enabled, starts_at, ends_at, subject, body, interval_days, sieve_script
         FROM user_vacation WHERE user_id = $1 AND tenant_id = $2"
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool).await?;
    let v = match row {
        Some(r) => Vacation {
            enabled:       r.get("enabled"),
            starts_at:     r.try_get("starts_at").ok(),
            ends_at:       r.try_get("ends_at").ok(),
            subject:       r.get("subject"),
            body:          r.get("body"),
            interval_days: r.get("interval_days"),
            sieve_script:  r.get("sieve_script"),
        },
        None => Vacation::default(),
    };
    Ok(Json(v))
}

async fn put_vacation(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(mut v): Json<Vacation>,
) -> Result<Json<Vacation>> {
    if v.interval_days < 1 || v.interval_days > 365 {
        return Err(MailError::BadRequest("interval_days out of range".into()));
    }
    v.sieve_script = render_script(&v);

    let pool = state.db();
    sqlx::query(
        "INSERT INTO user_vacation
            (user_id, tenant_id, enabled, starts_at, ends_at, subject, body, interval_days, sieve_script, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
         ON CONFLICT (user_id) DO UPDATE SET
            enabled       = EXCLUDED.enabled,
            starts_at     = EXCLUDED.starts_at,
            ends_at       = EXCLUDED.ends_at,
            subject       = EXCLUDED.subject,
            body          = EXCLUDED.body,
            interval_days = EXCLUDED.interval_days,
            sieve_script  = EXCLUDED.sieve_script,
            updated_at    = now()"
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .bind(v.enabled)
    .bind(v.starts_at)
    .bind(v.ends_at)
    .bind(&v.subject)
    .bind(&v.body)
    .bind(v.interval_days)
    .bind(&v.sieve_script)
    .execute(pool).await?;

    Ok(Json(v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_renders_empty() {
        let v = Vacation { enabled: false, ..Vacation::default() };
        assert_eq!(render_script(&v), "");
    }

    #[test]
    fn enabled_renders_sieve_with_escape() {
        let v = Vacation {
            enabled: true,
            subject: r#"Off "now""#.into(),
            body: "See you\\next week".into(),
            interval_days: 3,
            ..Vacation::default()
        };
        let s = render_script(&v);
        assert!(s.contains("require [\"vacation\"]"));
        assert!(s.contains(":days 3"));
        assert!(s.contains(r#":subject "Off \"now\"""#));
        assert!(s.contains(r#""See you\\next week""#));
    }

    #[test]
    fn clamps_interval_days() {
        let v = Vacation { enabled: true, interval_days: 999, ..Vacation::default() };
        assert!(render_script(&v).contains(":days 365"));
    }
}

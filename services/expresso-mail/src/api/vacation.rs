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
use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use time::OffsetDateTime;

use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

/// Limites duros pros campos do auto-reply.
///
/// `subject` casa com RFC 5322 §2.1.1 (998 chars/line) e protege o
/// header `Subject:` quando o auto-reply for emitido. CR/LF no subject
/// são bloqueados na validação — header smuggling clássico (atacante
/// injeta `Bcc:` ou body alheio via field do form).
///
/// `body` 8 KiB cobre uma mensagem de OOO razoável; acima é abuso —
/// cada delivery copia o body inteiro pro reply outbound, então sem
/// limite vira amplificador de bandwidth via mailing-lists/spam.
pub const MAX_VACATION_SUBJECT_BYTES: usize = 998;
pub const MAX_VACATION_BODY_BYTES:    usize = 8 * 1024;

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
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row  = sqlx::query(
        "SELECT enabled, starts_at, ends_at, subject, body, interval_days, sieve_script
         FROM user_vacation WHERE user_id = $1 AND tenant_id = $2"
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx).await?;
    tx.commit().await?;
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
    validate(&v)?;
    v.sieve_script = render_script(&v);

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
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
    .execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(v))
}

/// Gate aplicado em PUT /mail/vacation. Ordem: tamanho → CR/LF →
/// interval. Tamanho primeiro pra rejeitar abuso antes de tocar
/// memória/regex desnecessários.
fn validate(v: &Vacation) -> Result<()> {
    if v.subject.len() > MAX_VACATION_SUBJECT_BYTES {
        return Err(MailError::BadRequest(format!(
            "subject too large: {} bytes (max {})",
            v.subject.len(), MAX_VACATION_SUBJECT_BYTES
        )));
    }
    if v.body.len() > MAX_VACATION_BODY_BYTES {
        return Err(MailError::BadRequest(format!(
            "body too large: {} bytes (max {})",
            v.body.len(), MAX_VACATION_BODY_BYTES
        )));
    }
    if v.subject.contains('\r') || v.subject.contains('\n') {
        return Err(MailError::BadRequest(
            "subject must not contain CR or LF".into()
        ));
    }
    if v.interval_days < 1 || v.interval_days > 365 {
        return Err(MailError::BadRequest("interval_days out of range".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_vacation() -> Vacation {
        Vacation { enabled: true, ..Vacation::default() }
    }

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

    #[test]
    fn validate_default_ok() {
        assert!(validate(&ok_vacation()).is_ok());
    }

    #[test]
    fn validate_rejects_oversize_subject() {
        let v = Vacation {
            subject: "x".repeat(MAX_VACATION_SUBJECT_BYTES + 1),
            ..ok_vacation()
        };
        let err = format!("{:?}", validate(&v).unwrap_err());
        assert!(err.contains("subject too large"), "got: {err}");
    }

    #[test]
    fn validate_rejects_oversize_body() {
        let v = Vacation {
            body: "x".repeat(MAX_VACATION_BODY_BYTES + 1),
            ..ok_vacation()
        };
        let err = format!("{:?}", validate(&v).unwrap_err());
        assert!(err.contains("body too large"), "got: {err}");
    }

    #[test]
    fn validate_rejects_crlf_in_subject() {
        // Header smuggling clássico — subject vai pro header `Subject:`
        // do auto-reply emitido.
        let v = Vacation {
            subject: "Out\r\nBcc: attacker@evil.com".into(),
            ..ok_vacation()
        };
        let err = format!("{:?}", validate(&v).unwrap_err());
        assert!(err.contains("CR or LF"), "got: {err}");

        let v = Vacation { subject: "line1\nline2".into(), ..ok_vacation() };
        assert!(validate(&v).is_err());
    }

    #[test]
    fn validate_rejects_bad_interval() {
        let v = Vacation { interval_days: 0,    ..ok_vacation() };
        assert!(validate(&v).is_err());
        let v = Vacation { interval_days: 1000, ..ok_vacation() };
        assert!(validate(&v).is_err());
    }

    #[test]
    fn validate_accepts_boundary_subject_and_body() {
        let v = Vacation {
            subject: "x".repeat(MAX_VACATION_SUBJECT_BYTES),
            body:    "y".repeat(MAX_VACATION_BODY_BYTES),
            ..ok_vacation()
        };
        assert!(validate(&v).is_ok());
    }
}

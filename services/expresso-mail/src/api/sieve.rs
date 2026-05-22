//! Inbox rules — per-user Sieve filter script.
//!
//! GET/PUT /api/v1/mail/sieve — returns/upserts raw Sieve source + enabled flag.
//! PUT validates the script by compiling with `sieve::Compiler`; rejects with
//! 400 if compilation fails so users can't break their own delivery pipeline.

use axum::{
    extract::State,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
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

/// Limite duro do tamanho do script Sieve por usuário.
///
/// Filtros reais raramente passam de poucos KiB. 64 KiB cobre power-users
/// com dezenas de regras + comentários; acima disso é quase certamente
/// abuso (DoS via parser/runtime do `sieve-rs` em cada delivery, ou enchendo
/// a tabela `user_sieve`). Reject early — antes de compilar — pra evitar
/// gastar CPU compilando scripts gigantes.
pub const MAX_SIEVE_SCRIPT_BYTES: usize = 64 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/sieve", get(get_sieve).put(put_sieve))
        .route("/mail/sieve/test", axum::routing::post(test_sieve))
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
    ctx:          RequestCtx,
    req_headers:  HeaderMap,
) -> Result<Response> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(
        "SELECT enabled, script, updated_at
         FROM user_sieve WHERE user_id = $1 AND tenant_id = $2"
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx).await?;
    tx.commit().await?;

    let (rules, updated_at) = match row {
        Some(r) => {
            let ua: Option<OffsetDateTime> = r.try_get("updated_at").ok();
            (SieveRules { enabled: r.get("enabled"), script: r.get("script") }, ua)
        },
        None => (SieveRules::default(), None),
    };
    if let Some(ts) = updated_at {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
        let mut resp = Json(rules).into_response();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
        return Ok(resp);
    }
    Ok(Json(rules).into_response())
}

async fn put_sieve(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Json(rules): Json<SieveRules>,
) -> Result<Json<SieveRules>> {
    validate_script(&rules.script)?;

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
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
    .execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(rules))
}

/// Max raw message size for sieve test endpoint. Prevents DoS via huge bodies
/// through the sieve runtime. 1 MiB covers realistic test messages.
pub const MAX_TEST_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct SieveTestRequest {
    /// Sieve script to test (does not need to be the saved script).
    pub script: String,
    /// Raw RFC 2822 message to evaluate against the script.
    pub raw_message: String,
}

#[derive(Debug, Serialize)]
pub struct SieveTestAction {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SieveTestResponse {
    pub actions: Vec<SieveTestAction>,
}

/// POST /api/v1/mail/sieve/test — avalia o script Sieve fornecido contra a
/// mensagem RFC 2822 fornecida e retorna as ações resultantes sem persistir
/// nada. Permite que a UI valide regras antes de PUT. O script não precisa
/// ser o script salvo do usuário — qualquer script válido funciona.
async fn test_sieve(
    _ctx: RequestCtx,
    Json(req): Json<SieveTestRequest>,
) -> Result<Json<SieveTestResponse>> {
    if req.raw_message.len() > MAX_TEST_MESSAGE_BYTES {
        return Err(MailError::BadRequest(format!(
            "raw_message too large: {} bytes (max {})",
            req.raw_message.len(), MAX_TEST_MESSAGE_BYTES
        )));
    }
    // Validate + compile script first (size + syntax). Returns 400 on error.
    validate_script(&req.script)?;

    let actions_raw = crate::sieve::evaluate(req.script.as_bytes(), req.raw_message.as_bytes());

    let actions: Vec<SieveTestAction> = actions_raw.into_iter().map(|a| match a {
        crate::sieve::FilterAction::Keep { flags } =>
            SieveTestAction { action: "keep".into(), folder: None, reason: None, address: None, flags },
        crate::sieve::FilterAction::FileInto { folder, flags } =>
            SieveTestAction { action: "fileinto".into(), folder: Some(folder), reason: None, address: None, flags },
        crate::sieve::FilterAction::Reject { reason } =>
            SieveTestAction { action: "reject".into(), folder: None, reason: Some(reason), address: None, flags: vec![] },
        crate::sieve::FilterAction::Discard =>
            SieveTestAction { action: "discard".into(), folder: None, reason: None, address: None, flags: vec![] },
        crate::sieve::FilterAction::Redirect { address } =>
            SieveTestAction { action: "redirect".into(), folder: None, reason: None, address: Some(address), flags: vec![] },
    }).collect();

    Ok(Json(SieveTestResponse { actions }))
}

/// Valida script antes de gravar. Ordem importa: tamanho primeiro pra
/// não pagar `Compiler::compile` em scripts gigantes (vetor de DoS).
fn validate_script(script: &str) -> Result<()> {
    if script.len() > MAX_SIEVE_SCRIPT_BYTES {
        return Err(MailError::BadRequest(format!(
            "sieve script too large: {} bytes (max {})",
            script.len(), MAX_SIEVE_SCRIPT_BYTES
        )));
    }
    if !script.is_empty() {
        let compiler = sieve::Compiler::new();
        if let Err(e) = compiler.compile(script.as_bytes()) {
            return Err(MailError::BadRequest(format!("sieve compile error: {e}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_script_accepted() {
        assert!(validate_script("").is_ok());
    }

    #[test]
    fn small_valid_script_accepted() {
        let s = r#"require ["fileinto"];
if header :contains "Subject" "[spam]" {
    fileinto "Junk";
}"#;
        assert!(validate_script(s).is_ok());
    }

    #[test]
    fn invalid_syntax_rejected() {
        let bad = "this is not valid sieve at all }}}}";
        let err = validate_script(bad).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("compile") || msg.contains("sieve"),
            "expected compile error, got: {msg}");
    }

    #[test]
    fn oversize_script_rejected_before_compile() {
        // Conteúdo válido (comentário) repetido até estourar o limite.
        // Mesmo sendo sintaxe válida, deve rejeitar pelo tamanho.
        let chunk = "# pad pad pad pad pad pad pad pad pad pad pad pad pad pad\n";
        let mut s = String::with_capacity(MAX_SIEVE_SCRIPT_BYTES + 1024);
        while s.len() <= MAX_SIEVE_SCRIPT_BYTES {
            s.push_str(chunk);
        }
        assert!(s.len() > MAX_SIEVE_SCRIPT_BYTES);
        let err = validate_script(&s).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("too large") || msg.contains("max"),
            "expected size-limit error, got: {msg}");
    }

    #[test]
    fn boundary_script_accepted() {
        // Exatamente no limite (tudo comentário — válido pro compiler).
        let header = "# ";
        let mut s = String::with_capacity(MAX_SIEVE_SCRIPT_BYTES);
        s.push_str(header);
        while s.len() < MAX_SIEVE_SCRIPT_BYTES {
            s.push('x');
        }
        assert_eq!(s.len(), MAX_SIEVE_SCRIPT_BYTES);
        assert!(validate_script(&s).is_ok());
    }

    #[test]
    fn sieve_rules_default_enabled_true() {
        let r = SieveRules::default();
        assert!(r.enabled);
        assert!(r.script.is_empty());
    }

    #[test]
    fn sieve_test_request_deser() {
        let json = r#"{"script":"keep;","raw_message":"From: a@ex.com\r\n\r\nbody"}"#;
        let r: SieveTestRequest = serde_json::from_str(json).unwrap();
        assert_eq!(r.script, "keep;");
        assert!(!r.raw_message.is_empty());
    }

    #[test]
    fn sieve_rules_disabled_script_preserved() {
        let r = SieveRules { enabled: false, script: "fileinto \"INBOX\";".into() };
        assert!(!r.enabled);
        assert!(r.script.contains("fileinto"));
    }

    #[test]
    fn sieve_rules_enabled_serializes_true() {
        let r = SieveRules { enabled: true, script: "keep;".into() };
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["enabled"], true);
    }

    #[test]
    fn sieve_rules_script_preserved() {
        let r = SieveRules { enabled: false, script: "discard;".into() };
        assert_eq!(r.script, "discard;");
    }

    #[test]
    fn sieve_rules_enabled_flag_preserved() {
        let r = SieveRules { enabled: true, script: String::new() };
        assert!(r.enabled);
    }

    #[test]
    fn sieve_rules_disabled_flag_preserved() {
        let r = SieveRules { enabled: false, script: "keep;".into() };
        assert!(!r.enabled);
    }

    #[test]
    fn sieve_rules_empty_script_is_valid() {
        let r = SieveRules { enabled: false, script: String::new() };
        assert!(r.script.is_empty());
    }

    #[test]
    fn sieve_rules_enabled_field_preserved() {
        let r = SieveRules { enabled: true, script: "keep;".into() };
        assert!(r.enabled);
    }

    #[test]
    fn sieve_test_request_raw_message_preserved() {
        let json = r#"{"script":"keep;","raw_message":"From: x@y.com\r\n\r\nbody"}"#;
        let r: SieveTestRequest = serde_json::from_str(json).unwrap();
        assert!(r.raw_message.contains("From:"));
    }
}

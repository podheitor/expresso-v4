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
use expresso_core::begin_tenant_tx;
use serde::{Deserialize, Serialize};
use sqlx::Row;

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
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(
        "SELECT enabled, script
         FROM user_sieve WHERE user_id = $1 AND tenant_id = $2"
    )
    .bind(ctx.user_id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx).await?;
    tx.commit().await?;

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
}

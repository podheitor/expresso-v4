//! Email signatures — per-user HTML/plain signatures.
//!
//! GET    /api/v1/mail/signatures          — list signatures
//! POST   /api/v1/mail/signatures          — create signature
//! GET    /api/v1/mail/signatures/:id      — fetch single signature
//! PUT    /api/v1/mail/signatures/:id      — update signature
//! DELETE /api/v1/mail/signatures/:id      — delete signature
//!
//! Outlook semantics: a user has at most one *default* signature. Promoting
//! one to default demotes the previous default in the same transaction, so the
//! `is_default` invariant (≤ 1 per user) holds without a partial-unique index
//! racing against concurrent writers.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use expresso_core::begin_tenant_tx;
use crate::api::context::RequestCtx;
use crate::error::{MailError, Result};
use crate::state::AppState;

pub const MAX_SIGNATURE_BYTES: usize = 32 * 1024;
pub const MAX_SIGNATURE_NAME_BYTES: usize = 128;
pub const MAX_SIGNATURES_PER_USER: usize = 10;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/mail/signatures",     get(list_signatures).post(create_signature))
        .route("/mail/signatures/:id", get(get_signature).put(update_signature).delete(delete_signature))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SignatureFormat {
    Html,
    Plain,
}

impl SignatureFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Html  => "html",
            Self::Plain => "plain",
        }
    }

    /// Parse the DB `format` column. Unknown values fall back to `Plain` —
    /// a corrupt row should render as inert text, never as trusted HTML.
    fn from_db(s: &str) -> Self {
        match s {
            "html" => Self::Html,
            _      => Self::Plain,
        }
    }
}

impl std::fmt::Display for SignatureFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub id:         Uuid,
    pub user_id:    Uuid,
    pub tenant_id:  Uuid,
    pub name:       String,
    pub content:    String,
    pub format:     SignatureFormat,
    pub is_default: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateSignatureBody {
    pub name:       String,
    pub content:    String,
    pub format:     Option<SignatureFormat>,
    pub is_default: Option<bool>,
}

fn row_to_signature(r: &sqlx::postgres::PgRow) -> Signature {
    let fmt: String = r.get("format");
    Signature {
        id:         r.get("id"),
        user_id:    r.get("user_id"),
        tenant_id:  r.get("tenant_id"),
        name:       r.get("name"),
        content:    r.get("content"),
        format:     SignatureFormat::from_db(&fmt),
        is_default: r.get("is_default"),
    }
}

const SELECT_COLS: &str =
    "id, user_id, tenant_id, name, content, format, is_default";

async fn list_signatures(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<Json<Vec<Signature>>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let rows = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM user_signatures \
         WHERE tenant_id = $1 AND user_id = $2 \
         ORDER BY is_default DESC, name ASC"
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows.iter().map(row_to_signature).collect()))
}

async fn get_signature(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<Json<Signature>> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM user_signatures \
         WHERE id = $1 AND tenant_id = $2 AND user_id = $3"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    row.as_ref().map(row_to_signature).map(Json).ok_or(MailError::NotFound)
}

async fn create_signature(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Json(body):   Json<CreateSignatureBody>,
) -> Result<(StatusCode, Json<Signature>)> {
    validate(&body.name, &body.content)?;
    let format = body.format.unwrap_or(SignatureFormat::Html);
    let make_default = body.is_default.unwrap_or(false);

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    let count: i64 = sqlx::query(
        "SELECT count(*) AS n FROM user_signatures WHERE tenant_id = $1 AND user_id = $2"
    )
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?
    .get("n");
    if count as usize >= MAX_SIGNATURES_PER_USER {
        return Err(MailError::BadRequest(format!(
            "at most {MAX_SIGNATURES_PER_USER} signatures per user"
        )));
    }

    if make_default {
        clear_default(&mut tx, ctx.tenant_id, ctx.user_id).await?;
    }

    let row = sqlx::query(&format!(
        "INSERT INTO user_signatures (tenant_id, user_id, name, content, format, is_default) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING {SELECT_COLS}"
    ))
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&body.name)
    .bind(&body.content)
    .bind(format.as_str())
    .bind(make_default)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(row_to_signature(&row))))
}

async fn update_signature(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<CreateSignatureBody>,
) -> Result<Json<Signature>> {
    validate(&body.name, &body.content)?;
    let format = body.format.unwrap_or(SignatureFormat::Html);
    let make_default = body.is_default.unwrap_or(false);

    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;

    if make_default {
        clear_default(&mut tx, ctx.tenant_id, ctx.user_id).await?;
    }

    let row = sqlx::query(&format!(
        "UPDATE user_signatures \
            SET name = $4, content = $5, format = $6, is_default = $7 \
          WHERE id = $1 AND tenant_id = $2 AND user_id = $3 \
          RETURNING {SELECT_COLS}"
    ))
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .bind(&body.name)
    .bind(&body.content)
    .bind(format.as_str())
    .bind(make_default)
    .fetch_optional(&mut *tx)
    .await?;

    let row = row.ok_or(MailError::NotFound)?;
    tx.commit().await?;
    Ok(Json(row_to_signature(&row)))
}

async fn delete_signature(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<StatusCode> {
    let mut tx = begin_tenant_tx(state.db(), ctx.tenant_id).await?;
    let r = sqlx::query(
        "DELETE FROM user_signatures WHERE id = $1 AND tenant_id = $2 AND user_id = $3"
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    if r.rows_affected() == 0 {
        return Err(MailError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Demote the user's current default, if any. Called inside the write tx
/// before promoting a new default so at most one row stays `is_default`.
async fn clear_default(
    tx:        &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: Uuid,
    user_id:   Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE user_signatures SET is_default = false \
         WHERE tenant_id = $1 AND user_id = $2 AND is_default = true"
    )
    .bind(tenant_id)
    .bind(user_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Reject empty/oversize name and oversize content. Mirrors the byte caps the
/// module already declared; content may legitimately contain CR/LF (HTML body),
/// so unlike header fields it is not newline-restricted.
fn validate(name: &str, content: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(MailError::BadRequest("name must not be empty".into()));
    }
    if name.len() > MAX_SIGNATURE_NAME_BYTES {
        return Err(MailError::BadRequest(format!(
            "name too large: {} bytes (max {MAX_SIGNATURE_NAME_BYTES})",
            name.len()
        )));
    }
    if content.len() > MAX_SIGNATURE_BYTES {
        return Err(MailError::BadRequest(format!(
            "content too large: {} bytes (max {MAX_SIGNATURE_BYTES})",
            content.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_signature_bytes_constant() {
        assert_eq!(MAX_SIGNATURE_BYTES, 32 * 1024);
    }

    #[test]
    fn max_signature_name_bytes_constant() {
        assert_eq!(MAX_SIGNATURE_NAME_BYTES, 128);
    }

    #[test]
    fn max_signatures_per_user_constant() {
        assert_eq!(MAX_SIGNATURES_PER_USER, 10);
    }

    #[test]
    fn validate_accepts_normal_signature() {
        assert!(validate("Work", "<p>Best regards</p>").is_ok());
    }

    #[test]
    fn validate_rejects_empty_name() {
        assert!(validate("", "content").is_err());
    }

    #[test]
    fn validate_rejects_whitespace_only_name() {
        assert!(validate("   ", "content").is_err());
    }

    #[test]
    fn validate_rejects_oversize_name() {
        let name = "x".repeat(MAX_SIGNATURE_NAME_BYTES + 1);
        assert!(validate(&name, "c").is_err());
    }

    #[test]
    fn validate_accepts_name_at_byte_cap() {
        let name = "x".repeat(MAX_SIGNATURE_NAME_BYTES);
        assert!(validate(&name, "c").is_ok());
    }

    #[test]
    fn validate_rejects_oversize_content() {
        let content = "x".repeat(MAX_SIGNATURE_BYTES + 1);
        assert!(validate("Name", &content).is_err());
    }

    #[test]
    fn validate_accepts_content_at_byte_cap() {
        let content = "x".repeat(MAX_SIGNATURE_BYTES);
        assert!(validate("Name", &content).is_ok());
    }

    #[test]
    fn validate_allows_crlf_in_html_content() {
        // Unlike header fields, signature content is a body — newlines are fine.
        assert!(validate("Name", "line1\r\nline2").is_ok());
    }

    #[test]
    fn from_db_maps_html() {
        assert_eq!(SignatureFormat::from_db("html"), SignatureFormat::Html);
    }

    #[test]
    fn from_db_maps_plain() {
        assert_eq!(SignatureFormat::from_db("plain"), SignatureFormat::Plain);
    }

    #[test]
    fn from_db_unknown_falls_back_to_plain() {
        // A corrupt row must never render as trusted HTML.
        assert_eq!(SignatureFormat::from_db("garbage"), SignatureFormat::Plain);
    }

    #[test]
    fn format_html_as_str() {
        assert_eq!(SignatureFormat::Html.as_str(), "html");
    }

    #[test]
    fn format_plain_as_str() {
        assert_eq!(SignatureFormat::Plain.as_str(), "plain");
    }

    #[test]
    fn format_html_display() {
        assert_eq!(format!("{}", SignatureFormat::Html), "html");
    }

    #[test]
    fn format_plain_display() {
        assert_eq!(format!("{}", SignatureFormat::Plain), "plain");
    }

    #[test]
    fn format_equality() {
        assert_eq!(SignatureFormat::Html, SignatureFormat::Html);
        assert_ne!(SignatureFormat::Html, SignatureFormat::Plain);
    }

    #[test]
    fn format_serde_roundtrip_html() {
        let s = serde_json::to_string(&SignatureFormat::Html).unwrap();
        let back: SignatureFormat = serde_json::from_str(&s).unwrap();
        assert_eq!(back, SignatureFormat::Html);
    }

    #[test]
    fn format_serde_roundtrip_plain() {
        let s = serde_json::to_string(&SignatureFormat::Plain).unwrap();
        let back: SignatureFormat = serde_json::from_str(&s).unwrap();
        assert_eq!(back, SignatureFormat::Plain);
    }

    #[test]
    fn signature_serde_roundtrip() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "Work".into(), content: "<p>Best</p>".into(),
            format: SignatureFormat::Html, is_default: true,
        };
        let s = serde_json::to_string(&sig).unwrap();
        let back: Signature = serde_json::from_str(&s).unwrap();
        assert_eq!(back.name, "Work");
        assert!(back.is_default);
    }

    #[test]
    fn create_signature_body_format_optional() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"Sig","content":"Text"}"#).unwrap();
        assert!(b.format.is_none());
    }

    #[test]
    fn create_signature_body_is_default_optional() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"Sig","content":"Text"}"#).unwrap();
        assert!(b.is_default.is_none());
    }

    #[test]
    fn create_signature_body_with_format_html() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"S","content":"C","format":"html"}"#).unwrap();
        assert_eq!(b.format, Some(SignatureFormat::Html));
    }

    #[test]
    fn signature_clone_preserves_name() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "Clone me".into(), content: "...".into(),
            format: SignatureFormat::Plain, is_default: false,
        };
        assert_eq!(sig.clone().name, "Clone me");
    }

    #[test]
    fn signature_is_default_false_field() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Html, is_default: false,
        };
        assert!(!sig.is_default);
    }

    #[test]
    fn max_signatures_per_user_greater_than_zero() {
        assert!(MAX_SIGNATURES_PER_USER > 0);
    }

    #[test]
    fn max_signature_bytes_is_32_kib() {
        assert_eq!(MAX_SIGNATURE_BYTES, 32768);
    }

    #[test]
    fn signature_serde_json_contains_format_key() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Html, is_default: false,
        };
        let s = serde_json::to_string(&sig).unwrap();
        assert!(s.contains("format"));
    }

    #[test]
    fn signature_serde_json_contains_is_default_key() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Plain, is_default: true,
        };
        let s = serde_json::to_string(&sig).unwrap();
        assert!(s.contains("is_default"));
    }

    #[test]
    fn signature_user_id_accessible() {
        let uid = Uuid::new_v4();
        let sig = Signature {
            id: Uuid::nil(), user_id: uid, tenant_id: Uuid::nil(),
            name: "S".into(), content: "C".into(),
            format: SignatureFormat::Html, is_default: false,
        };
        assert_eq!(sig.user_id, uid);
    }

    #[test]
    fn create_body_is_default_true() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"S","content":"C","is_default":true}"#).unwrap();
        assert_eq!(b.is_default, Some(true));
    }

    #[test]
    fn format_clone_preserves_variant() {
        let f = SignatureFormat::Html;
        assert_eq!(f.clone(), SignatureFormat::Html);
    }

    #[test]
    fn create_body_name_preserved() {
        let b: CreateSignatureBody =
            serde_json::from_str(r#"{"name":"My Sig","content":"X"}"#).unwrap();
        assert_eq!(b.name, "My Sig");
    }

    #[test]
    fn max_signature_name_bytes_positive() {
        assert!(MAX_SIGNATURE_NAME_BYTES > 0);
    }

    #[test]
    fn signature_serde_json_contains_content_key() {
        let sig = Signature {
            id: Uuid::nil(), user_id: Uuid::nil(), tenant_id: Uuid::nil(),
            name: "S".into(), content: "my content".into(),
            format: SignatureFormat::Plain, is_default: false,
        };
        let s = serde_json::to_string(&sig).unwrap();
        assert!(s.contains("content"));
    }
}

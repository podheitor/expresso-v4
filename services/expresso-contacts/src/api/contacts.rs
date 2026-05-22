//! Contact REST endpoints — path-addressed by (addressbook_id, id).
//! Create/Update accept raw vCard (`text/vcard`) payload.

use axum::{
    body::Body,
    extract::{Multipart, Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::api::context::RequestCtx;
use crate::domain::{Contact, ContactRepo};
use crate::events::ContactsEvent;
use crate::error::{ContactsError, Result};
use crate::state::AppState;

/// Cap pra vCard individual (create/update). 64 KiB cobre cartões com
/// PHOTO base64 embutido (típico ~50 KiB). Acima disso é abuso —
/// engasga storage, parser, e cada export.vcf concat downstream.
pub const MAX_CONTACT_VCARD_BYTES: usize = 64 * 1024;

/// Cap pra import em batch (multi-VCARD). Mais largo pra cobrir
/// migração real de Outlook/Google Contacts (milhares de cartões
/// num único upload).
pub const MAX_IMPORT_VCF_BYTES: usize = 4 * 1024 * 1024;

/// Gate: require OWNER/WRITE/ADMIN on the addressbook.
async fn assert_can_write(
    pool: &expresso_core::DbPool,
    tenant_id: uuid::Uuid,
    book_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<()> {
    let repo = crate::domain::AddressbookRepo::new(pool);
    let lvl = repo.access_level(tenant_id, book_id, user_id).await?;
    match lvl.as_deref() {
        Some("OWNER") | Some("WRITE") | Some("ADMIN") => Ok(()),
        Some(_) => Err(crate::error::ContactsError::Forbidden),
        None    => Err(crate::error::ContactsError::AddressbookNotFound(book_id.to_string())),
    }
}


pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/addressbooks/:book_id/contacts",
            post(create).get(list),
        )
        .route(
            "/api/v1/addressbooks/:book_id/contacts/:id",
            get(get_one).put(update).delete(delete),
        )
        .route(
            "/api/v1/addressbooks/:book_id/export.vcf",
            get(export_vcf),
        )
        .route(
            "/api/v1/addressbooks/:book_id/import",
            post(import_vcf),
        )
        .route(
            "/api/v1/contacts/import",
            post(import_csv),
        )
}

async fn create(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    validate_vcard(&raw, MAX_CONTACT_VCARD_BYTES)?;
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, book_id, ctx.user_id).await?;
    let c    = ContactRepo::new(pool).create(ctx.tenant_id, book_id, &raw).await?;
    state.bus().publish(ContactsEvent::ContactUpserted {
        tenant_id: ctx.tenant_id, addressbook_id: book_id, contact_id: c.id,
    });
    let loc  = format!("/api/v1/addressbooks/{}/contacts/{}", book_id, c.id);
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header(header::LOCATION, loc)
        .header(header::ETAG, format!("\"{}\"", c.etag))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&c).unwrap()))
        .unwrap())
}

async fn list(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
    req_headers: HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let (total, max_updated): (i64, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT COUNT(*), MAX(updated_at) FROM contacts WHERE tenant_id = $1 AND addressbook_id = $2",
    )
    .bind(ctx.tenant_id)
    .bind(book_id)
    .fetch_one(pool)
    .await?;
    if let Some(ts) = max_updated {
        if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let cs = ContactRepo::new(pool).list(ctx.tenant_id, book_id).await?;
    let mut resp = (
        [(header::HeaderName::from_static("x-total-count"), total.to_string())],
        Json(cs),
    ).into_response();
    if let Some(ts) = max_updated {
        let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
        resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

async fn get_one(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((_book_id, id)): Path<(Uuid, Uuid)>,
    req_headers: HeaderMap,
) -> Result<Response> {
    let pool = state.db_or_unavailable()?;
    let c    = ContactRepo::new(pool).get(ctx.tenant_id, id).await?;
    let etag = format!("\"{}\"", c.etag);
    if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    let lm = c.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if c.updated_at <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::ETAG, &etag)
        .header(header::LAST_MODIFIED, &lm)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&c).unwrap()))
        .unwrap())
}

async fn update(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((book_id, id)): Path<(Uuid, Uuid)>,
    _headers: HeaderMap,
    raw: String,
) -> Result<Response> {
    validate_vcard(&raw, MAX_CONTACT_VCARD_BYTES)?;
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, book_id, ctx.user_id).await?;
    let c    = ContactRepo::new(pool).update(ctx.tenant_id, id, &raw).await?;
    state.bus().publish(ContactsEvent::ContactUpserted {
        tenant_id: ctx.tenant_id, addressbook_id: book_id, contact_id: c.id,
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::ETAG, format!("\"{}\"", c.etag))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&c).unwrap()))
        .unwrap())
}

async fn delete(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path((book_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode> {
    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, book_id, ctx.user_id).await?;
    ContactRepo::new(pool).delete(ctx.tenant_id, id).await?;
    state.bus().publish(ContactsEvent::ContactDeleted {
        tenant_id: ctx.tenant_id, addressbook_id: book_id, contact_id: id,
    });
    Ok(StatusCode::NO_CONTENT)
}


/// GET /api/v1/addressbooks/:book_id/export.vcf — concat of all contacts'
/// raw vCards as a single text/vcard download.
async fn export_vcf(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
) -> Result<Response> {
    use crate::domain::vcard;
    let pool  = state.db_or_unavailable()?;
    let cs    = ContactRepo::new(pool).list(ctx.tenant_id, book_id).await?;
    let cards: Vec<String> = cs.into_iter().map(|c| c.vcard_raw).collect();
    let body = vcard::concat_vcards(&cards);
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/vcard; charset=utf-8")
        .header(header::CONTENT_DISPOSITION, "attachment; filename=\"addressbook.vcf\"")
        .body(Body::from(body))
        .unwrap())
}

/// POST /api/v1/addressbooks/:book_id/import — body is a file with 1..N
/// BEGIN:VCARD..END:VCARD blocks. Each is upserted by UID. Returns summary.
async fn import_vcf(
    State(state): State<AppState>,
    ctx: RequestCtx,
    Path(book_id): Path<Uuid>,
    raw: String,
) -> Result<Response> {
    use crate::domain::vcard;
    validate_vcard(&raw, MAX_IMPORT_VCF_BYTES)?;
    let blocks = vcard::split_vcards(&raw);
    if blocks.is_empty() {
        return Err(crate::error::ContactsError::InvalidVCard("no VCARD blocks found".into()));
    }
    let pool = state.db_or_unavailable()?;
    let repo = ContactRepo::new(pool);
    let mut imported = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for (idx, block) in blocks.iter().enumerate() {
        match repo.replace_by_uid(ctx.tenant_id, book_id, block).await {
            Ok(_)  => imported += 1,
            Err(e) => errors.push(format!("vcard[{idx}]: {e}")),
        }
    }
    let body = serde_json::json!({
        "imported": imported,
        "failed":   errors.len(),
        "errors":   errors,
    });
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap())
}

/// POST /api/v1/contacts/import — bulk import via multipart CSV.
///
/// Multipart fields:
///   - `book_id`  (text): UUID of the destination addressbook
///   - `file`     (file): CSV with header row; recognized columns:
///                        first_name, last_name, email, phone, organization
///
/// Returns `{ imported, failed, errors: [string] }`.
async fn import_csv(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    mut mp:       Multipart,
) -> Result<Json<serde_json::Value>> {
    let mut book_id_raw: Option<String> = None;
    let mut csv_bytes:   Option<Vec<u8>> = None;

    while let Some(field) = mp.next_field().await
        .map_err(|e| ContactsError::BadRequest(format!("multipart error: {e}")))?
    {
        match field.name() {
            Some("book_id") => {
                let text = field.text().await
                    .map_err(|e| ContactsError::BadRequest(format!("book_id field: {e}")))?;
                book_id_raw = Some(text);
            }
            Some("file") => {
                let bytes = field.bytes().await
                    .map_err(|e| ContactsError::BadRequest(format!("file field: {e}")))?;
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err(ContactsError::BadRequest("CSV too large (max 4 MiB)".into()));
                }
                csv_bytes = Some(bytes.to_vec());
            }
            _ => {}
        }
    }

    let book_id: Uuid = book_id_raw
        .ok_or_else(|| ContactsError::BadRequest("missing book_id field".into()))
        .and_then(|s| s.parse::<Uuid>().map_err(|_| ContactsError::BadRequest("invalid book_id".into())))?;

    let csv_data = csv_bytes.ok_or_else(|| ContactsError::BadRequest("missing file field".into()))?;
    let csv_str  = std::str::from_utf8(&csv_data)
        .map_err(|_| ContactsError::BadRequest("CSV must be UTF-8".into()))?;

    let pool = state.db_or_unavailable()?;
    assert_can_write(pool, ctx.tenant_id, book_id, ctx.user_id).await?;

    let records = parse_csv(csv_str)?;
    if records.is_empty() {
        return Err(ContactsError::BadRequest("CSV has no data rows".into()));
    }

    let repo = ContactRepo::new(pool);
    let mut imported = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for (idx, rec) in records.iter().enumerate() {
        let uid  = format!("csv-import-{}-{}", book_id, uuid::Uuid::new_v4());
        let vcard = build_vcard_from_csv(rec, &uid);
        match repo.replace_by_uid(ctx.tenant_id, book_id, &vcard).await {
            Ok(_)  => {
                imported += 1;
                state.bus().publish(ContactsEvent::ContactUpserted {
                    tenant_id: ctx.tenant_id, addressbook_id: book_id, contact_id: uuid::Uuid::nil(),
                });
            }
            Err(e) => errors.push(format!("row[{idx}]: {e}")),
        }
    }

    Ok(Json(serde_json::json!({
        "imported": imported,
        "failed":   errors.len(),
        "errors":   errors,
    })))
}

/// CSV record — columns recognized by name (case-insensitive).
struct CsvRecord {
    first_name:   Option<String>,
    last_name:    Option<String>,
    email:        Option<String>,
    phone:        Option<String>,
    organization: Option<String>,
}

fn parse_csv(csv: &str) -> Result<Vec<CsvRecord>> {
    let mut lines = csv.lines();
    let header_line = lines.next()
        .ok_or_else(|| ContactsError::BadRequest("CSV is empty".into()))?;
    let headers: Vec<String> = split_csv_row(header_line)
        .into_iter().map(|h| h.trim().to_lowercase()).collect();

    let idx = |name: &str| headers.iter().position(|h| h == name);
    let col_first = idx("first_name");
    let col_last  = idx("last_name");
    let col_email = idx("email");
    let col_phone = idx("phone");
    let col_org   = idx("organization");

    let mut records = Vec::new();
    for line in lines {
        if line.trim().is_empty() { continue; }
        let cols = split_csv_row(line);
        let get  = |ci: Option<usize>| -> Option<String> {
            let s = cols.get(ci?)?;
            if s.is_empty() { None } else { Some(s.clone()) }
        };
        records.push(CsvRecord {
            first_name:   get(col_first),
            last_name:    get(col_last),
            email:        get(col_email),
            phone:        get(col_phone),
            organization: get(col_org),
        });
    }
    Ok(records)
}

/// Minimal RFC 4918 compliant vCard 3.0 from a CSV record.
fn build_vcard_from_csv(rec: &CsvRecord, uid: &str) -> String {
    let mut lines = vec![
        "BEGIN:VCARD".to_string(),
        "VERSION:3.0".to_string(),
        format!("UID:{uid}"),
    ];
    let fn_str = match (&rec.first_name, &rec.last_name) {
        (Some(f), Some(l)) => format!("{f} {l}"),
        (Some(f), None)    => f.clone(),
        (None, Some(l))    => l.clone(),
        (None, None)       => rec.email.clone().unwrap_or_else(|| "Unknown".to_string()),
    };
    let family  = rec.last_name.as_deref().unwrap_or("");
    let given   = rec.first_name.as_deref().unwrap_or("");
    lines.push(format!("FN:{fn_str}"));
    lines.push(format!("N:{family};{given};;;"));
    if let Some(email) = &rec.email {
        lines.push(format!("EMAIL;TYPE=INTERNET:{email}"));
    }
    if let Some(phone) = &rec.phone {
        lines.push(format!("TEL;TYPE=VOICE:{phone}"));
    }
    if let Some(org) = &rec.organization {
        lines.push(format!("ORG:{org}"));
    }
    lines.push("END:VCARD".to_string());
    lines.join("\r\n")
}

/// Naive CSV row splitter: handles double-quoted fields with embedded commas.
fn split_csv_row(row: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur    = String::new();
    let mut in_q   = false;
    let mut chars  = row.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_q => in_q = true,
            '"' if in_q  => {
                if chars.peek() == Some(&'"') { chars.next(); cur.push('"'); }
                else { in_q = false; }
            }
            ',' if !in_q => { fields.push(cur.trim().to_string()); cur = String::new(); }
            other        => cur.push(other),
        }
    }
    fields.push(cur.trim().to_string());
    fields
}

/// Gate aplicado em todos endpoints que aceitam vCard raw. Tamanho
/// primeiro pra rejeitar abuso antes de tocar o parser. `empty body`
/// já era rejeitado no import — agora unificado pros 3 endpoints.
fn validate_vcard(raw: &str, max_bytes: usize) -> Result<()> {
    if raw.trim().is_empty() {
        return Err(ContactsError::InvalidVCard("empty body".into()));
    }
    if raw.len() > max_bytes {
        return Err(ContactsError::InvalidVCard(format!(
            "vcard payload too large: {} bytes (max {})",
            raw.len(), max_bytes
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty() {
        let err = format!("{:?}", validate_vcard("", MAX_CONTACT_VCARD_BYTES).unwrap_err());
        assert!(err.contains("empty body"), "got: {err}");
    }

    #[test]
    fn accepts_small_vcard() {
        let s = "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:abc\r\nFN:John\r\nEND:VCARD\r\n";
        assert!(validate_vcard(s, MAX_CONTACT_VCARD_BYTES).is_ok());
    }

    #[test]
    fn rejects_oversize_contact() {
        let s = "x".repeat(MAX_CONTACT_VCARD_BYTES + 1);
        let err = format!("{:?}", validate_vcard(&s, MAX_CONTACT_VCARD_BYTES).unwrap_err());
        assert!(err.contains("too large"), "got: {err}");
    }

    #[test]
    fn import_cap_higher_than_contact_cap() {
        let s = "x".repeat(MAX_CONTACT_VCARD_BYTES + 1);
        assert!(validate_vcard(&s, MAX_CONTACT_VCARD_BYTES).is_err());
        assert!(validate_vcard(&s, MAX_IMPORT_VCF_BYTES).is_ok());
    }

    #[test]
    fn boundary_contact_accepted() {
        let s = "x".repeat(MAX_CONTACT_VCARD_BYTES);
        assert!(validate_vcard(&s, MAX_CONTACT_VCARD_BYTES).is_ok());
    }

    #[test]
    fn max_contact_vcard_bytes_is_64kb() {
        assert_eq!(MAX_CONTACT_VCARD_BYTES, 64 * 1024);
    }

    #[test]
    fn max_import_vcf_bytes_is_4mb() {
        assert_eq!(MAX_IMPORT_VCF_BYTES, 4 * 1024 * 1024);
    }

    #[test]
    fn empty_string_accepted_by_size_check() {
        assert!(validate_vcard("", MAX_CONTACT_VCARD_BYTES).is_ok());
    }

    #[test]
    fn oversized_vcard_rejected() {
        let too_big = "x".repeat(MAX_CONTACT_VCARD_BYTES + 1);
        assert!(validate_vcard(&too_big, MAX_CONTACT_VCARD_BYTES).is_err());
    }

    #[test]
    fn empty_vcard_rejected() {
        assert!(validate_vcard("   ", MAX_CONTACT_VCARD_BYTES).is_err());
    }

    #[test]
    fn non_empty_vcard_within_limit_accepted() {
        assert!(validate_vcard("BEGIN:VCARD\r\nEND:VCARD\r\n", MAX_CONTACT_VCARD_BYTES).is_ok());
    }

    #[test]
    fn max_contact_vcard_bytes_exact_value() {
        assert_eq!(MAX_CONTACT_VCARD_BYTES, 64 * 1024);
    }

    #[test]
    fn oversize_vcard_rejected() {
        let s = "x".repeat(MAX_CONTACT_VCARD_BYTES + 1);
        assert!(validate_vcard(&s, MAX_CONTACT_VCARD_BYTES).is_err());
    }

    #[test]
    fn import_cap_is_4mb() {
        assert_eq!(MAX_IMPORT_VCF_BYTES, 4 * 1024 * 1024);
    }

    #[test]
    fn contact_cap_smaller_than_import_cap() {
        assert!(MAX_CONTACT_VCARD_BYTES < MAX_IMPORT_VCF_BYTES);
    }

    #[test]
    fn max_contact_vcard_bytes_is_nonzero() {
        assert!(MAX_CONTACT_VCARD_BYTES > 0);
    }
}

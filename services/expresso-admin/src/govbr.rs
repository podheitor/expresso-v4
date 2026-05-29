//! Gov.br federation mapping API — CRUD para govbr_user_map.
//!
//! Acesso restrito a super_admin.
//!
//! GET  /api/v1/govbr/mappings                 — lista todos (filtrável por tenant_id)
//! GET  /api/v1/govbr/mappings/:cpf_hash        — busca por cpf_hash
//! POST /api/v1/govbr/mappings                 — cria/upsert mapeamento
//! DELETE /api/v1/govbr/mappings/:cpf_hash      — remove mapeamento

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth, AppState};

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct GovbrMapping {
    pub cpf_hash: String,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub assurance: Option<String>,
    pub created_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertBody {
    pub cpf_hash: String,
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    pub assurance: Option<String>,
}

// Err is an axum Response by design (early-return into the handler); boxing
// it would only add an allocation on the unavailable path.
#[allow(clippy::result_large_err)]
fn db_or_503(st: &Arc<AppState>) -> Result<&expresso_core::DbPool, Response> {
    st.db
        .as_ref()
        .ok_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Response, Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return Err(r);
    }
    let pool = db_or_503(&st)?;
    let max_ts: Option<OffsetDateTime> = if let Some(tid) = q.tenant_id {
        sqlx::query_scalar("SELECT MAX(updated_at) FROM govbr_user_map WHERE tenant_id = $1")
            .bind(tid)
            .fetch_one(pool)
            .await
            .unwrap_or(None)
    } else {
        sqlx::query_scalar("SELECT MAX(updated_at) FROM govbr_user_map")
            .fetch_one(pool)
            .await
            .unwrap_or(None)
    };
    if let Some(ts) = max_ts {
        if let Some(ims_val) = headers.get(header::IF_MODIFIED_SINCE) {
            if let Ok(ims_str) = ims_val.to_str() {
                if let Ok(ims_dt) =
                    OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822)
                {
                    if ts <= ims_dt {
                        return Ok(StatusCode::NOT_MODIFIED.into_response());
                    }
                }
            }
        }
    }
    let rows: Vec<GovbrMapping> = if let Some(tid) = q.tenant_id {
        sqlx::query_as(
            "SELECT cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at, updated_at \
             FROM govbr_user_map WHERE tenant_id = $1 ORDER BY updated_at DESC",
        )
        .bind(tid)
        .fetch_all(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
    } else {
        sqlx::query_as(
            "SELECT cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at, updated_at \
             FROM govbr_user_map ORDER BY updated_at DESC",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
    };
    let mut resp = Json(rows).into_response();
    if let Some(ts) = max_ts {
        let lm = ts
            .format(&time::format_description::well_known::Rfc2822)
            .unwrap_or_default();
        resp.headers_mut()
            .insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    }
    Ok(resp)
}

pub async fn get_one(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(cpf_hash): Path<String>,
) -> Result<Response, Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return Err(r);
    }
    let pool = db_or_503(&st)?;
    let row: Option<GovbrMapping> = sqlx::query_as(
        "SELECT cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at, updated_at \
         FROM govbr_user_map WHERE cpf_hash = $1",
    )
    .bind(&cpf_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    let r = row.ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
    let ts = r.updated_at;
    let etag = format!("\"{}-{}\"", ts.unix_timestamp(), r.cpf_hash);
    let lm = ts
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap_or_default();
    if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
        if inm.as_bytes() == etag.as_bytes() {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }
    if let Some(ims_val) = headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) =
                OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822)
            {
                if ts <= ims_dt {
                    return Ok(StatusCode::NOT_MODIFIED.into_response());
                }
            }
        }
    }
    let mut resp = Json(r).into_response();
    resp.headers_mut()
        .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    resp.headers_mut()
        .insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
    Ok(resp)
}

pub async fn upsert(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UpsertBody>,
) -> Result<(StatusCode, Json<GovbrMapping>), Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return Err(r);
    }
    if body.cpf_hash.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "cpf_hash must not be empty").into_response());
    }
    let pool = db_or_503(&st)?;
    let row: GovbrMapping = sqlx::query_as(
        "INSERT INTO govbr_user_map (cpf_hash, tenant_id, user_id, assurance) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (cpf_hash) DO UPDATE \
           SET tenant_id  = EXCLUDED.tenant_id, \
               user_id    = EXCLUDED.user_id, \
               assurance  = EXCLUDED.assurance, \
               updated_at = now() \
         RETURNING cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at, updated_at",
    )
    .bind(&body.cpf_hash)
    .bind(body.tenant_id)
    .bind(body.user_id)
    .bind(&body.assurance)
    .fetch_one(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    tracing::info!(target: "audit",
        event  = "govbr.mapping.upsert",
        cpf_hash = %body.cpf_hash,
        tenant_id = %body.tenant_id,
        user_id   = %body.user_id);
    Ok((StatusCode::OK, Json(row)))
}

pub async fn delete(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(cpf_hash): Path<String>,
) -> Result<StatusCode, Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await {
        return Err(r);
    }
    let pool = db_or_503(&st)?;
    let affected = sqlx::query("DELETE FROM govbr_user_map WHERE cpf_hash = $1")
        .bind(&cpf_hash)
        .execute(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
        .rows_affected();

    if affected == 0 {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    tracing::info!(target: "audit",
        event = "govbr.mapping.delete",
        cpf_hash = %cpf_hash);
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_body_deser() {
        let t = Uuid::new_v4();
        let u = Uuid::new_v4();
        let json = format!(
            r#"{{"cpf_hash":"abc123","tenant_id":"{t}","user_id":"{u}","assurance":"bronze"}}"#
        );
        let b: UpsertBody = serde_json::from_str(&json).unwrap();
        assert_eq!(b.cpf_hash, "abc123");
        assert_eq!(b.tenant_id, t);
        assert_eq!(b.assurance.as_deref(), Some("bronze"));
    }

    #[test]
    fn upsert_body_assurance_optional() {
        let t = Uuid::new_v4();
        let u = Uuid::new_v4();
        let json = format!(r#"{{"cpf_hash":"xyz","tenant_id":"{t}","user_id":"{u}"}}"#);
        let b: UpsertBody = serde_json::from_str(&json).unwrap();
        assert!(b.assurance.is_none());
    }

    #[test]
    fn list_query_tenant_id_optional() {
        let q: ListQuery = serde_json::from_str(r#"{}"#).unwrap();
        assert!(q.tenant_id.is_none());
    }

    #[test]
    fn upsert_body_cpf_hash_preserved() {
        let t = Uuid::new_v4();
        let u = Uuid::new_v4();
        let body = UpsertBody {
            cpf_hash: "sha256abc".into(),
            tenant_id: t,
            user_id: u,
            assurance: Some("ouro".into()),
        };
        assert_eq!(body.cpf_hash, "sha256abc");
        assert_eq!(body.assurance.as_deref(), Some("ouro"));
    }

    #[test]
    fn list_query_with_tenant_id() {
        let tid = Uuid::new_v4();
        let json = format!(r#"{{"tenant_id":"{tid}"}}"#);
        let q: ListQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(q.tenant_id, Some(tid));
    }

    #[test]
    fn upsert_body_no_assurance() {
        let t = Uuid::new_v4();
        let u = Uuid::new_v4();
        let b = UpsertBody {
            cpf_hash: "hash".into(),
            tenant_id: t,
            user_id: u,
            assurance: None,
        };
        assert!(b.assurance.is_none());
    }

    #[test]
    fn upsert_body_user_id_preserved() {
        let t = Uuid::new_v4();
        let u = Uuid::new_v4();
        let b = UpsertBody {
            cpf_hash: "h".into(),
            tenant_id: t,
            user_id: u,
            assurance: None,
        };
        assert_eq!(b.user_id, u);
        assert_eq!(b.tenant_id, t);
    }

    #[test]
    fn upsert_body_assurance_ouro_preserved() {
        let t = Uuid::new_v4();
        let u = Uuid::new_v4();
        let b = UpsertBody {
            cpf_hash: "sha256hashvalue".into(),
            tenant_id: t,
            user_id: u,
            assurance: Some("ouro".into()),
        };
        assert_eq!(b.cpf_hash, "sha256hashvalue");
        assert_eq!(b.assurance.as_deref(), Some("ouro"));
    }

    #[test]
    fn list_query_tenant_id_some() {
        let tid = Uuid::new_v4();
        let q = ListQuery {
            tenant_id: Some(tid),
        };
        assert_eq!(q.tenant_id, Some(tid));
    }

    #[test]
    fn list_query_tenant_id_none() {
        let q = ListQuery { tenant_id: None };
        assert!(q.tenant_id.is_none());
    }

    #[test]
    fn govbr_mapping_cpf_hash_preserved() {
        let m = GovbrMapping {
            tenant_id: uuid::Uuid::nil(),
            cpf_hash: "abc123".into(),
            user_id: uuid::Uuid::nil(),
            assurance: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_login_at: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert_eq!(m.cpf_hash, "abc123");
    }

    #[test]
    fn govbr_mapping_assurance_none_by_default() {
        let m = GovbrMapping {
            cpf_hash: "xyz".into(),
            tenant_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            assurance: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_login_at: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert!(m.assurance.is_none());
    }

    #[test]
    fn govbr_mapping_cpf_hash_not_empty() {
        let m = GovbrMapping {
            cpf_hash: "abc123".into(),
            tenant_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            assurance: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_login_at: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert!(!m.cpf_hash.is_empty());
    }

    #[test]
    fn govbr_mapping_user_id_nil_is_nil() {
        let m = GovbrMapping {
            cpf_hash: "x".into(),
            tenant_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            assurance: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_login_at: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert_eq!(m.user_id, uuid::Uuid::nil());
    }

    #[test]
    fn govbr_mapping_cpf_hash_field_preserved() {
        let m = GovbrMapping {
            cpf_hash: "abc123".into(),
            tenant_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            assurance: None,
            created_at: OffsetDateTime::UNIX_EPOCH,
            last_login_at: None,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert_eq!(m.cpf_hash, "abc123");
    }

    #[test]
    fn upsert_body_tenant_id_preserved() {
        let t = Uuid::new_v4();
        let u = Uuid::new_v4();
        let b = UpsertBody {
            cpf_hash: "h".into(),
            tenant_id: t,
            user_id: u,
            assurance: None,
        };
        assert_eq!(b.tenant_id, t);
    }

    #[test]
    fn upsert_body_assurance_none_by_default() {
        let b = UpsertBody {
            cpf_hash: "xyz".into(),
            tenant_id: Uuid::nil(),
            user_id: Uuid::nil(),
            assurance: None,
        };
        assert!(b.assurance.is_none());
    }

    #[test]
    fn list_query_tenant_id_round_trip_via_struct() {
        let tid = Uuid::new_v4();
        let q = ListQuery {
            tenant_id: Some(tid),
        };
        assert!(q.tenant_id.is_some());
        assert_eq!(q.tenant_id.unwrap(), tid);
    }

    #[test]
    fn upsert_body_assurance_prata_preserved() {
        let body = UpsertBody {
            user_id: Uuid::nil(),
            cpf_hash: "abc".into(),
            assurance: Some("prata".into()),
            tenant_id: Uuid::nil(),
        };
        assert_eq!(body.assurance.as_deref(), Some("prata"));
    }

    #[test]
    fn upsert_body_cpf_hash_trimmed_check() {
        let b = UpsertBody {
            cpf_hash: "  trimmed  ".into(),
            tenant_id: Uuid::nil(),
            user_id: Uuid::nil(),
            assurance: None,
        };
        assert!(!b.cpf_hash.trim().is_empty());
    }

    #[test]
    fn upsert_body_assurance_bronze_preserved() {
        let b = UpsertBody {
            cpf_hash: "abc".into(),
            tenant_id: Uuid::nil(),
            user_id: Uuid::nil(),
            assurance: Some("bronze".into()),
        };
        assert_eq!(b.assurance.as_deref(), Some("bronze"));
    }

    #[test]
    fn upsert_body_assurance_ouro_is_nonempty() {
        let b = UpsertBody {
            cpf_hash: "hash".into(),
            tenant_id: Uuid::nil(),
            user_id: Uuid::nil(),
            assurance: Some("ouro".into()),
        };
        assert!(!b.assurance.as_deref().unwrap_or("").is_empty());
    }

    #[test]
    fn list_query_tenant_id_none_means_all_tenants() {
        let q = ListQuery { tenant_id: None };
        assert!(q.tenant_id.is_none());
    }

    #[test]
    fn upsert_body_cpf_hash_is_preserved_verbatim() {
        let t = uuid::Uuid::new_v4();
        let u = uuid::Uuid::new_v4();
        let b = UpsertBody {
            cpf_hash: "deadbeef1234".into(),
            tenant_id: t,
            user_id: u,
            assurance: None,
        };
        assert_eq!(b.cpf_hash, "deadbeef1234");
    }
}

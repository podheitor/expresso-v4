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
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{auth, AppState};

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct GovbrMapping {
    pub cpf_hash:      String,
    pub tenant_id:     Uuid,
    pub user_id:       Uuid,
    pub assurance:     Option<String>,
    pub created_at:    OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertBody {
    pub cpf_hash:  String,
    pub tenant_id: Uuid,
    pub user_id:   Uuid,
    pub assurance: Option<String>,
}

fn db_or_503(st: &Arc<AppState>) -> Result<&expresso_core::DbPool, Response> {
    st.db.as_ref().ok_or_else(|| StatusCode::SERVICE_UNAVAILABLE.into_response())
}

pub async fn list(
    State(st):   State<Arc<AppState>>,
    headers:     HeaderMap,
    Query(q):    Query<ListQuery>,
) -> Result<Json<Vec<GovbrMapping>>, Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Err(r); }
    let pool = db_or_503(&st)?;
    let rows: Vec<GovbrMapping> = if let Some(tid) = q.tenant_id {
        sqlx::query_as(
            "SELECT cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at \
             FROM govbr_user_map WHERE tenant_id = $1 ORDER BY created_at DESC",
        )
        .bind(tid)
        .fetch_all(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
    } else {
        sqlx::query_as(
            "SELECT cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at \
             FROM govbr_user_map ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?
    };
    Ok(Json(rows))
}

pub async fn get_one(
    State(st):         State<Arc<AppState>>,
    headers:           HeaderMap,
    Path(cpf_hash):    Path<String>,
) -> Result<Json<GovbrMapping>, Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Err(r); }
    let pool = db_or_503(&st)?;
    let row: Option<GovbrMapping> = sqlx::query_as(
        "SELECT cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at \
         FROM govbr_user_map WHERE cpf_hash = $1",
    )
    .bind(&cpf_hash)
    .fetch_optional(pool)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response())?;

    match row {
        Some(r) => Ok(Json(r)),
        None    => Err(StatusCode::NOT_FOUND.into_response()),
    }
}

pub async fn upsert(
    State(st):  State<Arc<AppState>>,
    headers:    HeaderMap,
    Json(body): Json<UpsertBody>,
) -> Result<(StatusCode, Json<GovbrMapping>), Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Err(r); }
    if body.cpf_hash.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "cpf_hash must not be empty").into_response());
    }
    let pool = db_or_503(&st)?;
    let row: GovbrMapping = sqlx::query_as(
        "INSERT INTO govbr_user_map (cpf_hash, tenant_id, user_id, assurance) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (cpf_hash) DO UPDATE \
           SET tenant_id = EXCLUDED.tenant_id, \
               user_id   = EXCLUDED.user_id, \
               assurance = EXCLUDED.assurance \
         RETURNING cpf_hash, tenant_id, user_id, assurance, created_at, last_login_at",
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
    State(st):      State<Arc<AppState>>,
    headers:        HeaderMap,
    Path(cpf_hash): Path<String>,
) -> Result<StatusCode, Response> {
    if let Some(r) = auth::require_super_admin(&st, &headers).await { return Err(r); }
    let pool = db_or_503(&st)?;
    let affected = sqlx::query(
        "DELETE FROM govbr_user_map WHERE cpf_hash = $1",
    )
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

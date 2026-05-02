//! Drive file tags — sprint #415.
//!
//! Routes:
//!   POST   /api/v1/drive/files/:id/tags           — add a tag to a file
//!   POST   /api/v1/drive/files/:id/tags/bulk      — add multiple tags atomically (sprint #421)
//!   DELETE /api/v1/drive/files/:id/tags/:tag      — remove a tag from a file
//!   DELETE /api/v1/drive/files/:id/tags           — clear all tags on a file (sprint #416)
//!   GET    /api/v1/drive/files/:id/tags           — list tags on a file
//!   GET    /api/v1/drive/tags/:tag                — list files with this tag (tenant-scoped)
//!   PATCH  /api/v1/drive/tags/:tag                — rename a tag em todas as files (sprint #430)
//!   POST   /api/v1/drive/tags/:tag/merge          — funde 2 tags numa só (sprint #433)
//!   DELETE /api/v1/drive/tags/orphans             — apaga tags ligadas a files inexistentes ou soft-deleted (sprint #443)
//!   GET    /api/v1/drive/tags/stats                — contagem de files por tag no tenant (sprint #448)
//!   GET    /api/v1/drive/tags/:tag/count           — contagem de files com uma tag específica (sprint #455)
//!   GET    /api/v1/drive/tags/intersect?tags=a,b,c — files que possuem TODAS as tags listadas (AND, sprint #465)
//!   GET    /api/v1/drive/tags/union?tags=a,b,c     — files que possuem PELO MENOS UMA das tags (OR, sprint #467)
//!   GET    /api/v1/drive/tags/rename-history       — audit trail de renames passados (sprint #470)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use expresso_core::begin_tenant_tx;

use crate::api::context::RequestCtx;
use crate::error::{DriveError, Result};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileTag {
    pub id:         Uuid,
    pub file_id:    Uuid,
    pub tenant_id:  Uuid,
    pub tag:        String,
    pub created_by: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct AddTagBody {
    tag: String,
}

#[derive(Debug, Deserialize)]
struct BulkTagsBody {
    tags: Vec<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/drive/files/:id/tags",
            get(list_file_tags).post(add_tag).delete(clear_tags),
        )
        .route(
            "/api/v1/drive/files/:id/tags/bulk",
            post(bulk_add_tags),
        )
        .route(
            "/api/v1/drive/files/:id/tags/:tag",
            delete(remove_tag),
        )
        .route(
            "/api/v1/drive/tags/:tag",
            get(list_files_by_tag).patch(rename_tag),
        )
        .route(
            "/api/v1/drive/tags/:tag/merge",
            post(merge_tag),
        )
        .route(
            "/api/v1/drive/tags/:tag/count",
            get(count_files_by_tag),
        )
        .route(
            "/api/v1/drive/tags/orphans",
            delete(delete_orphan_tags),
        )
        .route(
            "/api/v1/drive/tags/stats",
            get(tag_stats),
        )
        .route(
            "/api/v1/drive/tags/intersect",
            get(intersect_files_by_tags),
        )
        .route(
            "/api/v1/drive/tags/union",
            get(union_files_by_tags),
        )
        .route(
            "/api/v1/drive/tags/rename-history",
            get(list_tag_rename_history),
        )
}

#[derive(Debug, Serialize, FromRow)]
struct TagStat {
    tag:        String,
    file_count: i64,
}

/// GET /api/v1/drive/tags/stats — contagem de files distintos por tag no tenant.
/// Conta apenas files ativos (deleted_at IS NULL). Ordenado por count DESC, depois
/// alfabético. Útil pra "tag cloud" e dashboards. Path estático ganha precedência
/// sobre `/:tag` (lição #443).
async fn tag_stats(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    let stats: Vec<TagStat> = sqlx::query_as(
        "SELECT t.tag, COUNT(DISTINCT t.file_id) AS file_count \
         FROM drive_file_tags t \
         JOIN drive_files f ON f.id = t.file_id AND f.tenant_id = t.tenant_id \
         WHERE t.tenant_id = $1 AND f.deleted_at IS NULL \
         GROUP BY t.tag \
         ORDER BY file_count DESC, t.tag ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(stats))
}

#[derive(Debug, Serialize)]
struct OrphansCleanupResult {
    removed: u64,
}

/// DELETE /api/v1/drive/tags/orphans — apaga tags do tenant que apontam pra
/// files inexistentes ou com deleted_at definido (soft-deleted). Idempotente.
/// Retorna `{removed: N}` com a contagem de linhas apagadas. Path estático
/// `/orphans` ganha precedência sobre `/:tag` em axum (lição #440).
async fn delete_orphan_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    let r = sqlx::query(
        "DELETE FROM drive_file_tags t \
          WHERE t.tenant_id = $1 \
            AND NOT EXISTS ( \
                SELECT 1 FROM drive_files f \
                 WHERE f.id = t.file_id \
                   AND f.tenant_id = t.tenant_id \
                   AND f.deleted_at IS NULL \
            )",
    )
    .bind(ctx.tenant_id)
    .execute(pool)
    .await?;
    Ok(Json(OrphansCleanupResult { removed: r.rows_affected() }))
}

#[derive(Debug, Deserialize)]
struct MergeTagBody {
    into: String,
}

#[derive(Debug, Serialize)]
struct MergeTagResult {
    merged: u64,
    into:   String,
}

/// POST /api/v1/drive/tags/:tag/merge — funde `:tag` em `into` (sprint #433),
/// consolidando todos os files do tag-fonte no tag-destino. Body: `{into: "..."}`.
/// Diferente de rename: ambas as tags podem existir; arquivos que já tinham `into`
/// têm o registro de `:tag` apagado (pré-DELETE evita unique conflict), e os
/// demais têm o tag UPDATEado para `into`. Idempotente. Retorna `{merged: N, into}`
/// com a contagem de UPDATEs efetuados.
async fn merge_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(tag):    Path<String>,
    Json(body):   Json<MergeTagBody>,
) -> Result<impl IntoResponse> {
    let src  = tag.trim().to_lowercase();
    let dst  = body.into.trim().to_lowercase();
    if dst.is_empty() || dst.chars().count() > 64 {
        return Err(DriveError::BadRequest("into must be 1-64 characters".into()));
    }
    if dst == src {
        return Err(DriveError::BadRequest("into must differ from source tag".into()));
    }

    let pool = state.db_or_unavailable()?;

    // Mesma técnica do rename (#430): pré-DELETE dos files que já tinham `dst`
    // pra liberar a chave única (file_id, tenant_id, tag) antes do UPDATE.
    let _ = sqlx::query(
        "DELETE FROM drive_file_tags \
         WHERE tenant_id = $1 AND tag = $2 \
           AND file_id IN ( \
               SELECT file_id FROM drive_file_tags \
               WHERE tenant_id = $1 AND tag = $3 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .bind(&dst)
    .execute(pool)
    .await?;

    let r = sqlx::query(
        "UPDATE drive_file_tags SET tag = $2 \
         WHERE tenant_id = $1 AND tag = $3",
    )
    .bind(ctx.tenant_id)
    .bind(&dst)
    .bind(&src)
    .execute(pool)
    .await?;

    Ok(Json(MergeTagResult { merged: r.rows_affected(), into: dst }))
}

#[derive(Debug, Deserialize)]
struct RenameTagBody {
    new_tag: String,
}

#[derive(Debug, Serialize)]
struct RenameTagResult {
    renamed: u64,
    new_tag: String,
}

/// PATCH /api/v1/drive/tags/:tag — renomeia uma tag em todos os arquivos do tenant
/// (sprint #430). Body: `{new_tag: "..."}`. Idempotente: se algum arquivo já tinha
/// `new_tag`, ON CONFLICT mantém o registro existente e o registro antigo é apagado.
/// Retorna `{renamed: N, new_tag}` com a contagem de linhas afetadas pelo UPDATE.
///
/// Sprint #470: pré-DELETE + UPDATE + insert na drive_tag_rename_history são
/// agora atômicos via begin_tenant_tx (RLS) — se a tabela history falhar, todo o
/// rename roda rollback. History grava `{tenant_id, old_tag, new_tag,
/// renamed_count, renamed_by, renamed_at}` para audit trail.
async fn rename_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(tag):    Path<String>,
    Json(body):   Json<RenameTagBody>,
) -> Result<impl IntoResponse> {
    let old = tag.trim().to_lowercase();
    let new = body.new_tag.trim().to_lowercase();
    if new.is_empty() || new.chars().count() > 64 {
        return Err(DriveError::BadRequest("new_tag must be 1-64 characters".into()));
    }
    if new == old {
        return Err(DriveError::BadRequest("new_tag must differ from old tag".into()));
    }

    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    // Apaga registros que já tinham new_tag nos files que também têm old_tag — evita
    // unique conflict ao renomear; o file já está taggeado com new, basta dropar o old.
    let _ = sqlx::query(
        "DELETE FROM drive_file_tags \
         WHERE tenant_id = $1 AND tag = $2 \
           AND file_id IN ( \
               SELECT file_id FROM drive_file_tags \
               WHERE tenant_id = $1 AND tag = $3 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&new)
    .bind(&old)
    .execute(&mut *tx)
    .await?;

    let r = sqlx::query(
        "UPDATE drive_file_tags SET tag = $2 \
         WHERE tenant_id = $1 AND tag = $3",
    )
    .bind(ctx.tenant_id)
    .bind(&new)
    .bind(&old)
    .execute(&mut *tx)
    .await?;

    let renamed = r.rows_affected();

    sqlx::query(
        "INSERT INTO drive_tag_rename_history \
            (tenant_id, old_tag, new_tag, renamed_count, renamed_by) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(ctx.tenant_id)
    .bind(&old)
    .bind(&new)
    .bind(renamed as i64)
    .bind(ctx.user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(RenameTagResult { renamed, new_tag: new }))
}

#[derive(Debug, Serialize, FromRow)]
struct TagRenameHistoryEntry {
    id:            Uuid,
    old_tag:       String,
    new_tag:       String,
    renamed_count: i64,
    renamed_by:    Uuid,
    #[serde(with = "time::serde::rfc3339")]
    renamed_at:    OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct TagRenameHistoryQuery {
    limit:  Option<i64>,
    since:  Option<OffsetDateTime>,
    before: Option<OffsetDateTime>,
    tag:    Option<String>,
}

/// GET /api/v1/drive/tags/rename-history?limit=&since=&before=&tag= — lista
/// renames passados no tenant, ordem decrescente por `renamed_at` (sprint #470).
/// Filtros opcionais: range temporal (`since`/`before`) e `tag` (matching tanto
/// old_tag quanto new_tag, normalizada lowercase). Limit padrão 50, cap 1..500.
/// Útil pra audit ("quem renomeou X pra Y, quando, quantos arquivos afetou") e
/// pra UI de undo manual. Path estático precede `/:tag` (lição #443/#448).
async fn list_tag_rename_history(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<TagRenameHistoryQuery>,
) -> Result<impl IntoResponse> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let tag_filter = q.tag.map(|t| t.trim().to_lowercase());

    let pool = state.db_or_unavailable()?;

    let entries: Vec<TagRenameHistoryEntry> = sqlx::query_as(
        "SELECT id, old_tag, new_tag, renamed_count, renamed_by, renamed_at \
           FROM drive_tag_rename_history \
          WHERE tenant_id = $1 \
            AND ($2::timestamptz IS NULL OR renamed_at >= $2) \
            AND ($3::timestamptz IS NULL OR renamed_at <  $3) \
            AND ($4::text IS NULL OR old_tag = $4 OR new_tag = $4) \
          ORDER BY renamed_at DESC \
          LIMIT $5",
    )
    .bind(ctx.tenant_id)
    .bind(q.since)
    .bind(q.before)
    .bind(tag_filter)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "limit":   limit,
        "entries": entries,
    })))
}

/// POST /api/v1/drive/files/:id/tags/bulk — add multiple tags atomically (sprint #421).
///
/// Body: `{tags: ["a","b","c"]}`. Tags são normalizadas (trim+lowercase), deduplicadas
/// e inseridas num único INSERT com UNNEST. Conflitos (tag já existente) são silenciosamente
/// ignorados via ON CONFLICT DO NOTHING. Retorna a lista atual de tags do arquivo.
async fn bulk_add_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<BulkTagsBody>,
) -> Result<impl IntoResponse> {
    use std::collections::BTreeSet;

    let tags: BTreeSet<String> = body
        .tags
        .into_iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty() && t.chars().count() <= 64)
        .collect();
    if tags.is_empty() {
        return Err(DriveError::BadRequest("at least one valid tag required".into()));
    }
    if tags.len() > 100 {
        return Err(DriveError::BadRequest("max 100 tags per bulk request".into()));
    }
    let tag_vec: Vec<String> = tags.into_iter().collect();

    let pool = state.db_or_unavailable()?;

    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(DriveError::NotFound(id));
    }

    sqlx::query(
        "INSERT INTO drive_file_tags (file_id, tenant_id, tag, created_by) \
         SELECT $1, $2, t, $4 FROM UNNEST($3::text[]) AS t \
         ON CONFLICT (file_id, tenant_id, tag) DO NOTHING",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&tag_vec)
    .bind(ctx.user_id)
    .execute(pool)
    .await?;

    let result: Vec<FileTag> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, tag, created_by, created_at \
         FROM drive_file_tags \
         WHERE tenant_id = $1 AND file_id = $2 \
         ORDER BY tag ASC",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(result)))
}

/// POST /api/v1/drive/files/:id/tags — add a tag to a file.
async fn add_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
    Json(body):   Json<AddTagBody>,
) -> Result<impl IntoResponse> {
    let tag = body.tag.trim().to_lowercase();
    if tag.is_empty() || tag.chars().count() > 64 {
        return Err(DriveError::BadRequest("tag must be 1-64 characters".into()));
    }

    let pool = state.db_or_unavailable()?;

    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(DriveError::NotFound(id));
    }

    let file_tag: FileTag = sqlx::query_as(
        "INSERT INTO drive_file_tags (file_id, tenant_id, tag, created_by) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (file_id, tenant_id, tag) DO UPDATE \
             SET created_at = drive_file_tags.created_at \
         RETURNING id, file_id, tenant_id, tag, created_by, created_at",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&tag)
    .bind(ctx.user_id)
    .fetch_one(pool)
    .await?;

    Ok((StatusCode::CREATED, Json(file_tag)))
}

/// DELETE /api/v1/drive/files/:id/tags/:tag — remove a tag from a file.
async fn remove_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path((id, tag)): Path<(Uuid, String)>,
) -> Result<impl IntoResponse> {
    let tag = tag.trim().to_lowercase();
    let pool = state.db_or_unavailable()?;

    let r = sqlx::query(
        "DELETE FROM drive_file_tags WHERE file_id = $1 AND tenant_id = $2 AND tag = $3",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .bind(&tag)
    .execute(pool)
    .await?;

    if r.rows_affected() == 0 {
        return Err(DriveError::NotFound(id));
    }

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/drive/files/:id/tags — remove all tags from a file (sprint #416).
async fn clear_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(DriveError::NotFound(id));
    }

    sqlx::query(
        "DELETE FROM drive_file_tags WHERE file_id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .execute(pool)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/drive/files/:id/tags — list all tags on a file.
async fn list_file_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;

    let exists: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM drive_files WHERE id = $1 AND tenant_id = $2 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(pool)
    .await?;
    if exists.is_none() {
        return Err(DriveError::NotFound(id));
    }

    let tags: Vec<FileTag> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, tag, created_by, created_at \
         FROM drive_file_tags \
         WHERE tenant_id = $1 AND file_id = $2 \
         ORDER BY tag ASC",
    )
    .bind(ctx.tenant_id)
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(Json(tags))
}

/// GET /api/v1/drive/tags/:tag — list files tagged with this tag (tenant-scoped).
async fn list_files_by_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(tag):    Path<String>,
) -> Result<impl IntoResponse> {
    let tag = tag.trim().to_lowercase();
    let pool = state.db_or_unavailable()?;

    let tags: Vec<FileTag> = sqlx::query_as(
        "SELECT id, file_id, tenant_id, tag, created_by, created_at \
         FROM drive_file_tags \
         WHERE tenant_id = $1 AND tag = $2 \
         ORDER BY created_at DESC",
    )
    .bind(ctx.tenant_id)
    .bind(&tag)
    .fetch_all(pool)
    .await?;

    Ok(Json(tags))
}

#[derive(Debug, Serialize)]
struct TagCount {
    tag:        String,
    file_count: i64,
}

/// GET /api/v1/drive/tags/:tag/count — count antes de listar (sprint #455).
/// Retorna `{tag, file_count}` filtrando files ativos (deleted_at IS NULL),
/// igual `tag_stats` (#448) mas para uma única tag — útil quando UI quer
/// exibir badge ("foo (12)") sem fetchar a lista inteira via /tags/:tag.
/// COUNT DISTINCT pra robustez caso (file_id, tag) duplicasse por algum bug
/// (unique constraint cobre, mas defesa em profundidade). Tag normalizada
/// pra lowercase igual list_files_by_tag.
async fn count_files_by_tag(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(tag):    Path<String>,
) -> Result<impl IntoResponse> {
    let tag = tag.trim().to_lowercase();
    let pool = state.db_or_unavailable()?;

    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(DISTINCT t.file_id) \
         FROM drive_file_tags t \
         JOIN drive_files f ON f.id = t.file_id AND f.tenant_id = t.tenant_id \
         WHERE t.tenant_id = $1 AND t.tag = $2 AND f.deleted_at IS NULL",
    )
    .bind(ctx.tenant_id)
    .bind(&tag)
    .fetch_one(pool)
    .await?;

    Ok(Json(TagCount { tag, file_count: count }))
}

#[derive(Debug, Deserialize)]
struct IntersectQuery {
    /// Comma-separated list of tags (e.g. `?tags=foo,bar,baz`).
    tags: String,
}

#[derive(Debug, Serialize)]
struct IntersectResult {
    tags:        Vec<String>,
    file_ids:    Vec<Uuid>,
    file_count:  i64,
}

/// GET /api/v1/drive/tags/intersect?tags=a,b,c — retorna file_ids que possuem
/// **TODAS** as tags listadas (AND, sprint #465). Complementa
/// `/api/v1/drive/tags/:tag` que faz busca por uma tag só. Tags normalizadas
/// lowercase + trim, deduplicated. Filtra apenas files ativos
/// (deleted_at IS NULL). Implementação: `WHERE t.tag = ANY($3)` + `GROUP BY
/// file_id HAVING COUNT(DISTINCT t.tag) = N` (clássico AND-set query, evita
/// N self-joins). Aceita 1-32 tags. Path estático precede `/:tag` (lição
/// #443/#448) — sem hífen necessário porque `intersect` é distinto de qualquer
/// tag legítima (tag não pode bater com static segment).
async fn intersect_files_by_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<IntersectQuery>,
) -> Result<impl IntoResponse> {
    let mut tags: Vec<String> = q.tags
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort();
    tags.dedup();

    if tags.is_empty() {
        return Err(DriveError::BadRequest("at least one tag required".into()));
    }
    if tags.len() > 32 {
        return Err(DriveError::BadRequest("max 32 tags per query".into()));
    }
    for t in &tags {
        if t.chars().count() > 64 {
            return Err(DriveError::BadRequest("each tag must be 1-64 characters".into()));
        }
    }

    let pool = state.db_or_unavailable()?;
    let n = tags.len() as i64;

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT t.file_id \
         FROM drive_file_tags t \
         JOIN drive_files f ON f.id = t.file_id AND f.tenant_id = t.tenant_id \
         WHERE t.tenant_id = $1 \
           AND t.tag = ANY($2::text[]) \
           AND f.deleted_at IS NULL \
         GROUP BY t.file_id \
         HAVING COUNT(DISTINCT t.tag) = $3 \
         ORDER BY t.file_id",
    )
    .bind(ctx.tenant_id)
    .bind(&tags)
    .bind(n)
    .fetch_all(pool)
    .await?;

    let file_ids: Vec<Uuid> = rows.into_iter().map(|(id,)| id).collect();
    let file_count = file_ids.len() as i64;

    Ok(Json(IntersectResult { tags, file_ids, file_count }))
}

#[derive(Debug, Serialize)]
struct UnionResult {
    tags:         Vec<String>,
    file_ids:     Vec<Uuid>,
    file_count:   i64,
}

/// GET /api/v1/drive/tags/union?tags=a,b,c — retorna file_ids que possuem
/// **PELO MENOS UMA** das tags listadas (OR, sprint #467). Complementa o
/// intersect (#465, AND) e o `/tags/:tag` (uma tag só). Mesma normalização
/// (lowercase + trim + dedup, 1-32 tags, 1-64 chars cada) e filtro de files
/// ativos (deleted_at IS NULL). Implementação: `WHERE t.tag = ANY($2)` +
/// `GROUP BY t.file_id` (sem HAVING — qualquer match basta). Path estático
/// `/tags/union` precede `/tags/:tag` (lição #443/#448).
async fn union_files_by_tags(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<IntersectQuery>,
) -> Result<impl IntoResponse> {
    let mut tags: Vec<String> = q.tags
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    tags.sort();
    tags.dedup();

    if tags.is_empty() {
        return Err(DriveError::BadRequest("at least one tag required".into()));
    }
    if tags.len() > 32 {
        return Err(DriveError::BadRequest("max 32 tags per query".into()));
    }
    for t in &tags {
        if t.chars().count() > 64 {
            return Err(DriveError::BadRequest("each tag must be 1-64 characters".into()));
        }
    }

    let pool = state.db_or_unavailable()?;

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT t.file_id \
         FROM drive_file_tags t \
         JOIN drive_files f ON f.id = t.file_id AND f.tenant_id = t.tenant_id \
         WHERE t.tenant_id = $1 \
           AND t.tag = ANY($2::text[]) \
           AND f.deleted_at IS NULL \
         GROUP BY t.file_id \
         ORDER BY t.file_id",
    )
    .bind(ctx.tenant_id)
    .bind(&tags)
    .fetch_all(pool)
    .await?;

    let file_ids: Vec<Uuid> = rows.into_iter().map(|(id,)| id).collect();
    let file_count = file_ids.len() as i64;

    Ok(Json(UnionResult { tags, file_ids, file_count }))
}

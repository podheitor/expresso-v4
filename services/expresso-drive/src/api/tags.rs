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
//!   GET    /api/v1/drive/tags/stats-by-user         — contagem por (created_by, tag) (sprint #479)
//!   GET    /api/v1/drive/tags/co-occurrence         — top-N pares de tags juntas em mais files (sprint #484)
//!   GET    /api/v1/drive/tags/:tag/count           — contagem de files com uma tag específica (sprint #455)
//!   GET    /api/v1/drive/tags/intersect?tags=a,b,c — files que possuem TODAS as tags listadas (AND, sprint #465)
//!   GET    /api/v1/drive/tags/union?tags=a,b,c     — files que possuem PELO MENOS UMA das tags (OR, sprint #467)
//!   GET    /api/v1/drive/tags/rename-history       — audit trail de renames passados (sprint #470)
//!   POST   /api/v1/drive/tags/rename-history/:id/undo — reverte um rename anterior (sprint #472)
//!   GET    /api/v1/drive/tags/merge-history         — audit trail de merges (sprint #477)
//!   POST   /api/v1/drive/tags/merge-history/:id/undo — reverte um merge anterior (sprint #477)

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
            "/api/v1/drive/tags/stats-by-user",
            get(tag_stats_by_user),
        )
        .route(
            "/api/v1/drive/tags/co-occurrence",
            get(tag_co_occurrence),
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
        .route(
            "/api/v1/drive/tags/rename-history/:id/undo",
            post(undo_tag_rename),
        )
        .route(
            "/api/v1/drive/tags/merge-history",
            get(list_tag_merge_history),
        )
        .route(
            "/api/v1/drive/tags/merge-history/:id/undo",
            post(undo_tag_merge),
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

#[derive(Debug, Serialize, FromRow)]
struct TagStatByUser {
    created_by: Uuid,
    tag:        String,
    file_count: i64,
}

/// GET /api/v1/drive/tags/stats-by-user — contagem de files distintos por
/// `(created_by, tag)` no tenant (sprint #479). Variante do tag_stats (#448)
/// particionada pelo autor da tag (campo `drive_file_tags.created_by`). Útil
/// pra dashboards "quem está taggeando quem", auditoria de uso, ou pra UI
/// "tags que EU criei". Conta apenas files ativos (deleted_at IS NULL).
/// Ordena por `created_by`, depois count DESC, depois alfabético da tag.
/// Path com hífen evita colisão com `/:tag` (lição #443).
async fn tag_stats_by_user(
    State(state): State<AppState>,
    ctx:          RequestCtx,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    let stats: Vec<TagStatByUser> = sqlx::query_as(
        "SELECT t.created_by, t.tag, COUNT(DISTINCT t.file_id) AS file_count \
         FROM drive_file_tags t \
         JOIN drive_files f ON f.id = t.file_id AND f.tenant_id = t.tenant_id \
         WHERE t.tenant_id = $1 AND f.deleted_at IS NULL \
         GROUP BY t.created_by, t.tag \
         ORDER BY t.created_by, file_count DESC, t.tag ASC",
    )
    .bind(ctx.tenant_id)
    .fetch_all(pool)
    .await?;
    Ok(Json(stats))
}

#[derive(Debug, Deserialize)]
struct CoOccurrenceQuery {
    /// Top-N pares retornados, default 50, cap 1..500.
    limit:    Option<i64>,
    /// Filtra pares onde uma das tags == this (matching tag_a OR tag_b),
    /// lowercase. Útil pra "qual tag co-ocorre com X".
    tag:      Option<String>,
    /// Threshold mínimo de co-occurrence_count, default 1 (tudo).
    min_count: Option<i64>,
}

#[derive(Debug, Serialize, FromRow)]
struct TagCoOccurrence {
    tag_a:  String,
    tag_b:  String,
    /// Files distintos que carregam ambas tags.
    co_count: i64,
}

/// GET /api/v1/drive/tags/co-occurrence?limit=&tag=&min_count= — top-N pares
/// de tags que aparecem juntos em mais files (sprint #484). Self-join
/// `drive_file_tags` por file_id com `t1.tag < t2.tag` (lex order) pra evitar
/// pares duplicados (a,b)/(b,a) e auto-pares (a,a). Conta `COUNT(DISTINCT
/// t1.file_id)`. Filtra files ativos (deleted_at IS NULL). Filtros opcionais:
/// `tag` matching tag_a OR tag_b (lowercase), `min_count` threshold (default 1).
/// Útil pra "tag suggestions", grafo de relacionamento, ou descobrir clusters
/// orgânicos no taxonomy. Path estático precede `/:tag` (lição #443/#448);
/// hífen evita ambiguidade adicional. Default limit 50, cap 1..500.
async fn tag_co_occurrence(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<CoOccurrenceQuery>,
) -> Result<impl IntoResponse> {
    let limit     = q.limit.unwrap_or(50).clamp(1, 500);
    let min_count = q.min_count.unwrap_or(1).max(1);
    let tag_filter = q.tag.map(|t| t.trim().to_lowercase());

    let pool = state.db_or_unavailable()?;

    let rows: Vec<TagCoOccurrence> = sqlx::query_as(
        "SELECT t1.tag AS tag_a, t2.tag AS tag_b, \
                COUNT(DISTINCT t1.file_id) AS co_count \
           FROM drive_file_tags t1 \
           JOIN drive_file_tags t2 \
             ON t2.file_id = t1.file_id \
            AND t2.tenant_id = t1.tenant_id \
            AND t1.tag < t2.tag \
           JOIN drive_files f \
             ON f.id = t1.file_id AND f.tenant_id = t1.tenant_id \
          WHERE t1.tenant_id = $1 \
            AND f.deleted_at IS NULL \
            AND ($2::text IS NULL OR t1.tag = $2 OR t2.tag = $2) \
          GROUP BY t1.tag, t2.tag \
         HAVING COUNT(DISTINCT t1.file_id) >= $3 \
          ORDER BY co_count DESC, t1.tag ASC, t2.tag ASC \
          LIMIT $4",
    )
    .bind(ctx.tenant_id)
    .bind(tag_filter)
    .bind(min_count)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(Json(serde_json::json!({
        "limit":     limit,
        "min_count": min_count,
        "pairs":     rows,
    })))
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

/// POST /api/v1/drive/tags/:tag/merge — funde `:tag` em `into` (sprint #433),
/// consolidando todos os files do tag-fonte no tag-destino. Body: `{into: "..."}`.
/// Diferente de rename: ambas as tags podem existir; arquivos que já tinham `into`
/// têm o registro de `:tag` apagado (pré-DELETE evita unique conflict), e os
/// demais têm o tag UPDATEado para `into`. Idempotente. Retorna `{merged, into,
/// history_id}`.
///
/// Sprint #477: pré-DELETE + UPDATE + INSERT em drive_tag_merge_history são
/// agora atômicos via begin_tenant_tx (RLS); history grava `merged_file_ids`
/// (files que tinham só src — undo precisa reverter dst→src) e
/// `dropped_file_ids` (files que tinham ambos — undo precisa re-adicionar src),
/// habilitando undo preciso.
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
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    // Captura files que tinham AMBAS (src + dst) ANTES do pré-DELETE — esses
    // terão src dropado mas dst preservado; undo precisa re-adicionar src.
    let dropped_rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT DISTINCT t1.file_id FROM drive_file_tags t1 \
          JOIN drive_file_tags t2 \
            ON t2.file_id = t1.file_id AND t2.tenant_id = t1.tenant_id \
          WHERE t1.tenant_id = $1 AND t1.tag = $2 AND t2.tag = $3",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .bind(&dst)
    .fetch_all(&mut *tx)
    .await?;
    let dropped_file_ids: Vec<Uuid> = dropped_rows.into_iter().map(|(id,)| id).collect();

    // Pré-DELETE dos files que já tinham `dst` pra liberar a chave única.
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
    .execute(&mut *tx)
    .await?;

    // Captura os files restantes com src (esses serão UPDATEados src→dst);
    // undo precisa reverter dst→src + re-add não, dst sai por update.
    let merged_rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT file_id FROM drive_file_tags WHERE tenant_id = $1 AND tag = $2",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .fetch_all(&mut *tx)
    .await?;
    let merged_file_ids: Vec<Uuid> = merged_rows.into_iter().map(|(id,)| id).collect();

    let r = sqlx::query(
        "UPDATE drive_file_tags SET tag = $2 \
         WHERE tenant_id = $1 AND tag = $3",
    )
    .bind(ctx.tenant_id)
    .bind(&dst)
    .bind(&src)
    .execute(&mut *tx)
    .await?;

    let merged = r.rows_affected();

    let history_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO drive_tag_merge_history \
            (tenant_id, src_tag, dst_tag, merged_count, merged_file_ids, dropped_file_ids, merged_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(&src)
    .bind(&dst)
    .bind(merged as i64)
    .bind(&merged_file_ids)
    .bind(&dropped_file_ids)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "merged":     merged,
        "into":       dst,
        "history_id": history_id.0,
    })))
}

#[derive(Debug, Serialize, FromRow)]
struct TagMergeHistoryEntry {
    id:                Uuid,
    src_tag:           String,
    dst_tag:           String,
    merged_count:      i64,
    merged_by:         Uuid,
    #[serde(with = "time::serde::rfc3339")]
    merged_at:         OffsetDateTime,
}

#[derive(Debug, Deserialize)]
struct TagMergeHistoryQuery {
    limit:  Option<i64>,
    since:  Option<OffsetDateTime>,
    before: Option<OffsetDateTime>,
    tag:    Option<String>,
}

/// GET /api/v1/drive/tags/merge-history?limit=&since=&before=&tag= — audit
/// trail dos merges passados no tenant (sprint #477). Filtros opcionais: range
/// temporal e `tag` matching src OR dst (lowercase). Limit padrão 50, cap
/// 1..500. Path estático precede `/:tag` (lição #443/#448).
async fn list_tag_merge_history(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Query(q):     Query<TagMergeHistoryQuery>,
) -> Result<impl IntoResponse> {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let tag_filter = q.tag.map(|t| t.trim().to_lowercase());

    let pool = state.db_or_unavailable()?;

    let entries: Vec<TagMergeHistoryEntry> = sqlx::query_as(
        "SELECT id, src_tag, dst_tag, merged_count, merged_by, merged_at \
           FROM drive_tag_merge_history \
          WHERE tenant_id = $1 \
            AND ($2::timestamptz IS NULL OR merged_at >= $2) \
            AND ($3::timestamptz IS NULL OR merged_at <  $3) \
            AND ($4::text IS NULL OR src_tag = $4 OR dst_tag = $4) \
          ORDER BY merged_at DESC \
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

/// POST /api/v1/drive/tags/merge-history/:id/undo — desfaz merge pelo id da
/// history (sprint #477). Lê `merged_file_ids` (UPDATEados src→dst — undo
/// reverte dst→src) e `dropped_file_ids` (tinham ambos, src foi DELETEado —
/// undo re-adiciona src). Tudo em tx via `begin_tenant_tx`. 404 se id não
/// existir. Idempotente: se files mudaram tags depois, partes podem virar
/// no-op mas history grava entrada do undo de qualquer jeito. Retorna
/// `{undone_id, src_tag, dst_tag, restored_merged, restored_dropped}`.
async fn undo_tag_merge(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let entry: Option<(String, String, Vec<Uuid>, Vec<Uuid>)> = sqlx::query_as(
        "SELECT src_tag, dst_tag, merged_file_ids, dropped_file_ids \
           FROM drive_tag_merge_history \
          WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (src, dst, merged_ids, dropped_ids) = entry.ok_or(DriveError::NotFound(id))?;

    // 1) Reverter merged_file_ids: esses só tinham src antes; merge fez
    //    src→dst. Undo: INSERT src + DELETE dst (os files não devem ter dst).
    //    Usar INSERT ... ON CONFLICT DO NOTHING pra idempotência.
    let restored_merged = if !merged_ids.is_empty() {
        sqlx::query(
            "INSERT INTO drive_file_tags (tenant_id, file_id, tag, created_by) \
             SELECT $1, file_id, $2, $3 FROM UNNEST($4::uuid[]) AS t(file_id) \
             ON CONFLICT DO NOTHING",
        )
        .bind(ctx.tenant_id)
        .bind(&src)
        .bind(ctx.user_id)
        .bind(&merged_ids)
        .execute(&mut *tx)
        .await?;
        let r = sqlx::query(
            "DELETE FROM drive_file_tags \
              WHERE tenant_id = $1 AND tag = $2 AND file_id = ANY($3)",
        )
        .bind(ctx.tenant_id)
        .bind(&dst)
        .bind(&merged_ids)
        .execute(&mut *tx)
        .await?;
        r.rows_affected()
    } else {
        0
    };

    // 2) Reverter dropped_file_ids: esses tinham AMBOS antes; merge dropou só
    //    src. Undo: re-INSERT src; dst permanece intocado nesses files.
    let restored_dropped = if !dropped_ids.is_empty() {
        let r = sqlx::query(
            "INSERT INTO drive_file_tags (tenant_id, file_id, tag, created_by) \
             SELECT $1, file_id, $2, $3 FROM UNNEST($4::uuid[]) AS t(file_id) \
             ON CONFLICT DO NOTHING",
        )
        .bind(ctx.tenant_id)
        .bind(&src)
        .bind(ctx.user_id)
        .bind(&dropped_ids)
        .execute(&mut *tx)
        .await?;
        r.rows_affected()
    } else {
        0
    };

    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "undone_id":        id,
        "src_tag":          src,
        "dst_tag":          dst,
        "restored_merged":  restored_merged,
        "restored_dropped": restored_dropped,
    })))
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

#[derive(Debug, Serialize)]
struct UndoTagRenameResult {
    undone_id:    Uuid,
    reverted:     u64,
    old_tag:      String,
    new_tag:      String,
    history_id:   Uuid,
}

/// POST /api/v1/drive/tags/rename-history/:id/undo — reverte um rename anterior
/// (sprint #472). Lê a entry `:id` em drive_tag_rename_history e aplica rename
/// reverso (`new_tag` → `old_tag`) em todos os files que ainda estão com
/// `new_tag`. Tudo numa única transação via `begin_tenant_tx` (RLS) — pré-DELETE
/// de conflitos + UPDATE + INSERT de novo registro de history (com `old_tag` e
/// `new_tag` invertidos) são atômicos. O novo registro permite "undo do undo".
/// 404 se a entry não existir no tenant. Idempotente em files: se nenhum file
/// estiver mais com `new_tag` (porque foi renomeado de novo), `reverted: 0` mas
/// a entry de undo é gravada mesmo assim para audit trail.
async fn undo_tag_rename(
    State(state): State<AppState>,
    ctx:          RequestCtx,
    Path(id):     Path<Uuid>,
) -> Result<impl IntoResponse> {
    let pool = state.db_or_unavailable()?;
    let mut tx = begin_tenant_tx(pool, ctx.tenant_id).await?;

    let entry: Option<(String, String)> = sqlx::query_as(
        "SELECT old_tag, new_tag FROM drive_tag_rename_history \
         WHERE id = $1 AND tenant_id = $2",
    )
    .bind(id)
    .bind(ctx.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;

    let (orig_old, orig_new) = entry.ok_or(DriveError::NotFound(id))?;

    // Reverso: queremos voltar new_tag para old_tag.
    let from = orig_new;
    let to   = orig_old;

    let _ = sqlx::query(
        "DELETE FROM drive_file_tags \
         WHERE tenant_id = $1 AND tag = $2 \
           AND file_id IN ( \
               SELECT file_id FROM drive_file_tags \
               WHERE tenant_id = $1 AND tag = $3 \
           )",
    )
    .bind(ctx.tenant_id)
    .bind(&to)
    .bind(&from)
    .execute(&mut *tx)
    .await?;

    let r = sqlx::query(
        "UPDATE drive_file_tags SET tag = $2 \
         WHERE tenant_id = $1 AND tag = $3",
    )
    .bind(ctx.tenant_id)
    .bind(&to)
    .bind(&from)
    .execute(&mut *tx)
    .await?;

    let reverted = r.rows_affected();

    let new_history_id: (Uuid,) = sqlx::query_as(
        "INSERT INTO drive_tag_rename_history \
            (tenant_id, old_tag, new_tag, renamed_count, renamed_by) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id",
    )
    .bind(ctx.tenant_id)
    .bind(&from)
    .bind(&to)
    .bind(reverted as i64)
    .bind(ctx.user_id)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(UndoTagRenameResult {
        undone_id:  id,
        reverted,
        old_tag:    from,
        new_tag:    to,
        history_id: new_history_id.0,
    }))
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

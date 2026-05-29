//! REST API handlers for search service.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::index_store::{IndexDoc, IndexStore, SearchHit};

/// Limites duros pro endpoint de busca.
///
/// `q` 1 KiB cobre query realista (Google caps ~2 KiB; usuários reais
/// mandam <100 chars). Acima disso o tantivy `QueryParser` pode passar
/// minutos compilando expressões com milhares de termos.
///
/// `limit` 200 cobre paginação razoável de UI; `TopDocs::with_limit(N)`
/// aloca um heap de tamanho N upfront — sem cap, `?limit=usize::MAX`
/// é OOM imediato.
pub const MAX_QUERY_BYTES: usize = 1024;
pub const MAX_LIMIT:       usize = 200;
pub const DEFAULT_LIMIT:   usize = 20;

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub tenant_id: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
    /// "kind" → `facets.kind`; "from_addr" → `facets.from_addr` (top-N remetentes, sprint #426);
    /// "subject_terms" → `facets.subject_terms` (top-N palavras-chave em subjects, sprint #431);
    /// "kind_x_from" → `facets.kind_x_from` (top-N pares "kind|from_addr", sprint #436).
    /// "domain" → `facets.domain` (top-N domínios after-@ do from_addr, sprint #441).
    /// "domain_x_kind" → `facets.domain_x_kind` (top-N pares "domain|kind", sprint #446).
    /// "received_at" → `facets.received_at` (bucket counts by granularity, sprint #564).
    pub facet: Option<String>,
    /// Temporal facet granularity: "day" (default), "week", "month". Only used when facet=received_at.
    pub facet_granularity: Option<String>,
    /// Filter: only return docs with received_at >= after_secs (unix seconds).
    pub after_secs: Option<u64>,
    /// Filter: only return docs with received_at < before_secs (unix seconds).
    pub before_secs: Option<u64>,
}

/// Cap on top-N entries returned for high-cardinality facets like from_addr.
/// 50 cobre uma sidebar de "top remetentes" sem explodir o payload.
pub const FACET_FROM_TOP_N: usize = 50;

/// Cap pra tag-cloud de palavras em subjects. 50 é tamanho típico de cloud
/// renderizado sem virar parede de texto.
pub const FACET_SUBJECT_TOP_N: usize = 50;

/// Cap pra cross-facet kind×from_addr — produto cartesiano pode crescer
/// rápido (ex.: 5 kinds × 200 remetentes), 100 cobre matriz densa sem
/// estourar payload.
pub const FACET_KIND_X_FROM_TOP_N: usize = 100;

/// Cap pra facet de domínio. Domínios costumam ter ordem-de-magnitude
/// menor que remetentes individuais (gmail.com, outlook.com, etc.), 50
/// cobre cauda longa.
pub const FACET_DOMAIN_TOP_N: usize = 50;

/// Cap pra cross-facet domain×kind. Mesmo critério do kind_x_from (#436).
pub const FACET_DOMAIN_X_KIND_TOP_N: usize = 100;

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

/// Valida + clamp dos parâmetros antes de bater no índice. Retorna
/// `Some(BAD_REQUEST_msg)` quando rejeita; `None` quando OK (com
/// `params.limit` clampado em-place).
fn validate_search_params(params: &mut SearchParams) -> Option<String> {
    if params.q.len() > MAX_QUERY_BYTES {
        return Some(format!(
            "query too large: {} bytes (max {})",
            params.q.len(), MAX_QUERY_BYTES
        ));
    }
    // limit=0 é nonsense (sem hits) mas não-perigoso; melhor 400 explícito.
    if params.limit == 0 {
        return Some("limit must be >= 1".into());
    }
    // Clamp em vez de rejeitar — operador via UI passa limites altos por
    // engano sem maldade. O cap protege a memória do índice.
    if params.limit > MAX_LIMIT {
        params.limit = MAX_LIMIT;
    }
    None
}

#[derive(Debug, Serialize)]
pub struct FacetEntry {
    pub value: String,
    pub count: u64,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<std::collections::HashMap<String, Vec<FacetEntry>>>,
}

/// POST /api/v1/index — index a document
pub async fn index_doc(
    State(store): State<IndexStore>,
    Json(doc): Json<IndexDoc>,
) -> Result<StatusCode, (StatusCode, String)> {
    store
        .index_document(&doc)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::CREATED)
}

pub const MAX_BULK_DOCS: usize = 500;

#[derive(Debug, Deserialize)]
pub struct BulkIndexRequest {
    pub documents: Vec<IndexDoc>,
}

#[derive(Debug, Serialize)]
pub struct BulkIndexResponse {
    pub indexed: usize,
    pub rejected: Vec<String>,
}

/// POST /api/v1/index/bulk — index multiple documents in one request
pub async fn bulk_index(
    State(store): State<IndexStore>,
    Json(body): Json<BulkIndexRequest>,
) -> Result<Json<BulkIndexResponse>, (StatusCode, String)> {
    if body.documents.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "documents must not be empty".into()));
    }
    if body.documents.len() > MAX_BULK_DOCS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("too many documents: {} (max {})", body.documents.len(), MAX_BULK_DOCS),
        ));
    }
    let total = body.documents.len();
    let rejected = store
        .index_documents(&body.documents)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let indexed = total - rejected.len();
    Ok(Json(BulkIndexResponse { indexed, rejected }))
}

fn map_search_err(e: anyhow::Error) -> (StatusCode, String) {
    if e.to_string().starts_with("bad_query:") {
        (StatusCode::BAD_REQUEST, "invalid query syntax".to_string())
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "search failed".to_string())
    }
}

/// GET /api/v1/search?q=...&tenant_id=...&limit=20&offset=0[&facet=kind][&after_secs=&before_secs=]
pub async fn search(
    State(store): State<IndexStore>,
    Query(mut params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, (StatusCode, String)> {
    if let Some(msg) = validate_search_params(&mut params) {
        return Err((StatusCode::BAD_REQUEST, msg));
    }

    // Use temporal-range search path when any received_at filter is active.
    let hits = if params.after_secs.is_some() || params.before_secs.is_some() {
        store
            .search_with_received_at_filter(
                &params.q,
                &params.tenant_id,
                params.limit,
                params.offset,
                params.after_secs,
                params.before_secs,
            )
            .map_err(map_search_err)?
    } else {
        store
            .search(&params.q, &params.tenant_id, params.limit, params.offset)
            .map_err(map_search_err)?
    };
    let count = hits.len();

    let facets = match params.facet.as_deref() {
        Some("received_at") => {
            let granularity = params.facet_granularity.as_deref().unwrap_or("day");
            if !matches!(granularity, "day" | "week" | "month") {
                return Err((StatusCode::BAD_REQUEST, "facet_granularity must be day, week, or month".into()));
            }
            let buckets = store
                .facet_received_at_buckets(&params.q, &params.tenant_id, granularity)
                .map_err(map_search_err)?;
            let entries: Vec<FacetEntry> = buckets.into_iter()
                .map(|(value, count)| FacetEntry { value, count })
                .collect();
            let mut map = std::collections::HashMap::new();
            map.insert("received_at".to_string(), entries);
            Some(map)
        }
        Some("kind") => {
            let counts = store
                .facet_counts_by_kind(&params.q, &params.tenant_id)
                .map_err(map_search_err)?;
            let entries: Vec<FacetEntry> = counts.into_iter()
                .map(|(value, count)| FacetEntry { value, count })
                .collect();
            let mut map = std::collections::HashMap::new();
            map.insert("kind".to_string(), entries);
            Some(map)
        }
        Some("from_addr") => {
            let counts = store
                .facet_counts_by_from(&params.q, &params.tenant_id, FACET_FROM_TOP_N)
                .map_err(map_search_err)?;
            let entries: Vec<FacetEntry> = counts.into_iter()
                .map(|(value, count)| FacetEntry { value, count })
                .collect();
            let mut map = std::collections::HashMap::new();
            map.insert("from_addr".to_string(), entries);
            Some(map)
        }
        Some("subject_terms") => {
            let counts = store
                .facet_top_subject_terms(&params.q, &params.tenant_id, FACET_SUBJECT_TOP_N)
                .map_err(map_search_err)?;
            let entries: Vec<FacetEntry> = counts.into_iter()
                .map(|(value, count)| FacetEntry { value, count })
                .collect();
            let mut map = std::collections::HashMap::new();
            map.insert("subject_terms".to_string(), entries);
            Some(map)
        }
        Some("domain") => {
            let counts = store
                .facet_counts_by_domain(&params.q, &params.tenant_id, FACET_DOMAIN_TOP_N)
                .map_err(map_search_err)?;
            let entries: Vec<FacetEntry> = counts.into_iter()
                .map(|(value, count)| FacetEntry { value, count })
                .collect();
            let mut map = std::collections::HashMap::new();
            map.insert("domain".to_string(), entries);
            Some(map)
        }
        Some("domain_x_kind") => {
            let triples = store
                .facet_domain_by_kind(&params.q, &params.tenant_id, FACET_DOMAIN_X_KIND_TOP_N)
                .map_err(map_search_err)?;
            let entries: Vec<FacetEntry> = triples.into_iter()
                .map(|(domain, kind, count)| FacetEntry {
                    value: format!("{}|{}", domain, kind),
                    count,
                })
                .collect();
            let mut map = std::collections::HashMap::new();
            map.insert("domain_x_kind".to_string(), entries);
            Some(map)
        }
        Some("kind_x_from") => {
            let triples = store
                .facet_kind_by_from(&params.q, &params.tenant_id, FACET_KIND_X_FROM_TOP_N)
                .map_err(map_search_err)?;
            // value = "kind|from_addr" — cliente faz split('|', 1) pra recuperar.
            let entries: Vec<FacetEntry> = triples.into_iter()
                .map(|(kind, from, count)| FacetEntry {
                    value: format!("{}|{}", kind, from),
                    count,
                })
                .collect();
            let mut map = std::collections::HashMap::new();
            map.insert("kind_x_from".to_string(), entries);
            Some(map)
        }
        _ => None,
    };

    Ok(Json(SearchResponse { hits, count, facets }))
}

/// DELETE /api/v1/index/:id — remove document from index
pub async fn delete_doc(
    State(store): State<IndexStore>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    store
        .delete_document(&id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct DeleteByTenantParams {
    pub tenant_id: String,
}

/// DELETE /api/v1/index?tenant_id= — remove all documents for a tenant (query-param form)
pub async fn delete_by_tenant(
    State(store): State<IndexStore>,
    Query(params): Query<DeleteByTenantParams>,
) -> Result<StatusCode, (StatusCode, String)> {
    if params.tenant_id.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "tenant_id is required".into()));
    }
    store
        .delete_tenant_documents(&params.tenant_id)
        .await
        .map_err(|e| {
            if e.to_string().contains("valid UUID") {
                (StatusCode::BAD_REQUEST, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/v1/search/index/tenant/:tenant_id — purge all documents for a tenant.
///
/// Path-param form do delete_by_tenant — mais RESTful, sem query string.
/// Retorna `{tenant_id, status: "purged"}` em vez de 204 para melhor observability.
/// Sprint #605.
pub async fn purge_tenant_index(
    State(store): State<IndexStore>,
    Path(tenant_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    store
        .delete_tenant_documents(&tenant_id)
        .await
        .map_err(|e| {
            if e.to_string().contains("valid UUID") {
                (StatusCode::BAD_REQUEST, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;
    Ok(Json(serde_json::json!({
        "tenant_id": tenant_id,
        "status":    "purged",
    })))
}

#[derive(Debug, Deserialize)]
pub struct StatsParams {
    pub tenant_id: String,
}

/// POST /api/v1/search/index/segments/merge — força merge de todos os segmentos Tantivy.
///
/// Útil pra reduzir fragmentação após ingestão em lote. Bloqueia até o merge completar.
/// Retorna `{merged_from, status}` onde status é "merged" ou "already_merged".
/// Sprint #595.
pub async fn merge_segments(
    State(store): State<IndexStore>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (merged_from, status) = store
        .force_merge_segments()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "merged_from": merged_from,
        "status":      status,
    })))
}

/// POST /api/v1/search/index/vacuum — garbage-collect unreferenced segment files.
///
/// Remove arquivos de segmento obsoletos (tombstones, merges antigos) do disco.
/// Análogo a VACUUM no Postgres — não afeta queries em andamento.
/// Retorna `{deleted_files}` com número de arquivos removidos. Sprint #603.
pub async fn vacuum_index(
    State(store): State<IndexStore>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let deleted = store
        .vacuum()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "deleted_files": deleted,
    })))
}

/// GET /api/v1/search/health/index — Tantivy index health info (ops, tenant-agnostic).
///
/// Retorna `{num_docs, num_segments, disk_bytes}`.
/// `num_docs` = total de documentos no índice (todos os tenants).
/// `num_segments` = número de segmentos Tantivy (indica fragmentação; writer commit cria
/// novos segmentos; merger os consolida). `disk_bytes` = soma dos tamanhos dos arquivos
/// gerenciados pelo índice. Sprint #591.
pub async fn index_health(
    State(store): State<IndexStore>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (num_docs, num_segments, disk_bytes) = store
        .index_health()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "num_docs":     num_docs,
        "num_segments": num_segments,
        "disk_bytes":   disk_bytes,
    })))
}

/// GET /api/v1/search/index/segments — list active index segments with metadata.
///
/// Returns `{count, segments: [{id, num_docs, disk_bytes}]}` for each segment
/// visible to the current index reader. Useful for observability: spot segment
/// explosion (many small segments → slow searches → trigger merge), confirm that
/// vacuum removed unreferenced files, and see per-segment doc counts.
/// Tenant-agnostic (ops endpoint). Sprint #613.
pub async fn list_segments(
    State(store): State<IndexStore>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let segs = store
        .list_segments()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let count = segs.len();
    let segments: Vec<serde_json::Value> = segs
        .into_iter()
        .map(|(id, num_docs, disk_bytes)| serde_json::json!({
            "id":         id,
            "num_docs":   num_docs,
            "disk_bytes": disk_bytes,
        }))
        .collect();

    Ok(Json(serde_json::json!({
        "count":    count,
        "segments": segments,
    })))
}

/// GET /api/v1/search/index/segments/count — current segment count.
///
/// Returns `{count}` without transmitting the full segment list. Useful for
/// badge/alert UIs that only need to know how many segments exist (e.g. "index
/// needs vacuum if count > N"). Complements `GET /index/segments` (#613).
/// Sprint #638.
pub async fn segment_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let count = store
        .list_segments()
        .map(|s| s.len())
        .unwrap_or(0);
    Json(serde_json::json!({"count": count}))
}

/// GET /api/v1/search/index/segments/:id — metadata for a single segment by UUID.
///
/// Returns `{id, num_docs, disk_bytes}` for the segment visible to the current
/// reader. 404 if no segment with that ID exists (may have been merged/vacuumed).
/// Complements `GET /index/segments` (list all) with single-segment lookup.
/// Sprint #618.
pub async fn get_segment(
    State(store): State<IndexStore>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let seg = store
        .get_segment(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    match seg {
        Some((sid, num_docs, disk_bytes)) => Ok(Json(serde_json::json!({
            "id":         sid,
            "num_docs":   num_docs,
            "disk_bytes": disk_bytes,
        }))),
        None => Err((
            StatusCode::NOT_FOUND,
            format!("segment '{id}' not found"),
        )),
    }
}

/// POST /api/v1/search/index/segments/reload — force Tantivy reader reload.
///
/// The reader auto-reloads after each writer commit via `OnCommitWithDelay`, but
/// this endpoint triggers an immediate synchronous reload — useful when you want
/// freshly indexed documents to be visible without waiting for the delay window.
/// Returns `{status: "reloaded"}`. Idempotent — safe to call repeatedly.
/// Sprint #623.
pub async fn reload_index(
    State(store): State<IndexStore>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    store
        .reload()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "status": "reloaded",
    })))
}

/// GET /api/v1/search/stats?tenant_id= — aggregate doc counts for a tenant.
///
/// Returns `{tenant_id, total, by_kind: [{kind, count}]}` ordered by count DESC.
/// 400 if `tenant_id` is not a valid UUID. Sprint #584.
pub async fn search_stats(
    State(store): State<IndexStore>,
    Query(params): Query<StatsParams>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if params.tenant_id.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "tenant_id is required".into()));
    }
    let (total, by_kind) = store
        .doc_stats(&params.tenant_id)
        .map_err(|e| {
            if e.to_string().contains("valid UUID") {
                (StatusCode::BAD_REQUEST, e.to_string())
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        })?;

    let kinds: Vec<serde_json::Value> = by_kind
        .into_iter()
        .map(|(kind, count)| serde_json::json!({"kind": kind, "count": count}))
        .collect();

    Ok(Json(serde_json::json!({
        "tenant_id": params.tenant_id,
        "total":     total,
        "by_kind":   kinds,
    })))
}

/// GET /api/v1/search/index/health/detailed — consolidated index health.
///
/// Combines index_health (#591) + writer_stats (#628) into a single response:
/// `{status, num_docs, num_segments, disk_bytes, writer_busy, heap_budget_bytes}`.
/// `status` is "ok" always — endpoint exists to surface metrics, not circuit-break.
/// Useful as a single-call "how is the index?" dashboard source. Sprint #633.
pub async fn index_health_detailed(
    State(store): State<IndexStore>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (num_docs, num_segments, disk_bytes) = store
        .index_health()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (heap_budget_bytes, writer_busy, _, _) = store.writer_stats();

    Ok(Json(serde_json::json!({
        "status":            "ok",
        "num_docs":          num_docs,
        "num_segments":      num_segments,
        "disk_bytes":        disk_bytes,
        "writer_busy":       writer_busy,
        "heap_budget_bytes": heap_budget_bytes,
    })))
}

/// GET /api/v1/search/index/segments/largest — segment with the highest disk_bytes.
///
/// Returns `{segment}` with `{id, num_docs, disk_bytes}` for the single largest
/// segment by `disk_bytes`, or `{segment: null}` when the index is empty.
/// Useful for spotting bloat after bulk indexing without sorting the full segment
/// list client-side. Complements `GET /index/segments` (#613). Sprint #649.
pub async fn largest_segment(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segment = store
        .list_segments()
        .ok()
        .and_then(|mut segs| {
            segs.sort_unstable_by(|a, b| b.2.cmp(&a.2));
            segs.into_iter().next()
        })
        .map(|(id, num_docs, disk_bytes)| serde_json::json!({
            "id":         id,
            "num_docs":   num_docs,
            "disk_bytes": disk_bytes,
        }));
    Json(serde_json::json!({"segment": segment}))
}

/// GET /api/v1/search/index/segments/smallest — segment with the lowest disk_bytes.
///
/// Symmetric with `/segments/largest` (#649): sorts ASC by `disk_bytes` and returns
/// the first entry. Returns `{segment: null}` when the index is empty.
/// Useful for fragmentation signals: when `smallest` is orders of magnitude smaller
/// than `largest`, the index has many tiny segments and a merge may help.
/// Sprint #654.
pub async fn smallest_segment(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segment = store
        .list_segments()
        .ok()
        .and_then(|mut segs| {
            segs.sort_unstable_by(|a, b| a.2.cmp(&b.2));
            segs.into_iter().next()
        })
        .map(|(id, num_docs, disk_bytes)| serde_json::json!({
            "id":         id,
            "num_docs":   num_docs,
            "disk_bytes": disk_bytes,
        }));
    Json(serde_json::json!({"segment": segment}))
}

/// GET /api/v1/search/index/segments/stats — consolidated segment stats.
///
/// Combina `count` + `largest` + `smallest` + `total_disk_bytes` numa única chamada,
/// evitando 4 requests separados no dashboard. Retorna:
/// `{segment_count, total_disk_bytes, largest_disk_bytes, smallest_disk_bytes}`.
/// `largest_disk_bytes` e `smallest_disk_bytes` são `null` quando o índice está vazio.
/// Graceful: retorna zeros/nulls se `list_segments()` falhar. Sprint #659.
pub async fn segment_stats(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let (segment_count, total_disk_bytes, largest_disk_bytes, smallest_disk_bytes) = store
        .list_segments()
        .map(|segs| {
            if segs.is_empty() {
                return (0usize, 0u64, None::<u64>, None::<u64>);
            }
            let total   = segs.iter().map(|(_, _, db)| db).sum::<u64>();
            let largest  = segs.iter().map(|(_, _, db)| *db).max();
            let smallest = segs.iter().map(|(_, _, db)| *db).min();
            (segs.len(), total, largest, smallest)
        })
        .unwrap_or((0, 0, None, None));

    Json(serde_json::json!({
        "segment_count":       segment_count,
        "total_disk_bytes":    total_disk_bytes,
        "largest_disk_bytes":  largest_disk_bytes,
        "smallest_disk_bytes": smallest_disk_bytes,
    }))
}

/// GET /api/v1/search/index/segments/age-stats — num_docs distribution across segments.
///
/// Retorna `{segment_count, min_docs, max_docs, avg_docs}` com a distribuição de
/// `num_docs` por segmento. `min_docs`, `max_docs` e `avg_docs` são `null` quando o
/// índice está vazio. Graceful: zeros/nulls se `list_segments()` falhar. Sprint #664.
pub async fn segment_age_stats(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let (segment_count, min_docs, max_docs, avg_docs) = store
        .list_segments()
        .map(|segs| {
            if segs.is_empty() {
                return (0usize, None::<u64>, None::<u64>, None::<f64>);
            }
            let docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
            let min  = docs.iter().copied().min();
            let max  = docs.iter().copied().max();
            let avg  = Some(docs.iter().sum::<u64>() as f64 / docs.len() as f64);
            (segs.len(), min, max, avg)
        })
        .unwrap_or((0, None, None, None));

    Json(serde_json::json!({
        "segment_count": segment_count,
        "min_docs":      min_docs,
        "max_docs":      max_docs,
        "avg_docs":      avg_docs,
    }))
}

/// GET /api/v1/search/index/segments/doc-distribution — histograma de num_docs por faixa.
///
/// Classifica cada segmento em 4 faixas: `tiny` (0–100), `small` (101–1 000),
/// `medium` (1 001–10 000), `large` (>10 000). Retorna
/// `{segment_count, buckets:[{range,count}]}`. Graceful: zeros se `list_segments()` falhar.
/// Sprint #669.
pub async fn segment_doc_distribution(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();

    let mut tiny   = 0u64;
    let mut small  = 0u64;
    let mut medium = 0u64;
    let mut large  = 0u64;

    for (_, num_docs, _) in &segs {
        match num_docs {
            0..=100       => tiny   += 1,
            101..=1_000   => small  += 1,
            1_001..=10_000 => medium += 1,
            _             => large  += 1,
        }
    }

    Json(serde_json::json!({
        "segment_count": segs.len(),
        "buckets": [
            {"range": "0-100",       "count": tiny},
            {"range": "101-1000",    "count": small},
            {"range": "1001-10000",  "count": medium},
            {"range": ">10000",      "count": large},
        ],
    }))
}

/// GET /api/v1/search/index/segments/top-n?limit=N — top-N segmentos por disk_bytes.
///
/// Generaliza `/segments/largest` (#565) para N resultados. Ordena DESC por `disk_bytes`
/// e retorna `{segments:[{id,num_docs,disk_bytes}]}`. `limit` default 5 max 50.
/// Graceful: lista vazia se `list_segments()` falhar. Sprint #674.
pub async fn segments_top_n(
    State(store): State<IndexStore>,
    Query(q):     Query<TopNQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(5).clamp(1, 50) as usize;

    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_unstable_by(|a, b| b.2.cmp(&a.2));
    segs.truncate(limit);

    let segments: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, num_docs, disk_bytes)| serde_json::json!({
            "id":        id,
            "num_docs":  num_docs,
            "disk_bytes": disk_bytes,
        }))
        .collect();

    Json(serde_json::json!({"segments": segments}))
}

#[derive(Debug, serde::Deserialize)]
struct TopNQuery {
    limit: Option<u64>,
}

/// GET /api/v1/search/index/segments/bottom-n?limit=N — bottom-N segmentos por disk_bytes.
///
/// Candidatos a merge — os menores segmentos em disco. Simetria com top-n (#674):
/// sort ASC por `disk_bytes` + truncate. `limit` default 5 max 50.
/// Graceful: lista vazia se `list_segments()` falhar. Sprint #679.
pub async fn segments_bottom_n(
    State(store): State<IndexStore>,
    Query(q):     Query<TopNQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(5).clamp(1, 50) as usize;

    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_unstable_by(|a, b| a.2.cmp(&b.2));
    segs.truncate(limit);

    let segments: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, num_docs, disk_bytes)| serde_json::json!({
            "id":         id,
            "num_docs":   num_docs,
            "disk_bytes": disk_bytes,
        }))
        .collect();

    Json(serde_json::json!({"segments": segments}))
}

/// GET /api/v1/search/index/disk-usage — total disk bytes used by index segments.
///
/// Aggregates `disk_bytes` across all segments returned by `list_segments()`.
/// Returns `{total_disk_bytes, segment_count}`. Complements `GET /index/health`
/// (which also exposes `disk_bytes` as a side-effect) with a dedicated,
/// self-descriptive ops endpoint for storage dashboards. Graceful degradation:
/// returns zeros when `list_segments()` fails. Sprint #644.
pub async fn index_disk_usage(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let (total_disk_bytes, segment_count) = store
        .list_segments()
        .map(|segs| {
            let total = segs.iter().map(|(_, _, db)| db).sum::<u64>();
            (total, segs.len())
        })
        .unwrap_or((0, 0));
    Json(serde_json::json!({
        "total_disk_bytes": total_disk_bytes,
        "segment_count":    segment_count,
    }))
}

/// GET /api/v1/search/index/writer/stats — writer-level observability.
///
/// Returns: `heap_budget_bytes` (static 50 MB allocated to the writer),
/// `writer_busy` (true if writer mutex is currently held by another task),
/// `num_docs_committed` (live docs in current reader snapshot),
/// `num_segments_committed` (segments in current reader snapshot).
/// Does NOT block on the writer lock — safe to call under load. Sprint #628.
pub async fn writer_stats(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let (heap_budget_bytes, writer_busy, num_docs, num_segments) = store.writer_stats();
    Json(serde_json::json!({
        "heap_budget_bytes":      heap_budget_bytes,
        "writer_busy":            writer_busy,
        "num_docs_committed":     num_docs,
        "num_segments_committed": num_segments,
    }))
}

/// GET /api/v1/search/index/segments/size-stats — distribuição de disk_bytes por segmento.
///
/// Retorna `{segment_count, min_disk_bytes, max_disk_bytes, avg_disk_bytes}` com
/// `null` quando o índice está vazio. Análogo a `age-stats` (#664) mas para bytes
/// em vez de num_docs. Útil pra medir dispersão de tamanho de segmentos. Sprint #694.
pub async fn segment_size_stats(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let (segment_count, min_disk_bytes, max_disk_bytes, avg_disk_bytes) = store
        .list_segments()
        .map(|segs| {
            if segs.is_empty() {
                return (0usize, None::<u64>, None::<u64>, None::<f64>);
            }
            let bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
            let min = bytes.iter().copied().min();
            let max = bytes.iter().copied().max();
            let avg = Some(bytes.iter().sum::<u64>() as f64 / bytes.len() as f64);
            (segs.len(), min, max, avg)
        })
        .unwrap_or((0, None, None, None));

    Json(serde_json::json!({
        "segment_count":    segment_count,
        "min_disk_bytes":   min_disk_bytes,
        "max_disk_bytes":   max_disk_bytes,
        "avg_disk_bytes":   avg_disk_bytes,
    }))
}

/// GET /api/v1/search/index/segments/merge-candidates?min_docs=N&max_docs=N
///
/// Filtra segmentos por faixa de `num_docs`: retorna apenas aqueles com
/// `num_docs >= min_docs` (default 0) e `num_docs <= max_docs` (default u64::MAX).
/// Ordenados por `num_docs ASC`. Útil pra automação de merge seletivo — identifica
/// segmentos pequenos acima de um threshold mínimo. Sprint #689.
#[derive(Debug, serde::Deserialize)]
struct MergeCandidatesQuery {
    min_docs: Option<u64>,
    max_docs: Option<u64>,
}

pub async fn segments_merge_candidates(
    State(store): State<IndexStore>,
    Query(q):     Query<MergeCandidatesQuery>,
) -> Json<serde_json::Value> {
    let min_docs = q.min_docs.unwrap_or(0);
    let max_docs = q.max_docs.unwrap_or(u64::MAX);

    let segs = store.list_segments().unwrap_or_default();
    let mut candidates: Vec<serde_json::Value> = segs.into_iter()
        .filter(|(_, num_docs, _)| *num_docs >= min_docs && *num_docs <= max_docs)
        .map(|(id, num_docs, disk_bytes)| serde_json::json!({
            "id":         id,
            "num_docs":   num_docs,
            "disk_bytes": disk_bytes,
        }))
        .collect();
    candidates.sort_by(|a, b| {
        a["num_docs"].as_u64().unwrap_or(0)
            .cmp(&b["num_docs"].as_u64().unwrap_or(0))
    });
    Json(serde_json::json!({
        "count":      candidates.len(),
        "segments":   candidates,
        "filter":     {"min_docs": min_docs, "max_docs": if max_docs == u64::MAX { serde_json::Value::Null } else { serde_json::json!(max_docs) }},
    }))
}

/// GET /api/v1/search/stats/by-tenant?limit=N — doc count por tenant_id.
///
/// Varre todos os documentos do índice e agrupa por `tenant_id`. Retorna
/// `{rows:[{tenant_id,doc_count}]}` ordenado por `doc_count DESC`. Ops endpoint
/// cross-tenant sem RLS. `limit` default 20 max 200. Sprint #684.
#[derive(Debug, serde::Deserialize)]
struct StatsByTenantQuery {
    limit: Option<usize>,
}

/// GET /api/v1/search/index/segments/doc-ratio — densidade de documentos por segmento.
///
/// Calcula `num_docs / disk_bytes` (docs per byte) para cada segmento, ordenado
/// DESC (mais denso primeiro). Segmentos com `disk_bytes = 0` recebem ratio `null`.
/// Retorna `{segments:[{id,num_docs,disk_bytes,docs_per_byte}]}`. Sprint #698.
pub async fn segment_doc_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut out: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, num_docs, disk_bytes)| {
            let ratio = if disk_bytes > 0 {
                serde_json::json!(num_docs as f64 / disk_bytes as f64)
            } else {
                serde_json::Value::Null
            };
            serde_json::json!({
                "id":           id,
                "num_docs":     num_docs,
                "disk_bytes":   disk_bytes,
                "docs_per_byte": ratio,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let ra = a["docs_per_byte"].as_f64().unwrap_or(-1.0);
        let rb = b["docs_per_byte"].as_f64().unwrap_or(-1.0);
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });
    Json(serde_json::json!({"segments": out}))
}

/// GET /api/v1/search/index/segments/percentile?p=N — num_docs e disk_bytes no percentil N.
///
/// Ordena segmentos por num_docs ASC, calcula rank percentil. `p` aceita 0-100 (default 50 = mediana).
/// Retorna `{p,segment_count,num_docs_at_p,disk_bytes_at_p}`. Null quando índice vazio. Sprint #713.
#[derive(Debug, serde::Deserialize)]
struct PercentileQuery {
    p: Option<u64>,
}

pub async fn segment_percentile(
    State(store): State<IndexStore>,
    Query(q):     Query<PercentileQuery>,
) -> Json<serde_json::Value> {
    let p = q.p.unwrap_or(50).clamp(0, 100);
    let mut segs = store.list_segments().unwrap_or_default();
    let count = segs.len();

    if count == 0 {
        return Json(serde_json::json!({
            "p": p, "segment_count": 0,
            "num_docs_at_p": serde_json::Value::Null,
            "disk_bytes_at_p": serde_json::Value::Null,
        }));
    }

    segs.sort_by_key(|(_, num_docs, _)| *num_docs);
    let idx = ((p as usize * (count - 1)) + 50) / 100;
    let idx = idx.min(count - 1);
    let (_, num_docs_at_p, disk_bytes_at_p) = &segs[idx];

    Json(serde_json::json!({
        "p":               p,
        "segment_count":   count,
        "num_docs_at_p":   num_docs_at_p,
        "disk_bytes_at_p": disk_bytes_at_p,
    }))
}

/// GET /api/v1/search/index/segments/stdev — desvio padrão de num_docs e disk_bytes.
///
/// Medida de desbalanceamento entre segmentos. Calcula média e desvio padrão amostral (n-1)
/// de num_docs e disk_bytes via iteração em memória sobre list_segments().
/// Retorna `{segment_count,num_docs_mean,num_docs_stdev,disk_bytes_mean,disk_bytes_stdev}`.
/// Todos os campos são null quando o índice está vazio; stdev é null com 1 segmento. Sprint #718.
pub async fn segment_stdev(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count":    0,
            "num_docs_mean":    serde_json::Value::Null,
            "num_docs_stdev":   serde_json::Value::Null,
            "disk_bytes_mean":  serde_json::Value::Null,
            "disk_bytes_stdev": serde_json::Value::Null,
        }));
    }

    let docs_vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes_vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    let mean_f = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let stdev_f = |v: &[f64], mean: f64| -> Option<f64> {
        if v.len() < 2 { return None; }
        let variance = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (v.len() - 1) as f64;
        Some(variance.sqrt())
    };

    let docs_mean  = mean_f(&docs_vals);
    let bytes_mean = mean_f(&bytes_vals);

    Json(serde_json::json!({
        "segment_count":    n,
        "num_docs_mean":    docs_mean,
        "num_docs_stdev":   stdev_f(&docs_vals, docs_mean),
        "disk_bytes_mean":  bytes_mean,
        "disk_bytes_stdev": stdev_f(&bytes_vals, bytes_mean),
    }))
}

/// GET /api/v1/search/index/segments/entropy — entropia de Shannon da distribuição de num_docs.
///
/// H = -sum(p_i * log2(p_i)) onde p_i = num_docs_i / total_docs.
/// H=0 quando há um único segmento; H=log2(n) quando todos têm o mesmo tamanho.
/// Retorna `{segment_count,total_docs,entropy_bits}`. Null quando índice vazio. Sprint #723.
pub async fn segment_entropy(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count": 0,
            "total_docs":    0,
            "entropy_bits":  serde_json::Value::Null,
        }));
    }

    let total_docs: u64 = segs.iter().map(|(_, d, _)| *d).sum();

    let entropy = if total_docs == 0 {
        0.0_f64
    } else {
        segs.iter()
            .filter(|(_, d, _)| *d > 0)
            .map(|(_, d, _)| {
                let p = *d as f64 / total_docs as f64;
                -p * p.log2()
            })
            .sum::<f64>()
    };

    Json(serde_json::json!({
        "segment_count": n,
        "total_docs":    total_docs,
        "entropy_bits":  entropy,
    }))
}

/// GET /api/v1/search/index/segments/gini — coeficiente de Gini da distribuição de num_docs.
///
/// G = (2 * sum(i * x_i) / (n * sum(x_i))) - (n+1)/n  onde x_i é num_docs ordenado ASC.
/// 0 = distribuição perfeitamente uniforme; 1 = toda concentração num único segmento.
/// Retorna `{segment_count,gini}`. Null quando índice vazio ou 1 segmento. Sprint #728.
pub async fn segment_gini(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let mut segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n < 2 {
        return Json(serde_json::json!({
            "segment_count": n,
            "gini": serde_json::Value::Null,
        }));
    }

    segs.sort_by_key(|(_, d, _)| *d);
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let sum: f64 = vals.iter().sum();

    let gini = if sum == 0.0 {
        0.0
    } else {
        let weighted_sum: f64 = vals.iter().enumerate()
            .map(|(i, x)| (i + 1) as f64 * x)
            .sum();
        (2.0 * weighted_sum / (n as f64 * sum)) - (n as f64 + 1.0) / n as f64
    };

    Json(serde_json::json!({
        "segment_count": n,
        "gini": gini,
    }))
}

/// GET /api/v1/search/index/segments/iqr — IQR (Q3-Q1) de num_docs e disk_bytes.
///
/// Calcula Q1 (25°) e Q3 (75°) via interpolação linear sobre valores ordenados.
/// IQR é robusto a outliers vs stdev (#718). Null quando n < 2.
/// Retorna `{segment_count,num_docs_q1,num_docs_q3,num_docs_iqr,disk_bytes_q1,disk_bytes_q3,disk_bytes_iqr}`. Sprint #733.
pub async fn segment_iqr(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n < 2 {
        return Json(serde_json::json!({
            "segment_count":  n,
            "num_docs_q1":    serde_json::Value::Null,
            "num_docs_q3":    serde_json::Value::Null,
            "num_docs_iqr":   serde_json::Value::Null,
            "disk_bytes_q1":  serde_json::Value::Null,
            "disk_bytes_q3":  serde_json::Value::Null,
            "disk_bytes_iqr": serde_json::Value::Null,
        }));
    }

    fn quantile(sorted: &[f64], p: f64) -> f64 {
        let h = p * (sorted.len() - 1) as f64;
        let lo = h.floor() as usize;
        let hi = (lo + 1).min(sorted.len() - 1);
        sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
    }

    let mut docs: Vec<f64>  = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mut bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    docs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    bytes.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let dq1 = quantile(&docs,  0.25);
    let dq3 = quantile(&docs,  0.75);
    let bq1 = quantile(&bytes, 0.25);
    let bq3 = quantile(&bytes, 0.75);

    Json(serde_json::json!({
        "segment_count":  n,
        "num_docs_q1":    dq1,
        "num_docs_q3":    dq3,
        "num_docs_iqr":   dq3 - dq1,
        "disk_bytes_q1":  bq1,
        "disk_bytes_q3":  bq3,
        "disk_bytes_iqr": bq3 - bq1,
    }))
}

/// GET /api/v1/search/index/segments/range — min, max e range (max-min) de num_docs e disk_bytes.
///
/// Medida simples de spread. Null quando n=0. Complementa stdev (#718) e IQR (#733).
/// Retorna `{segment_count,num_docs_min,num_docs_max,num_docs_range,disk_bytes_min,disk_bytes_max,disk_bytes_range}`. Sprint #738.
pub async fn segment_range(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count":    0,
            "num_docs_min":     serde_json::Value::Null,
            "num_docs_max":     serde_json::Value::Null,
            "num_docs_range":   serde_json::Value::Null,
            "disk_bytes_min":   serde_json::Value::Null,
            "disk_bytes_max":   serde_json::Value::Null,
            "disk_bytes_range": serde_json::Value::Null,
        }));
    }

    let dmin = segs.iter().map(|(_, d, _)| *d).min().unwrap();
    let dmax = segs.iter().map(|(_, d, _)| *d).max().unwrap();
    let bmin = segs.iter().map(|(_, _, b)| *b).min().unwrap();
    let bmax = segs.iter().map(|(_, _, b)| *b).max().unwrap();

    Json(serde_json::json!({
        "segment_count":    n,
        "num_docs_min":     dmin,
        "num_docs_max":     dmax,
        "num_docs_range":   dmax - dmin,
        "disk_bytes_min":   bmin,
        "disk_bytes_max":   bmax,
        "disk_bytes_range": bmax - bmin,
    }))
}

/// GET /api/v1/search/index/segments/cv — coeficiente de variação (stdev/mean) de num_docs e disk_bytes.
///
/// CV = stdev / mean — normaliza dispersão pela escala. Null quando n<2 ou mean=0.
/// Retorna `{segment_count,num_docs_cv,disk_bytes_cv}`. Sprint #743.
pub async fn segment_cv(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n < 2 {
        return Json(serde_json::json!({
            "segment_count": n,
            "num_docs_cv":   serde_json::Value::Null,
            "disk_bytes_cv": serde_json::Value::Null,
        }));
    }

    fn cv(vals: &[f64]) -> Option<f64> {
        let n = vals.len() as f64;
        let mean = vals.iter().sum::<f64>() / n;
        if mean == 0.0 { return None; }
        let var = vals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        Some(var.sqrt() / mean)
    }

    let docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    Json(serde_json::json!({
        "segment_count": n,
        "num_docs_cv":   cv(&docs),
        "disk_bytes_cv": cv(&bytes),
    }))
}

/// GET /api/v1/search/index/segments/skewness — assimetria amostral g1 de num_docs e disk_bytes.
///
/// g1 = (n/((n-1)(n-2))) * Σ((xi-mean)³/stdev³) — fórmula Fisher sample skewness.
/// Null quando n < 3. Positivo = cauda direita (muitos segmentos pequenos, alguns grandes).
/// Retorna `{segment_count,num_docs_skewness,disk_bytes_skewness}`. Sprint #748.
pub async fn segment_skewness(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n < 3 {
        return Json(serde_json::json!({
            "segment_count":      n,
            "num_docs_skewness":  serde_json::Value::Null,
            "disk_bytes_skewness": serde_json::Value::Null,
        }));
    }

    fn skewness(vals: &[f64]) -> Option<f64> {
        let n = vals.len() as f64;
        let mean = vals.iter().sum::<f64>() / n;
        let var  = vals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let std  = var.sqrt();
        if std == 0.0 { return Some(0.0); }
        let m3: f64 = vals.iter().map(|x| ((x - mean) / std).powi(3)).sum();
        Some((n / ((n - 1.0) * (n - 2.0))) * m3)
    }

    let docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    Json(serde_json::json!({
        "segment_count":       n,
        "num_docs_skewness":   skewness(&docs),
        "disk_bytes_skewness": skewness(&bytes),
    }))
}

/// GET /api/v1/search/index/segments/mad — Median Absolute Deviation de num_docs.
///
/// MAD = median(|xi - median(x)|) — robusto a outliers. Null quando n=0.
/// Retorna `{segment_count,num_docs_median,num_docs_mad,disk_bytes_median,disk_bytes_mad}`. Sprint #753.
pub async fn segment_mad(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count":   0,
            "num_docs_median": serde_json::Value::Null,
            "num_docs_mad":    serde_json::Value::Null,
            "disk_bytes_median": serde_json::Value::Null,
            "disk_bytes_mad":  serde_json::Value::Null,
        }));
    }

    fn median_sorted(sorted: &mut Vec<f64>) -> f64 {
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let m = sorted.len();
        if m % 2 == 1 { sorted[m / 2] } else { (sorted[m / 2 - 1] + sorted[m / 2]) / 2.0 }
    }

    fn mad(vals: &[f64]) -> (f64, f64) {
        let mut v = vals.to_vec();
        let med = median_sorted(&mut v);
        let mut deviations: Vec<f64> = vals.iter().map(|x| (x - med).abs()).collect();
        let mad_val = median_sorted(&mut deviations);
        (med, mad_val)
    }

    let docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let (dm, dmad) = mad(&docs);
    let (bm, bmad) = mad(&bytes);

    Json(serde_json::json!({
        "segment_count":     n,
        "num_docs_median":   dm,
        "num_docs_mad":      dmad,
        "disk_bytes_median": bm,
        "disk_bytes_mad":    bmad,
    }))
}

/// GET /api/v1/search/index/segments/kurtosis — curtose excess g2 de num_docs e disk_bytes.
///
/// g2 = [(n(n+1)/((n-1)(n-2)(n-3))) * Σ((xi-m)⁴/s⁴)] - 3(n-1)²/((n-2)(n-3)).
/// Null quando n < 4. Positivo = caudas pesadas (leptocúrtico). Sprint #758.
pub async fn segment_kurtosis(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n < 4 {
        return Json(serde_json::json!({
            "segment_count":       n,
            "num_docs_kurtosis":   serde_json::Value::Null,
            "disk_bytes_kurtosis": serde_json::Value::Null,
        }));
    }

    fn kurtosis(vals: &[f64]) -> Option<f64> {
        let n = vals.len() as f64;
        let mean = vals.iter().sum::<f64>() / n;
        let var  = vals.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
        let std  = var.sqrt();
        if std == 0.0 { return Some(0.0); }
        let m4: f64 = vals.iter().map(|x| ((x - mean) / std).powi(4)).sum();
        let kurt = (n * (n + 1.0)) / ((n - 1.0) * (n - 2.0) * (n - 3.0)) * m4
            - 3.0 * (n - 1.0).powi(2) / ((n - 2.0) * (n - 3.0));
        Some(kurt)
    }

    let docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    Json(serde_json::json!({
        "segment_count":       n,
        "num_docs_kurtosis":   kurtosis(&docs),
        "disk_bytes_kurtosis": kurtosis(&bytes),
    }))
}

/// GET /api/v1/search/index/segments/trimmed-mean?pct=N — média podada descartando top+bottom N%.
///
/// `pct` 0-49, default 10. Remove os N% menores e N% maiores de num_docs e disk_bytes.
/// Robusta a outliers extremos. Retorna `{segment_count,pct,num_docs_trimmed_mean,disk_bytes_trimmed_mean}`. Sprint #763.
#[derive(Debug, serde::Deserialize)]
pub struct TrimmedMeanQuery {
    pct: Option<usize>,
}

pub async fn segment_trimmed_mean(
    State(store): State<IndexStore>,
    Query(q): Query<TrimmedMeanQuery>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let pct = q.pct.unwrap_or(10).min(49);

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count": 0,
            "pct": pct,
            "num_docs_trimmed_mean":   serde_json::Value::Null,
            "disk_bytes_trimmed_mean": serde_json::Value::Null,
        }));
    }

    fn trimmed_mean(vals: &mut Vec<f64>, pct: usize) -> f64 {
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let trim = (vals.len() * pct / 100).min(vals.len() / 2);
        let slice = &vals[trim..vals.len() - trim];
        if slice.is_empty() { return 0.0; }
        slice.iter().sum::<f64>() / slice.len() as f64
    }

    let mut docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mut bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    Json(serde_json::json!({
        "segment_count":           n,
        "pct":                     pct,
        "num_docs_trimmed_mean":   trimmed_mean(&mut docs,  pct),
        "disk_bytes_trimmed_mean": trimmed_mean(&mut bytes, pct),
    }))
}

/// GET /api/v1/search/index/segments/harmonic-mean — média harmônica de num_docs.
///
/// H = n / Σ(1/xi). Undefined quando algum xi=0 (retorna null). Sprint #768.
pub async fn segment_harmonic_mean(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count":             0,
            "num_docs_harmonic_mean":    serde_json::Value::Null,
            "disk_bytes_harmonic_mean":  serde_json::Value::Null,
        }));
    }

    fn harmonic(vals: &[f64]) -> Option<f64> {
        if vals.iter().any(|&x| x == 0.0) { return None; }
        let sum_inv: f64 = vals.iter().map(|x| 1.0 / x).sum();
        Some(vals.len() as f64 / sum_inv)
    }

    let docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    Json(serde_json::json!({
        "segment_count":            n,
        "num_docs_harmonic_mean":   harmonic(&docs),
        "disk_bytes_harmonic_mean": harmonic(&bytes),
    }))
}

/// GET /api/v1/search/index/segments/geometric-mean — média geométrica de num_docs e disk_bytes.
///
/// G = exp(Σ(ln(xi))/n). Undefined quando algum xi=0 (retorna null). Sprint #773.
pub async fn segment_geometric_mean(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count":              0,
            "num_docs_geometric_mean":    serde_json::Value::Null,
            "disk_bytes_geometric_mean":  serde_json::Value::Null,
        }));
    }

    fn geometric(vals: &[f64]) -> Option<f64> {
        if vals.iter().any(|&x| x <= 0.0) { return None; }
        let sum_ln: f64 = vals.iter().map(|x| x.ln()).sum();
        Some((sum_ln / vals.len() as f64).exp())
    }

    let docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    Json(serde_json::json!({
        "segment_count":             n,
        "num_docs_geometric_mean":   geometric(&docs),
        "disk_bytes_geometric_mean": geometric(&bytes),
    }))
}

/// GET /api/v1/search/index/segments/cumulative — acumulado de num_docs e disk_bytes por segmento.
///
/// Ordena segmentos por num_docs ASC e calcula cumsum de num_docs e disk_bytes.
/// Útil pra visualizar distribuição acumulada — quantos docs/bytes cobrem os N menores segmentos.
/// Retorna `{segments:[{id,num_docs,disk_bytes,cum_docs,cum_bytes}]}`. Sprint #708.
pub async fn segments_cumulative(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_by_key(|(_, num_docs, _)| *num_docs);

    let mut cum_docs: u64 = 0;
    let mut cum_bytes: u64 = 0;
    let out: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, num_docs, disk_bytes)| {
            cum_docs  += num_docs;
            cum_bytes += disk_bytes;
            serde_json::json!({
                "id":        id,
                "num_docs":  num_docs,
                "disk_bytes": disk_bytes,
                "cum_docs":  cum_docs,
                "cum_bytes": cum_bytes,
            })
        })
        .collect();
    Json(serde_json::json!({"segments": out}))
}

/// GET /api/v1/search/index/segments/overlap — pares de segmentos com faixas de num_docs sobrepostas.
///
/// Dois segmentos "sobrepõem" quando max(min_a, min_b) <= min(max_a, max_b) usando
/// bandas de tamanho `band` (default 1000 docs). Segmentos na mesma banda são candidatos
/// a merge combinado. Retorna `{band,pairs:[{seg_a,seg_b,docs_a,docs_b}]}` ordenado por
/// (docs_a ASC). `band` query param default 1000. Sprint #703.
#[derive(Debug, serde::Deserialize)]
struct OverlapQuery {
    band: Option<u64>,
}

pub async fn segments_overlap(
    State(store): State<IndexStore>,
    Query(q):     Query<OverlapQuery>,
) -> Json<serde_json::Value> {
    let band = q.band.unwrap_or(1000).max(1);
    let segs = store.list_segments().unwrap_or_default();

    let mut pairs: Vec<serde_json::Value> = Vec::new();
    for i in 0..segs.len() {
        for j in (i + 1)..segs.len() {
            let (ref id_a, docs_a, _) = segs[i];
            let (ref id_b, docs_b, _) = segs[j];
            let band_a = docs_a / band;
            let band_b = docs_b / band;
            if band_a == band_b {
                pairs.push(serde_json::json!({
                    "seg_a":  id_a,
                    "seg_b":  id_b,
                    "docs_a": docs_a,
                    "docs_b": docs_b,
                    "band":   band_a * band,
                }));
            }
        }
    }
    pairs.sort_by(|a, b| {
        a["docs_a"].as_u64().unwrap_or(0)
            .cmp(&b["docs_a"].as_u64().unwrap_or(0))
    });
    Json(serde_json::json!({"band": band, "pair_count": pairs.len(), "pairs": pairs}))
}

/// GET /api/v1/search/index/segments/winsorized-mean?pct=N — média Winsorizada de num_docs e disk_bytes.
///
/// Clamp (não descarta) os top/bottom N% ao valor do percentil correspondente, depois tira média.
/// `pct` default 10, max 40. Retorna `{segment_count,pct,num_docs_winsorized_mean,disk_bytes_winsorized_mean}`. Sprint #778.
#[derive(Debug, serde::Deserialize)]
pub struct WinsorizedMeanQuery {
    pub pct: Option<usize>,
}

pub async fn segment_winsorized_mean(
    State(store): State<IndexStore>,
    Query(q):     Query<WinsorizedMeanQuery>,
) -> Json<serde_json::Value> {
    let pct  = q.pct.unwrap_or(10).min(40);
    let segs = store.list_segments().unwrap_or_default();
    let n    = segs.len();

    if n == 0 {
        return Json(serde_json::json!({
            "segment_count":              0,
            "pct":                        pct,
            "num_docs_winsorized_mean":   serde_json::Value::Null,
            "disk_bytes_winsorized_mean": serde_json::Value::Null,
        }));
    }

    fn winsorized(vals: &mut Vec<f64>, pct: usize) -> f64 {
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = vals.len();
        let trim = (n * pct / 100).min(n / 2);
        let lo = vals[trim];
        let hi = vals[n - 1 - trim];
        let clamped: Vec<f64> = vals.iter().map(|&x| x.clamp(lo, hi)).collect();
        clamped.iter().sum::<f64>() / n as f64
    }

    let mut docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mut bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();

    Json(serde_json::json!({
        "segment_count":              n,
        "pct":                        pct,
        "num_docs_winsorized_mean":   winsorized(&mut docs,  pct),
        "disk_bytes_winsorized_mean": winsorized(&mut bytes, pct),
    }))
}

/// GET /api/v1/search/index/segments/normalized-entropy — entropia normalizada H/log2(n) de num_docs.
///
/// H normalizada ∈ [0,1]; 0 = todos os docs num único segmento, 1 = distribuição perfeitamente uniforme.
/// Retorna `{segment_count,raw_entropy_bits,normalized_entropy,disk_bytes_entropy,disk_bytes_normalized}`. Sprint #783.
pub async fn segment_normalized_entropy(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n    = segs.len();

    if n < 2 {
        return Json(serde_json::json!({
            "segment_count":          n,
            "raw_entropy_bits":       serde_json::Value::Null,
            "normalized_entropy":     serde_json::Value::Null,
            "disk_bytes_entropy":     serde_json::Value::Null,
            "disk_bytes_normalized":  serde_json::Value::Null,
        }));
    }

    fn entropy_and_norm(vals: &[u64]) -> (f64, f64) {
        let total: f64 = vals.iter().map(|&x| x as f64).sum();
        if total == 0.0 { return (0.0, 0.0); }
        let h: f64 = vals.iter().filter(|&&x| x > 0).map(|&x| {
            let p = x as f64 / total;
            -p * p.log2()
        }).sum();
        let max_h = (vals.len() as f64).log2();
        (h, if max_h > 0.0 { h / max_h } else { 0.0 })
    }

    let docs:  Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    let bytes: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();

    let (h_docs,  hn_docs)  = entropy_and_norm(&docs);
    let (h_bytes, hn_bytes) = entropy_and_norm(&bytes);

    Json(serde_json::json!({
        "segment_count":         n,
        "raw_entropy_bits":      h_docs,
        "normalized_entropy":    hn_docs,
        "disk_bytes_entropy":    h_bytes,
        "disk_bytes_normalized": hn_bytes,
    }))
}

/// GET /api/v1/search/index/segments/relative-sizes — cada segmento como % do total de disk_bytes.
///
/// Retorna `{total_bytes,segments:[{id,num_docs,disk_bytes,pct_bytes}]}` ordenado por disk_bytes DESC.
/// Útil pra visualizar quais segmentos dominam o storage. Sprint #788.
pub async fn segment_relative_sizes(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_by(|a, b| b.2.cmp(&a.2));

    let total_bytes: u64 = segs.iter().map(|(_, _, b)| b).sum();
    let out: Vec<serde_json::Value> = segs.iter().map(|(id, docs, bytes)| {
        let pct = if total_bytes > 0 { *bytes as f64 / total_bytes as f64 * 100.0 } else { 0.0 };
        serde_json::json!({"id": id, "num_docs": docs, "disk_bytes": bytes, "pct_bytes": pct})
    }).collect();

    Json(serde_json::json!({"total_bytes": total_bytes, "segments": out}))
}

/// GET /api/v1/search/index/segments/size-ratio — bytes por doc em cada segmento.
///
/// disk_bytes / num_docs por segmento — indica densidade de indexação.
/// Segmentos com num_docs=0 têm size_ratio=null. Ordenado por size_ratio DESC. Sprint #793.
pub async fn segment_size_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_by(|a, b| {
        let ra = if a.1 > 0 { a.2 as f64 / a.1 as f64 } else { 0.0 };
        let rb = if b.1 > 0 { b.2 as f64 / b.1 as f64 } else { 0.0 };
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });

    let out: Vec<serde_json::Value> = segs.iter().map(|(id, docs, bytes)| {
        let ratio: Option<f64> = if *docs > 0 { Some(*bytes as f64 / *docs as f64) } else { None };
        serde_json::json!({"id": id, "num_docs": docs, "disk_bytes": bytes, "bytes_per_doc": ratio})
    }).collect();

    Json(serde_json::json!({"segments": out}))
}

/// GET /api/v1/search/index/segments/z-scores — z-score de num_docs e disk_bytes por segmento.
///
/// z = (x - mean) / stdev. Segmentos com |z| > 2 são outliers candidatos a merge/vacuum.
/// Retorna `{segment_count,segments:[{id,num_docs,disk_bytes,docs_z,bytes_z}]}` docs_z DESC. Sprint #798.
pub async fn segment_z_scores(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n    = segs.len();

    if n < 2 {
        return Json(serde_json::json!({"segment_count": n, "segments": []}));
    }

    fn mean_stdev(vals: &[f64]) -> (f64, f64) {
        let n = vals.len() as f64;
        let m = vals.iter().sum::<f64>() / n;
        let var = vals.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0);
        (m, var.sqrt())
    }

    let docs_f:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let bytes_f: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let (md, sd) = mean_stdev(&docs_f);
    let (mb, sb) = mean_stdev(&bytes_f);

    let mut out: Vec<serde_json::Value> = segs.iter().map(|(id, docs, bytes)| {
        let dz = if sd > 0.0 { (*docs as f64 - md) / sd } else { 0.0 };
        let bz = if sb > 0.0 { (*bytes as f64 - mb) / sb } else { 0.0 };
        serde_json::json!({"id": id, "num_docs": docs, "disk_bytes": bytes, "docs_z": dz, "bytes_z": bz})
    }).collect();
    out.sort_by(|a, b| {
        b["docs_z"].as_f64().unwrap_or(0.0)
            .partial_cmp(&a["docs_z"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Json(serde_json::json!({"segment_count": n, "segments": out}))
}

/// GET /api/v1/search/index/segments/doc-density — num_docs/disk_bytes por segmento (inverso de size_ratio).
///
/// docs_per_byte = num_docs / disk_bytes. disk_bytes=0 → null. Ordenado por docs_per_byte DESC. Sprint #803.
pub async fn segment_doc_density(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_by(|a, b| {
        let ra = if a.2 > 0 { a.1 as f64 / a.2 as f64 } else { 0.0 };
        let rb = if b.2 > 0 { b.1 as f64 / b.2 as f64 } else { 0.0 };
        rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
    });

    let out: Vec<serde_json::Value> = segs.iter().map(|(id, docs, bytes)| {
        let density: Option<f64> = if *bytes > 0 { Some(*docs as f64 / *bytes as f64) } else { None };
        serde_json::json!({"id": id, "num_docs": docs, "disk_bytes": bytes, "docs_per_byte": density})
    }).collect();

    Json(serde_json::json!({"segments": out}))
}

/// GET /api/v1/search/index/segments/coefficient-dispersion — range/mean (índice de dispersão).
///
/// CD = (max - min) / mean. Valor alto indica segmentos muito desbalanceados.
/// Retorna `{segment_count,num_docs_cd,disk_bytes_cd}`. Sprint #808.
pub async fn segment_coefficient_dispersion(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n    = segs.len();

    if n < 2 {
        return Json(serde_json::json!({
            "segment_count":    n,
            "num_docs_cd":      serde_json::Value::Null,
            "disk_bytes_cd":    serde_json::Value::Null,
        }));
    }

    fn cd(vals: &[u64]) -> Option<f64> {
        let min = *vals.iter().min()?;
        let max = *vals.iter().max()?;
        let mean = vals.iter().sum::<u64>() as f64 / vals.len() as f64;
        if mean == 0.0 { None } else { Some((max - min) as f64 / mean) }
    }

    let docs:  Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    let bytes: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();

    Json(serde_json::json!({
        "segment_count":  n,
        "num_docs_cd":    cd(&docs),
        "disk_bytes_cd":  cd(&bytes),
    }))
}

/// GET /api/v1/search/index/segments/percentile-rank — percentis 25/50/75/90/95 de num_docs e disk_bytes.
///
/// Interpolação linear nos valores ordenados. Sprint #813.
pub async fn segment_percentile_rank(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n    = segs.len();

    if n == 0 {
        return Json(serde_json::json!({"segment_count": 0}));
    }

    fn quantile(sorted: &[f64], p: f64) -> f64 {
        let idx = p * (sorted.len() - 1) as f64;
        let lo  = idx.floor() as usize;
        let hi  = idx.ceil() as usize;
        if lo == hi { sorted[lo] } else { sorted[lo] + (idx - lo as f64) * (sorted[hi] - sorted[lo]) }
    }

    let mut docs:  Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mut bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    docs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    bytes.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let pcts = [0.25_f64, 0.50, 0.75, 0.90, 0.95];
    let docs_pct:  Vec<serde_json::Value> = pcts.iter().map(|&p| {
        serde_json::json!({"pct": (p * 100.0) as u32, "value": quantile(&docs, p)})
    }).collect();
    let bytes_pct: Vec<serde_json::Value> = pcts.iter().map(|&p| {
        serde_json::json!({"pct": (p * 100.0) as u32, "value": quantile(&bytes, p)})
    }).collect();

    Json(serde_json::json!({
        "segment_count":        n,
        "num_docs_percentiles":   docs_pct,
        "disk_bytes_percentiles": bytes_pct,
    }))
}

pub async fn search_stats_by_tenant(
    State(store): State<IndexStore>,
    Query(q):     Query<StatsByTenantQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let limit = q.limit.unwrap_or(20).clamp(1, 200);
    let rows = store
        .docs_count_by_tenant(limit)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let out: Vec<serde_json::Value> = rows.into_iter()
        .map(|(tenant_id, doc_count)| serde_json::json!({
            "tenant_id": tenant_id,
            "doc_count": doc_count,
        }))
        .collect();
    Ok(Json(serde_json::json!({"rows": out})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(q: &str, limit: usize) -> SearchParams {
        SearchParams {
            q:                 q.to_string(),
            tenant_id:         "00000000-0000-0000-0000-000000000000".into(),
            limit,
            offset:            0,
            facet:             None,
            facet_granularity: None,
            after_secs:        None,
            before_secs:       None,
        }
    }

    #[test]
    fn accepts_default() {
        let mut params = p("hello", DEFAULT_LIMIT);
        assert!(validate_search_params(&mut params).is_none());
        assert_eq!(params.limit, DEFAULT_LIMIT);
    }

    #[test]
    fn rejects_oversize_query() {
        let mut params = p(&"x".repeat(MAX_QUERY_BYTES + 1), DEFAULT_LIMIT);
        let err = validate_search_params(&mut params).unwrap();
        assert!(err.contains("query too large"), "got: {err}");
    }

    #[test]
    fn accepts_query_at_boundary() {
        let mut params = p(&"x".repeat(MAX_QUERY_BYTES), DEFAULT_LIMIT);
        assert!(validate_search_params(&mut params).is_none());
    }

    #[test]
    fn rejects_zero_limit() {
        let mut params = p("hello", 0);
        let err = validate_search_params(&mut params).unwrap();
        assert!(err.contains("limit must be >= 1"), "got: {err}");
    }

    #[test]
    fn clamps_excessive_limit() {
        // Clamp em vez de rejeitar — usuário não é punido por passar
        // limite alto via UI; só protege o índice.
        let mut params = p("hello", usize::MAX);
        assert!(validate_search_params(&mut params).is_none());
        assert_eq!(params.limit, MAX_LIMIT);
    }

    #[test]
    fn accepts_limit_at_boundary() {
        let mut params = p("hello", MAX_LIMIT);
        assert!(validate_search_params(&mut params).is_none());
        assert_eq!(params.limit, MAX_LIMIT);
    }
}

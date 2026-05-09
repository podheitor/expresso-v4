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

/// GET /api/v1/search/index/segments/outliers?threshold=N — segmentos com |z-score| > threshold.
///
/// Usa z-score de num_docs. threshold default 2.0. Retorna `{threshold,outliers:[{id,num_docs,disk_bytes,z}]}`. Sprint #818.
#[derive(Debug, serde::Deserialize)]
pub struct OutliersQuery {
    pub threshold: Option<f64>,
}

pub async fn segment_outliers(
    State(store): State<IndexStore>,
    Query(q):     Query<OutliersQuery>,
) -> Json<serde_json::Value> {
    let threshold = q.threshold.unwrap_or(2.0_f64).abs();
    let segs = store.list_segments().unwrap_or_default();
    let n    = segs.len();

    if n < 2 {
        return Json(serde_json::json!({"threshold": threshold, "outliers": []}));
    }

    let docs: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let var  = docs.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
    let sd   = var.sqrt();

    let outliers: Vec<serde_json::Value> = if sd == 0.0 {
        vec![]
    } else {
        segs.iter()
            .filter_map(|(id, nd, db)| {
                let z = (*nd as f64 - mean) / sd;
                if z.abs() > threshold {
                    Some(serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "z_score": z}))
                } else {
                    None
                }
            })
            .collect()
    };

    Json(serde_json::json!({"segment_count": n, "threshold": threshold, "outliers": outliers}))
}

/// GET /api/v1/search/index/segments/size-bands — histograma de num_docs por banda fixa.
///
/// Bandas: tiny(<100)/small(100-999)/medium(1000-9999)/large(10000-99999)/huge(≥100000).
/// Retorna `{bands:{tiny,small,medium,large,huge}}`. Sprint #823.
pub async fn segment_size_bands(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let (mut tiny, mut small, mut medium, mut large, mut huge) = (0_u64, 0_u64, 0_u64, 0_u64, 0_u64);
    for (_, nd, _) in &segs {
        match nd {
            0..=99        => tiny   += 1,
            100..=999     => small  += 1,
            1000..=9999   => medium += 1,
            10000..=99999 => large  += 1,
            _             => huge   += 1,
        }
    }
    Json(serde_json::json!({
        "segment_count": segs.len(),
        "bands": {"tiny": tiny, "small": small, "medium": medium, "large": large, "huge": huge},
    }))
}

/// GET /api/v1/search/index/segments/top-docs-ratio — razão num_docs/total_docs por segmento.
///
/// pct_docs = num_docs / total_docs * 100. Ordenado pct_docs DESC. Sprint #828.
pub async fn segment_top_docs_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs  = store.list_segments().unwrap_or_default();
    let total: u64 = segs.iter().map(|(_, d, _)| d).sum();

    if total == 0 {
        return Json(serde_json::json!({"total_docs": 0, "segments": []}));
    }

    let mut out: Vec<serde_json::Value> = segs.iter()
        .map(|(id, nd, db)| {
            let pct = *nd as f64 / total as f64 * 100.0;
            serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "pct_docs": pct})
        })
        .collect();
    out.sort_by(|a, b| b["pct_docs"].as_f64().unwrap_or(0.0)
        .partial_cmp(&a["pct_docs"].as_f64().unwrap_or(0.0))
        .unwrap_or(std::cmp::Ordering::Equal));

    Json(serde_json::json!({"total_docs": total, "segment_count": segs.len(), "segments": out}))
}

/// GET /api/v1/search/index/segments/decay — razão de segmentos com num_docs < threshold.
///
/// threshold default 1000. decay_ratio = tiny_count / total_count.
/// Retorna `{threshold,total,below_threshold,decay_ratio}`. Sprint #833.
#[derive(Debug, serde::Deserialize)]
pub struct DecayQuery {
    pub threshold: Option<u64>,
}

pub async fn segment_decay(
    State(store): State<IndexStore>,
    Query(q):     Query<DecayQuery>,
) -> Json<serde_json::Value> {
    let threshold = q.threshold.unwrap_or(1000);
    let segs  = store.list_segments().unwrap_or_default();
    let total = segs.len() as u64;
    let below = segs.iter().filter(|(_, d, _)| *d < threshold).count() as u64;
    let ratio = if total == 0 { 0.0_f64 } else { below as f64 / total as f64 };

    Json(serde_json::json!({
        "segment_count":    total,
        "threshold":        threshold,
        "below_threshold":  below,
        "decay_ratio":      ratio,
    }))
}

/// GET /api/v1/search/index/segments/balance-score — 1 − (stdev/mean) de num_docs; ∈ (-∞,1].
///
/// balance_score próximo de 1 = segmentos uniformes. Retorna `{balance_score,mean,stdev,segment_count}`. Sprint #838.
pub async fn segment_balance_score(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"balance_score": null, "mean": null, "stdev": null, "segment_count": 0}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let stdev = if n > 1 {
        let var = docs.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64;
        var.sqrt()
    } else {
        0.0
    };
    let balance = if mean == 0.0 { 1.0 } else { 1.0 - stdev / mean };
    Json(serde_json::json!({
        "balance_score":   balance,
        "mean":            mean,
        "stdev":           stdev,
        "segment_count":   n,
    }))
}

/// GET /api/v1/search/index/segments/age-index-ratio — doc_count / disk_bytes_per_segment × count.
///
/// Retorna `{total_docs,total_bytes,segment_count,avg_bytes_per_doc}`. Sprint #843.
pub async fn segment_age_index_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total_docs: u64  = segs.iter().map(|(_, d, _)| d).sum();
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| b).sum();
    let segment_count = segs.len() as u64;
    let avg_bytes_per_doc = if total_docs == 0 { 0.0_f64 } else { total_bytes as f64 / total_docs as f64 };
    Json(serde_json::json!({
        "total_docs":        total_docs,
        "total_bytes":       total_bytes,
        "segment_count":     segment_count,
        "avg_bytes_per_doc": avg_bytes_per_doc,
    }))
}

/// GET /api/v1/search/index/segments/doc-index-ratio — total_docs / segment_count; media de docs por segmento.
///
/// Retorna `{docs_per_segment,total_docs,segment_count}`. Sprint #848.
pub async fn segment_doc_index_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total_docs: u64  = segs.iter().map(|(_, d, _)| d).sum();
    let segment_count = segs.len() as u64;
    let docs_per_segment = if segment_count == 0 { 0.0_f64 } else { total_docs as f64 / segment_count as f64 };
    Json(serde_json::json!({
        "docs_per_segment": docs_per_segment,
        "total_docs":       total_docs,
        "segment_count":    segment_count,
    }))
}

/// GET /api/v1/search/index/segments/fragmentation — segment_count / total_docs; razão de fragmentação.
///
/// Valor alto indica muitos segmentos pequenos. Retorna `{fragmentation,segment_count,total_docs}`. Sprint #853.
pub async fn segment_fragmentation(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total_docs: u64  = segs.iter().map(|(_, d, _)| d).sum();
    let segment_count = segs.len() as u64;
    let fragmentation = if total_docs == 0 { 0.0_f64 } else { segment_count as f64 / total_docs as f64 };
    Json(serde_json::json!({
        "fragmentation":  fragmentation,
        "segment_count":  segment_count,
        "total_docs":     total_docs,
    }))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-by-segment — disk_bytes/num_docs por segmento DESC.
///
/// Retorna `{rows:[{id,num_docs,disk_bytes,bytes_per_doc}]}`. Sprint #858.
pub async fn segment_bytes_per_doc_by_segment(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut rows: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, nd, db)| {
            let bpd = if nd == 0 { 0.0_f64 } else { db as f64 / nd as f64 };
            serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": bpd})
        })
        .collect();
    rows.sort_by(|a, b| {
        b["bytes_per_doc"].as_f64().unwrap_or(0.0)
            .partial_cmp(&a["bytes_per_doc"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Json(serde_json::json!({"rows": rows}))
}

/// GET /api/v1/search/index/segments/health-score — balance × (1 − fragmentation).
///
/// Composite score ∈ (-∞, 1]: próximo de 1 = índice saudável. Sprint #863.
pub async fn segment_health_score(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"health_score": null, "balance_score": null, "fragmentation": null, "segment_count": 0}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let total_docs: f64 = docs.iter().sum();
    let mean = total_docs / n as f64;
    let stdev = if n > 1 {
        (docs.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
    } else { 0.0 };
    let balance = if mean == 0.0 { 1.0 } else { 1.0 - stdev / mean };
    let fragmentation = if total_docs == 0.0 { 0.0 } else { n as f64 / total_docs };
    let health = balance * (1.0 - fragmentation.min(1.0));
    Json(serde_json::json!({
        "health_score":  health,
        "balance_score": balance,
        "fragmentation": fragmentation,
        "segment_count": n,
        "total_docs":    total_docs as u64,
    }))
}

/// GET /api/v1/search/index/segments/write-amplification — total_bytes / total_docs; bytes-per-doc global.
///
/// Análogo a size-ratio mas agrega todos os segmentos. Sprint #868.
pub async fn segment_write_amplification(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total_docs: u64  = segs.iter().map(|(_, d, _)| d).sum();
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| b).sum();
    let write_amplification = if total_docs == 0 { 0.0_f64 } else { total_bytes as f64 / total_docs as f64 };
    Json(serde_json::json!({
        "write_amplification": write_amplification,
        "total_bytes":         total_bytes,
        "total_docs":          total_docs,
        "segment_count":       segs.len(),
    }))
}

/// GET /api/v1/search/index/segments/utilization?max_docs=N — num_docs/max_docs ratio por segmento.
///
/// `max_docs` default 10_000_000. Retorna `{rows:[{id,num_docs,utilization_pct}]}`. Sprint #873.
pub async fn segment_utilization(
    State(store): State<IndexStore>,
    Query(q):     Query<DecayQuery>,
) -> Json<serde_json::Value> {
    let max_docs = q.threshold.unwrap_or(10_000_000);
    let segs = store.list_segments().unwrap_or_default();
    let rows: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, nd, _)| {
            let pct = if max_docs == 0 { 0.0_f64 } else { nd as f64 / max_docs as f64 * 100.0 };
            serde_json::json!({"id": id, "num_docs": nd, "utilization_pct": pct})
        })
        .collect();
    Json(serde_json::json!({"max_docs_per_segment": max_docs, "rows": rows}))
}

/// GET /api/v1/search/index/segments/bottom-by-docs — id+num_docs do segmento com menos docs.
///
/// Retorna `{segment: {id,num_docs,disk_bytes} | null}`. Sprint #898.
pub async fn segment_bottom_by_docs(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let bottom = segs.into_iter().min_by_key(|(_, nd, _)| *nd);
    let segment = bottom.map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}));
    Json(serde_json::json!({"segment": segment}))
}

/// GET /api/v1/search/index/segments/docs-above-median — COUNT segmentos acima da mediana de num_docs.
///
/// Retorna `{above_median,at_or_below_median,median_docs,segment_count}`. Sprint #908.
pub async fn segment_docs_above_median(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"above_median": 0, "at_or_below_median": 0, "median_docs": null, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let median = if n % 2 == 1 { docs[n / 2] as f64 } else { (docs[n / 2 - 1] + docs[n / 2]) as f64 / 2.0 };
    let above = docs.iter().filter(|&&d| d as f64 > median).count();
    Json(serde_json::json!({
        "above_median":      above,
        "at_or_below_median": n - above,
        "median_docs":       median,
        "segment_count":     n,
    }))
}

/// GET /api/v1/search/index/segments/size-spread — max_bytes - min_bytes; amplitude de disk_bytes.
///
/// Retorna `{min_bytes,max_bytes,spread_bytes,segment_count}`. Sprint #933.
pub async fn segment_size_spread(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_bytes": null, "max_bytes": null, "spread_bytes": null, "segment_count": 0}));
    }
    let min_bytes = segs.iter().map(|(_, _, db)| *db).min().unwrap_or(0);
    let max_bytes = segs.iter().map(|(_, _, db)| *db).max().unwrap_or(0);
    let spread = max_bytes.saturating_sub(min_bytes);
    Json(serde_json::json!({"min_bytes": min_bytes, "max_bytes": max_bytes, "spread_bytes": spread, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/compaction-ratio — avg docs/segment (total_docs/segment_count).
///
/// Retorna `{compaction_ratio,total_docs,segment_count}`. Sprint #928.
pub async fn segment_compaction_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"compaction_ratio": null, "total_docs": 0, "segment_count": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let ratio = total_docs as f64 / n as f64;
    Json(serde_json::json!({"compaction_ratio": ratio, "total_docs": total_docs, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-bytes-ratio-stdev — stdev de (num_docs/disk_bytes) por segmento. Sprint #1138.
pub async fn segment_docs_bytes_ratio_stdev(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_bytes_ratio_stdev": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter()
        .filter(|(_, _, db)| *db > 0)
        .map(|(_, nd, db)| *nd as f64 / *db as f64)
        .collect();
    let val = if ratios.len() < 2 {
        serde_json::Value::Null
    } else {
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        let variance = ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (ratios.len() - 1) as f64;
        serde_json::json!(variance.sqrt())
    };
    Json(serde_json::json!({"segment_count": n, "docs_bytes_ratio_stdev": val}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-mean — média de disk_bytes/num_docs por segmento. Sprint #1143.
pub async fn segment_bytes_per_doc_mean(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_mean": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.is_empty() {
        serde_json::Value::Null
    } else {
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        serde_json::json!(mean)
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_mean": val}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-stdev — stdev de disk_bytes/num_docs por segmento. Sprint #1148.
pub async fn segment_bytes_per_doc_stdev(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_stdev": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.len() < 2 {
        serde_json::Value::Null
    } else {
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        let variance = ratios.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (ratios.len() - 1) as f64;
        serde_json::json!(variance.sqrt())
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_stdev": val}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-cv — coeficiente de variação de disk_bytes/num_docs. Sprint #1153.
pub async fn segment_bytes_per_doc_cv(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_cv": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.len() < 2 {
        serde_json::Value::Null
    } else {
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        if mean == 0.0 {
            serde_json::Value::Null
        } else {
            let variance = ratios.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (ratios.len() - 1) as f64;
            let stdev = variance.sqrt();
            serde_json::json!(stdev / mean)
        }
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_cv": val}))
}

/// GET /api/v1/search/index/segments/bytes-above-p75 — segmentos com disk_bytes acima do P75. Sprint #1158.
pub async fn segment_bytes_above_p75(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "segment_count": 0, "p75_bytes": null}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes_sorted.sort_unstable();
    let p75_idx = ((n as f64 * 0.75) as usize).min(n - 1);
    let p75 = bytes_sorted[p75_idx];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db > p75)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"segment_count": n, "p75_bytes": p75, "above_count": above.len(), "segments": above}))
}

/// GET /api/v1/search/index/segments/bytes-median — mediana de disk_bytes dos segmentos. Sprint #1163.
pub async fn segment_bytes_median(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_median": null, "segment_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes_sorted.sort_unstable();
    let median = if n % 2 == 1 {
        bytes_sorted[n / 2] as f64
    } else {
        (bytes_sorted[n / 2 - 1] + bytes_sorted[n / 2]) as f64 / 2.0
    };
    Json(serde_json::json!({"segment_count": n, "bytes_median": median}))
}

/// GET /api/v1/search/index/segments/docs-median — mediana de num_docs dos segmentos. Sprint #1168.
pub async fn segment_docs_median(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_median": null, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let median = if n % 2 == 1 {
        docs_sorted[n / 2] as f64
    } else {
        (docs_sorted[n / 2 - 1] + docs_sorted[n / 2]) as f64 / 2.0
    };
    Json(serde_json::json!({"segment_count": n, "docs_median": median}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p75 — percentil 75 do ratio disk_bytes/num_docs. Sprint #1173.
pub async fn segment_bytes_per_doc_p75(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_p75": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.is_empty() {
        serde_json::Value::Null
    } else {
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p75_idx = ((ratios.len() as f64 * 0.75) as usize).min(ratios.len() - 1);
        serde_json::json!(ratios[p75_idx])
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_p75": val}))
}

/// GET /api/v1/search/index/segments/docs-p25 — percentil 25 de num_docs dos segmentos. Sprint #1178.
pub async fn segment_docs_p25(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_p25": null, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let p25_idx = ((n as f64 * 0.25) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "docs_p25": docs_sorted[p25_idx]}))
}

/// GET /api/v1/search/index/segments/bytes-p25 — percentil 25 de disk_bytes dos segmentos. Sprint #1183.
pub async fn segment_bytes_p25(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_p25": null, "segment_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes_sorted.sort_unstable();
    let p25_idx = ((n as f64 * 0.25) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "bytes_p25": bytes_sorted[p25_idx]}))
}

/// GET /api/v1/search/index/segments/docs-p75 — percentil 75 de num_docs dos segmentos. Sprint #1188.
pub async fn segment_docs_p75(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_p75": null, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let p75_idx = ((n as f64 * 0.75) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "docs_p75": docs_sorted[p75_idx]}))
}

/// GET /api/v1/search/index/segments/bytes-p75 — percentil 75 de disk_bytes dos segmentos. Sprint #1193.
pub async fn segment_bytes_p75(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_p75": null, "segment_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes_sorted.sort_unstable();
    let p75_idx = ((n as f64 * 0.75) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "bytes_p75": bytes_sorted[p75_idx]}))
}

/// GET /api/v1/search/index/segments/docs-p90 — percentil 90 de num_docs dos segmentos. Sprint #1198.
pub async fn segment_docs_p90(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_p90": null, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let p90_idx = ((n as f64 * 0.90) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "docs_p90": docs_sorted[p90_idx]}))
}

/// GET /api/v1/search/index/segments/bytes-p90 — percentil 90 de disk_bytes dos segmentos. Sprint #1203.
pub async fn segment_bytes_p90(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_p90": null, "segment_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes_sorted.sort_unstable();
    let p90_idx = ((n as f64 * 0.90) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "bytes_p90": bytes_sorted[p90_idx]}))
}

/// GET /api/v1/search/index/segments/docs-p10 — percentil 10 de num_docs dos segmentos. Sprint #1208.
pub async fn segment_docs_p10(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_p10": null, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let p10_idx = ((n as f64 * 0.10) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "docs_p10": docs_sorted[p10_idx]}))
}

/// GET /api/v1/search/index/segments/bytes-p10 — percentil 10 de disk_bytes dos segmentos. Sprint #1213.
pub async fn segment_bytes_p10(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_p10": null, "segment_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes_sorted.sort_unstable();
    let p10_idx = ((n as f64 * 0.10) as usize).min(n - 1);
    Json(serde_json::json!({"segment_count": n, "bytes_p10": bytes_sorted[p10_idx]}))
}

/// GET /api/v1/search/index/segments/docs-bytes-ratio-min — valor mínimo de (num_docs/disk_bytes) por segmento. Sprint #1128.
pub async fn segment_docs_bytes_ratio_min(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_bytes_ratio_min": null, "segment_count": 0}));
    }
    let min_ratio = segs.iter()
        .filter(|(_, _, db)| *db > 0)
        .map(|(_, nd, db)| *nd as f64 / *db as f64)
        .fold(f64::INFINITY, f64::min);
    let val = if min_ratio == f64::INFINITY { serde_json::Value::Null } else { serde_json::json!(min_ratio) };
    Json(serde_json::json!({"segment_count": n, "docs_bytes_ratio_min": val}))
}

/// GET /api/v1/search/index/segments/docs-bytes-ratio-mean — média de (num_docs/disk_bytes) por segmento. Sprint #1133.
pub async fn segment_docs_bytes_ratio_mean(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_bytes_ratio_mean": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter()
        .filter(|(_, _, db)| *db > 0)
        .map(|(_, nd, db)| *nd as f64 / *db as f64)
        .collect();
    let val = if ratios.is_empty() {
        serde_json::Value::Null
    } else {
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        serde_json::json!(mean)
    };
    Json(serde_json::json!({"segment_count": n, "docs_bytes_ratio_mean": val}))
}

/// GET /api/v1/search/index/segments/large-docs-ratio — fração de num_docs do maior segmento vs total. Sprint #1118.
pub async fn segment_large_docs_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"large_docs_ratio": null, "segment_count": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let max_docs = segs.iter().map(|(_, nd, _)| *nd).max().unwrap_or(0);
    let ratio = if total_docs > 0 { max_docs as f64 / total_docs as f64 } else { 0.0 };
    Json(serde_json::json!({
        "segment_count": n,
        "total_docs": total_docs,
        "max_docs": max_docs,
        "large_docs_ratio": ratio,
    }))
}

/// GET /api/v1/search/index/segments/docs-bytes-ratio-max — valor máximo de (num_docs/disk_bytes) por segmento. Sprint #1123.
pub async fn segment_docs_bytes_ratio_max(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_bytes_ratio_max": null, "segment_count": 0}));
    }
    let max_ratio = segs.iter()
        .filter(|(_, _, db)| *db > 0)
        .map(|(_, nd, db)| *nd as f64 / *db as f64)
        .fold(f64::NEG_INFINITY, f64::max);
    let val = if max_ratio == f64::NEG_INFINITY { serde_json::Value::Null } else { serde_json::json!(max_ratio) };
    Json(serde_json::json!({"segment_count": n, "docs_bytes_ratio_max": val}))
}

/// GET /api/v1/search/index/segments/large-bytes-ratio — fração de disk_bytes do maior segmento vs total. Sprint #1113.
pub async fn segment_large_bytes_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"large_bytes_ratio": null, "segment_count": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let max_bytes = segs.iter().map(|(_, _, db)| *db).max().unwrap_or(0);
    let ratio = if total_bytes > 0 { max_bytes as f64 / total_bytes as f64 } else { 0.0 };
    Json(serde_json::json!({
        "segment_count": n,
        "total_bytes": total_bytes,
        "max_bytes": max_bytes,
        "large_bytes_ratio": ratio,
    }))
}

/// GET /api/v1/search/index/segments/bottom-n-by-docs — bottom-N segmentos por num_docs. Sprint #1108.
pub async fn segment_bottom_n_by_docs(
    State(store): State<IndexStore>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let n = params.get("n").and_then(|v| v.parse::<usize>().ok()).unwrap_or(5).min(100).max(1);
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted: Vec<_> = segs.iter().collect();
    sorted.sort_by_key(|(_, nd, _)| *nd);
    let rows: Vec<serde_json::Value> = sorted.into_iter().take(n)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"n": n, "segment_count": segs.len(), "rows": rows}))
}

/// GET /api/v1/search/index/segments/docs-above-p75 — segmentos com num_docs acima do 75th percentile. Sprint #1103.
pub async fn segment_docs_above_p75(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_docs": null, "rows": [], "count_above": 0, "segment_count": 0}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p75_idx = ((n as f64 * 0.75) as usize).saturating_sub(1).min(n - 1);
    let p75 = sorted_docs[p75_idx];
    let mut above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd > p75)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    above.sort_by(|a, b| b["num_docs"].as_u64().cmp(&a["num_docs"].as_u64()));
    let count_above = above.len();
    Json(serde_json::json!({"p75_docs": p75, "rows": above, "count_above": count_above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/above-p75 — segmentos com disk_bytes acima do 75th percentile.
///
/// Retorna `{p75_bytes,rows:[{id,num_docs,disk_bytes}],count_above,segment_count}`. Sprint #923.
pub async fn segment_above_p75(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_bytes": null, "rows": [], "count_above": 0, "segment_count": 0}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let p75_idx = ((n as f64 * 0.75) as usize).saturating_sub(1).min(n - 1);
    let p75 = sorted_bytes[p75_idx];
    let mut above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db > p75)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    above.sort_by(|a, b| b["disk_bytes"].as_u64().cmp(&a["disk_bytes"].as_u64()));
    let count_above = above.len();
    Json(serde_json::json!({"p75_bytes": p75, "rows": above, "count_above": count_above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-floor — segmento com menor num_docs e valor do piso.
///
/// Retorna `{floor_docs,segment:{id,num_docs,disk_bytes}|null,segment_count}`. Sprint #953.
pub async fn segment_docs_floor(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"floor_docs": null, "segment": null, "segment_count": 0}));
    }
    let min_seg = segs.iter().min_by_key(|(_, nd, _)| *nd);
    let result = min_seg.map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}));
    let floor = min_seg.map(|(_, nd, _)| *nd);
    Json(serde_json::json!({"floor_docs": floor, "segment": result, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/id-length-stats — avg/min/max LENGTH(id) dos segment ids.
///
/// Retorna `{avg_id_length,min_id_length,max_id_length,segment_count}`. Sprint #948.
pub async fn segment_id_length_stats(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_id_length": null, "min_id_length": null, "max_id_length": null, "segment_count": 0}));
    }
    let lengths: Vec<usize> = segs.iter().map(|(id, _, _)| id.len()).collect();
    let min_len = lengths.iter().min().copied().unwrap_or(0);
    let max_len = lengths.iter().max().copied().unwrap_or(0);
    let avg_len = lengths.iter().sum::<usize>() as f64 / n as f64;
    Json(serde_json::json!({
        "avg_id_length": avg_len,
        "min_id_length": min_len,
        "max_id_length": max_len,
        "segment_count": n,
    }))
}

/// GET /api/v1/search/index/segments/docs-sum — soma total de num_docs de todos os segmentos.
///
/// Retorna `{total_docs,segment_count,avg_docs_per_segment}`. Sprint #943.
pub async fn segment_docs_sum(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let total: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let avg = if n > 0 { total as f64 / n as f64 } else { 0.0 };
    Json(serde_json::json!({"total_docs": total, "segment_count": n, "avg_docs_per_segment": avg}))
}

/// GET /api/v1/search/index/segments/docs-density-rank — rank num_docs/disk_bytes (docs_per_byte) DESC.
///
/// Retorna `{rows:[{rank,id,num_docs,disk_bytes,docs_per_byte}]}`. Sprint #938.
pub async fn segment_docs_density_rank(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut ranked: Vec<(String, u64, u64, f64)> = segs.into_iter()
        .map(|(id, nd, db)| {
            let dpb = if db > 0 { nd as f64 / db as f64 } else { 0.0 };
            (id, nd, db, dpb)
        })
        .collect();
    ranked.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
    let rows: Vec<serde_json::Value> = ranked.into_iter().enumerate()
        .map(|(i, (id, nd, db, dpb))| serde_json::json!({
            "rank": i + 1, "id": id, "num_docs": nd, "disk_bytes": db, "docs_per_byte": dpb
        }))
        .collect();
    Json(serde_json::json!({"rows": rows}))
}

/// GET /api/v1/search/index/segments/variance — variância amostral (n-1) de num_docs e disk_bytes.
///
/// Retorna `{docs_variance,bytes_variance,segment_count}`. Sprint #918.
pub async fn segment_variance(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"docs_variance": null, "bytes_variance": null, "segment_count": n}));
    }
    let docs: Vec<f64>  = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let bytes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean_docs  = docs.iter().sum::<f64>()  / n as f64;
    let mean_bytes = bytes.iter().sum::<f64>() / n as f64;
    let var_docs  = docs.iter().map(|x|  (x - mean_docs).powi(2)).sum::<f64>()  / (n - 1) as f64;
    let var_bytes = bytes.iter().map(|x| (x - mean_bytes).powi(2)).sum::<f64>() / (n - 1) as f64;
    Json(serde_json::json!({"docs_variance": var_docs, "bytes_variance": var_bytes, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/median-docs — mediana de num_docs.
///
/// Retorna `{median_docs,segment_count}`. Sprint #913.
pub async fn segment_median_docs(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"median_docs": null, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let median = if n % 2 == 1 { docs[n / 2] as f64 } else { (docs[n / 2 - 1] + docs[n / 2]) as f64 / 2.0 };
    Json(serde_json::json!({"median_docs": median, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/segment-age-rank — rank segmentos por id lexicográfico como proxy de criação.
///
/// Retorna `{rows:[{rank,id,num_docs,disk_bytes}]}` id ASC. Sprint #903.
pub async fn segment_age_rank(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted = segs;
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let rows: Vec<serde_json::Value> = sorted.into_iter().enumerate()
        .map(|(i, (id, nd, db))| serde_json::json!({
            "rank": i + 1, "id": id, "num_docs": nd, "disk_bytes": db
        }))
        .collect();
    Json(serde_json::json!({"rows": rows}))
}

/// GET /api/v1/search/index/segments/top-by-docs — id+num_docs do segmento com mais docs.
///
/// Retorna `{segment: {id,num_docs,disk_bytes} | null}`. Sprint #893.
pub async fn segment_top_by_docs(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let top = segs.into_iter().max_by_key(|(_, nd, _)| *nd);
    let segment = top.map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}));
    Json(serde_json::json!({"segment": segment}))
}

/// GET /api/v1/search/index/segments/size-percentile — percentis 25/50/75/90/95 de disk_bytes.
///
/// Interpolação linear por rank em memória. Retorna `{p25,p50,p75,p90,p95,segment_count}`. Sprint #888.
pub async fn segment_size_percentile(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25": null, "p50": null, "p75": null, "p90": null, "p95": null, "segment_count": 0}));
    }
    let mut sizes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sizes.sort_unstable();

    let percentile = |p: f64| -> f64 {
        let idx = p / 100.0 * (n - 1) as f64;
        let lo = idx.floor() as usize;
        let hi = idx.ceil() as usize;
        let frac = idx - lo as f64;
        sizes[lo] as f64 * (1.0 - frac) + sizes[hi.min(n - 1)] as f64 * frac
    };

    Json(serde_json::json!({
        "p25": percentile(25.0),
        "p50": percentile(50.0),
        "p75": percentile(75.0),
        "p90": percentile(90.0),
        "p95": percentile(95.0),
        "segment_count": n,
    }))
}

/// GET /api/v1/search/index/segments/docs-size-correlation — correlação Pearson entre num_docs e disk_bytes.
///
/// r = (Σxy - n*x̄*ȳ) / (√(Σx² - n*x̄²) * √(Σy² - n*ȳ²)). Retorna `{pearson_r,segment_count}`. Sprint #883.
pub async fn segment_docs_size_correlation(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"pearson_r": serde_json::Value::Null, "segment_count": n}));
    }
    let xs: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let ys: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let nf = n as f64;
    let mean_x = xs.iter().sum::<f64>() / nf;
    let mean_y = ys.iter().sum::<f64>() / nf;
    let sum_xy: f64 = xs.iter().zip(ys.iter()).map(|(x, y)| x * y).sum();
    let sum_x2: f64 = xs.iter().map(|x| x * x).sum();
    let sum_y2: f64 = ys.iter().map(|y| y * y).sum();
    let denom = ((sum_x2 - nf * mean_x * mean_x) * (sum_y2 - nf * mean_y * mean_y)).sqrt();
    let pearson_r = if denom == 0.0 { None } else {
        Some((sum_xy - nf * mean_x * mean_y) / denom)
    };
    Json(serde_json::json!({"pearson_r": pearson_r, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-percentile-band — conta segmentos por faixa percentil de num_docs.
///
/// Divide os segmentos em 4 bandas (p0-p25, p25-p50, p50-p75, p75-p100) e conta quantos caem em cada.
/// Retorna `{segment_count,bands:[{band,min,max,count}]}`. Sprint #878.
pub async fn segment_docs_percentile_band(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segment_count": 0, "bands": []}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();

    let p25 = docs[n * 25 / 100];
    let p50 = docs[n * 50 / 100];
    let p75 = docs[n * 75 / 100];
    let max  = *docs.last().unwrap();
    let min  = docs[0];

    let (mut b0, mut b1, mut b2, mut b3) = (0u64, 0u64, 0u64, 0u64);
    for &d in &docs {
        if d <= p25      { b0 += 1; }
        else if d <= p50 { b1 += 1; }
        else if d <= p75 { b2 += 1; }
        else             { b3 += 1; }
    }
    Json(serde_json::json!({
        "segment_count": n,
        "bands": [
            {"band": "p0-p25",   "threshold_docs": p25, "count": b0},
            {"band": "p25-p50",  "threshold_docs": p50, "count": b1},
            {"band": "p50-p75",  "threshold_docs": p75, "count": b2},
            {"band": "p75-p100", "threshold_docs": max,  "count": b3},
        ],
        "global_min_docs": min,
        "global_max_docs": max,
    }))
}

/// GET /api/v1/search/index/segments/bytes-ceiling — segmento com maior disk_bytes.
///
/// Retorna `{segment:{id,num_docs,disk_bytes}|null,segment_count}`. Sprint #958.
pub async fn segment_bytes_ceiling(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let top = segs.into_iter().max_by_key(|(_, _, db)| *db);
    let segment = top.map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}));
    Json(serde_json::json!({"segment": segment, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-mean — segmentos com num_docs > média.
///
/// Retorna `{mean_docs,rows:[{id,num_docs,disk_bytes}],count_above,segment_count}`. Sprint #963.
pub async fn segment_docs_above_mean(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mean_docs": null, "rows": [], "count_above": 0, "segment_count": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let mean = total_docs as f64 / n as f64;
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd as f64 > mean)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    let count_above = above.len();
    Json(serde_json::json!({"mean_docs": mean, "rows": above, "count_above": count_above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-mean — segmentos com disk_bytes > média.
///
/// Retorna `{mean_bytes,rows:[{id,num_docs,disk_bytes}],count_above,segment_count}`. Sprint #968.
pub async fn segment_bytes_above_mean(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mean_bytes": null, "rows": [], "count_above": 0, "segment_count": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let mean = total_bytes as f64 / n as f64;
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db as f64 > mean)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    let count_above = above.len();
    Json(serde_json::json!({"mean_bytes": mean, "rows": above, "count_above": count_above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-above-mean — alias semântico para bytes-above-mean.
///
/// Retorna `{mean_bytes,rows:[{id,num_docs,disk_bytes}],count_above,segment_count}`. Sprint #973.
pub async fn segment_size_above_mean(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    segment_bytes_above_mean(State(store)).await
}

/// GET /api/v1/search/index/segments/bytes-floor — segmento com menor disk_bytes.
///
/// Retorna `{segment:{id,num_docs,disk_bytes}|null,segment_count}`. Sprint #978.
pub async fn segment_bytes_floor(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let bottom = segs.into_iter().min_by_key(|(_, _, db)| *db);
    let segment = bottom.map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}));
    Json(serde_json::json!({"segment": segment, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-median-deviation — |num_docs − mediana| por segmento.
///
/// Retorna `{median_docs,rows:[{id,num_docs,disk_bytes,deviation}],segment_count}`. Sprint #983.
pub async fn segment_docs_median_deviation(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"median_docs": null, "rows": [], "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let median = if n % 2 == 1 {
        docs_sorted[n / 2] as f64
    } else {
        (docs_sorted[n / 2 - 1] + docs_sorted[n / 2]) as f64 / 2.0
    };
    let rows: Vec<serde_json::Value> = segs.iter()
        .map(|(id, nd, db)| {
            let dev = (*nd as f64 - median).abs();
            serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "deviation": dev})
        })
        .collect();
    Json(serde_json::json!({"median_docs": median, "rows": rows, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-median — mediana de disk_bytes.
///
/// Retorna `{median_bytes,segment_count}`. Sprint #988.
pub async fn segment_size_median(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"median_bytes": null, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let median = if n % 2 == 1 {
        bytes[n / 2] as f64
    } else {
        (bytes[n / 2 - 1] + bytes[n / 2]) as f64 / 2.0
    };
    Json(serde_json::json!({"median_bytes": median, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/top-n-by-bytes?limit=N — top-N segmentos por disk_bytes DESC.
///
/// Default limit=5 max=50. Sprint #993.
pub async fn segment_top_n_by_bytes(
    State(store): State<IndexStore>,
    Query(q):     Query<StatsLimitQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(5).clamp(1, 50) as usize;
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted = segs;
    sorted.sort_by(|a, b| b.2.cmp(&a.2));
    sorted.truncate(limit);
    let rows: Vec<serde_json::Value> = sorted.into_iter()
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"rows": rows, "limit": limit}))
}

/// GET /api/v1/search/index/segments/docs-range — max_docs − min_docs amplitude. Sprint #1007.
pub async fn segment_docs_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() {
        return Json(serde_json::json!({"docs_range": null}));
    }
    let min = segs.iter().map(|(_, nd, _)| *nd).min().unwrap_or(0);
    let max = segs.iter().map(|(_, nd, _)| *nd).max().unwrap_or(0);
    Json(serde_json::json!({"min_docs": min, "max_docs": max, "docs_range": max - min}))
}

/// GET /api/v1/search/index/segments/count-by-size-band — histogram tiny/small/medium/large/xlarge by disk_bytes. Sprint #1008.
pub async fn segment_count_by_size_band(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut tiny: u64 = 0;   // < 64 KB
    let mut small: u64 = 0;  // 64 KB – 1 MB
    let mut medium: u64 = 0; // 1 MB – 16 MB
    let mut large: u64 = 0;  // 16 MB – 256 MB
    let mut xlarge: u64 = 0; // ≥ 256 MB
    for (_, _, db) in &segs {
        match *db {
            b if b < 65_536             => tiny   += 1,
            b if b < 1_048_576          => small  += 1,
            b if b < 16_777_216         => medium += 1,
            b if b < 268_435_456        => large  += 1,
            _                           => xlarge += 1,
        }
    }
    Json(serde_json::json!({
        "total_segments": segs.len(),
        "tiny_lt64kb": tiny,
        "small_64kb_1mb": small,
        "medium_1mb_16mb": medium,
        "large_16mb_256mb": large,
        "xlarge_gte256mb": xlarge,
    }))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-range — max−min bytes/doc amplitude. Sprint #1009.
pub async fn segment_bytes_per_doc_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    if ratios.is_empty() {
        return Json(serde_json::json!({"bytes_per_doc_range": null}));
    }
    let min = ratios.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = ratios.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    Json(serde_json::json!({"min_bytes_per_doc": min, "max_bytes_per_doc": max, "bytes_per_doc_range": max - min}))
}

/// GET /api/v1/search/index/segments/docs-stdev — population stdev of num_docs across segments. Sprint #1010.
pub async fn segment_docs_stdev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"docs_stdev": null, "segment_count": n}));
    }
    let mean = segs.iter().map(|(_, nd, _)| *nd as f64).sum::<f64>() / n as f64;
    let variance = segs.iter().map(|(_, nd, _)| { let d = *nd as f64 - mean; d * d }).sum::<f64>() / n as f64;
    let stdev = variance.sqrt();
    Json(serde_json::json!({"mean_docs": mean, "docs_stdev": stdev, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-stdev — population stdev de disk_bytes. Sprint #1027.
pub async fn segment_bytes_stdev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"bytes_stdev": null, "segment_count": n}));
    }
    let mean = segs.iter().map(|(_, _, db)| *db as f64).sum::<f64>() / n as f64;
    let variance = segs.iter().map(|(_, _, db)| { let d = *db as f64 - mean; d * d }).sum::<f64>() / n as f64;
    Json(serde_json::json!({"mean_bytes": mean, "bytes_stdev": variance.sqrt(), "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-cv — coefficient of variation of num_docs (stdev/mean). Sprint #1028.
pub async fn segment_docs_cv(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"docs_cv": null, "segment_count": n}));
    }
    let mean = segs.iter().map(|(_, nd, _)| *nd as f64).sum::<f64>() / n as f64;
    if mean == 0.0 {
        return Json(serde_json::json!({"docs_cv": null, "segment_count": n}));
    }
    let variance = segs.iter().map(|(_, nd, _)| { let d = *nd as f64 - mean; d * d }).sum::<f64>() / n as f64;
    let cv = variance.sqrt() / mean;
    Json(serde_json::json!({"mean_docs": mean, "docs_stdev": variance.sqrt(), "docs_cv": cv, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-range — max_bytes − min_bytes amplitude. Sprint #1098.
pub async fn segment_bytes_range(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_range": null, "segment_count": 0}));
    }
    let min = segs.iter().map(|(_, _, db)| *db).min().unwrap_or(0);
    let max = segs.iter().map(|(_, _, db)| *db).max().unwrap_or(0);
    Json(serde_json::json!({
        "segment_count": n,
        "disk_bytes_min": min,
        "disk_bytes_max": max,
        "bytes_range": max.saturating_sub(min),
    }))
}

/// GET /api/v1/search/index/segments/bytes-cv — coefficient of variation of disk_bytes (stdev/mean). Sprint #1029.
pub async fn segment_bytes_cv(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"bytes_cv": null, "segment_count": n}));
    }
    let mean = segs.iter().map(|(_, _, db)| *db as f64).sum::<f64>() / n as f64;
    if mean == 0.0 {
        return Json(serde_json::json!({"bytes_cv": null, "segment_count": n}));
    }
    let variance = segs.iter().map(|(_, _, db)| { let d = *db as f64 - mean; d * d }).sum::<f64>() / n as f64;
    let cv = variance.sqrt() / mean;
    Json(serde_json::json!({"mean_bytes": mean, "bytes_stdev": variance.sqrt(), "bytes_cv": cv, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-bytes-ratio-stats — stats do ratio num_docs/disk_bytes por segmento. Sprint #1030.
pub async fn segment_docs_bytes_ratio_stats(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<f64> = segs.iter()
        .filter(|(_, _, db)| *db > 0)
        .map(|(_, nd, db)| *nd as f64 / *db as f64)
        .collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_bytes_ratio_stats": null}));
    }
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let min = ratios.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = ratios.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let variance = ratios.iter().map(|r| { let d = r - mean; d * d }).sum::<f64>() / n as f64;
    Json(serde_json::json!({
        "segment_count": n,
        "mean_docs_per_byte": mean,
        "min_docs_per_byte": min,
        "max_docs_per_byte": max,
        "stdev_docs_per_byte": variance.sqrt(),
    }))
}

/// GET /api/v1/search/index/segments/bytes-iqr — IQR de disk_bytes (Q3−Q1). Sprint #1047.
pub async fn segment_bytes_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"bytes_iqr": null, "segment_count": n}));
    }
    let mut sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted.sort_unstable();
    let q1_idx = ((n as f64 - 1.0) * 0.25) as usize;
    let q3_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let q1 = sorted[q1_idx] as f64;
    let q3 = sorted[q3_idx.min(n - 1)] as f64;
    Json(serde_json::json!({"q1_bytes": q1, "q3_bytes": q3, "bytes_iqr": q3 - q1, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-iqr — IQR de num_docs (Q3−Q1). Sprint #1048.
pub async fn segment_docs_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"docs_iqr": null, "segment_count": n}));
    }
    let mut sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted.sort_unstable();
    let q1_idx = ((n as f64 - 1.0) * 0.25) as usize;
    let q3_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let q1 = sorted[q1_idx] as f64;
    let q3 = sorted[q3_idx.min(n - 1)] as f64;
    Json(serde_json::json!({"q1_docs": q1, "q3_docs": q3, "docs_iqr": q3 - q1, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/top-n-by-docs — top-N segmentos por num_docs. Sprint #1049.
pub async fn segment_top_n_by_docs(
    State(store): State<IndexStore>,
    Query(q):     Query<StatsLimitQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(5).clamp(1, 50) as usize;
    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_by(|a, b| b.1.cmp(&a.1));
    segs.truncate(limit);
    let rows: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"rows": rows, "limit": limit}))
}

/// GET /api/v1/search/index/segments/total-size — SUM total disk_bytes + total num_docs do índice. Sprint #1067.
pub async fn segment_total_size(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let total_bytes: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let total_docs:  u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    Json(serde_json::json!({
        "segment_count": n,
        "total_docs":    total_docs,
        "total_bytes":   total_bytes,
    }))
}

/// GET /api/v1/search/index/segments/bytes-above-p90 — segmentos com disk_bytes > P90. Sprint #1068.
pub async fn segment_bytes_above_p90(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p90_bytes": null, "above_p90": [], "segment_count": n}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted_bytes[p90_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db > p90)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p90_bytes": p90, "above_p90": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p90 — segmentos com num_docs > P90. Sprint #1069.
pub async fn segment_docs_above_p90(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p90_docs": null, "above_p90": [], "segment_count": n}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted_docs[p90_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd > p90)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p90_docs": p90, "above_p90": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/total-docs — soma total de num_docs em todos os segmentos. Sprint #1243.
pub async fn segment_total_docs(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    Json(serde_json::json!({"total_docs": total, "segment_count": segs.len()}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-median — mediana de bytes/doc dos segmentos. Sprint #1238.
pub async fn segment_bytes_per_doc_median(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_median": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.is_empty() {
        serde_json::Value::Null
    } else {
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = (ratios.len() - 1) / 2;
        serde_json::json!(ratios[mid])
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_median": val}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p90 — percentil 90 de bytes/doc dos segmentos. Sprint #1233.
pub async fn segment_bytes_per_doc_p90(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_p90": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.is_empty() {
        serde_json::Value::Null
    } else {
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p90_idx = ((ratios.len() as f64 * 0.90) as usize).min(ratios.len() - 1);
        serde_json::json!(ratios[p90_idx])
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_p90": val}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p10 — percentil 10 de bytes/doc dos segmentos. Sprint #1228.
pub async fn segment_bytes_per_doc_p10(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_p10": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.is_empty() {
        serde_json::Value::Null
    } else {
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p10_idx = ((ratios.len() as f64 * 0.10) as usize).min(ratios.len() - 1);
        serde_json::json!(ratios[p10_idx])
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_p10": val}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p25 — percentil 25 de bytes/doc dos segmentos. Sprint #1223.
pub async fn segment_bytes_per_doc_p25(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_p25": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, db)| *db as f64 / *nd as f64)
        .collect();
    let val = if ratios.is_empty() {
        serde_json::Value::Null
    } else {
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p25_idx = ((ratios.len() as f64 * 0.25) as usize).min(ratios.len() - 1);
        serde_json::json!(ratios[p25_idx])
    };
    Json(serde_json::json!({"segment_count": n, "bytes_per_doc_p25": val}))
}

/// GET /api/v1/search/index/segments/docs-above-p90-count — contagem numérica de segmentos com num_docs > P90. Sprint #1218.
pub async fn segment_docs_above_p90_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p90_docs": null, "above_p90_count": 0, "segment_count": n}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted_docs[p90_idx.min(n - 1)];
    let count = segs.iter().filter(|(_, nd, _)| *nd > p90).count();
    Json(serde_json::json!({"p90_docs": p90, "above_p90_count": count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/large-ratio — ratio segmentos > mediana disk_bytes / total. Sprint #1070.
pub async fn segment_large_ratio(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"large_ratio": null, "segment_count": 0}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let median_idx = (n - 1) / 2;
    let median = sorted_bytes[median_idx];
    let large_count = segs.iter().filter(|(_, _, db)| *db > median).count();
    let ratio = large_count as f64 / n as f64;
    Json(serde_json::json!({
        "median_bytes":  median,
        "large_count":   large_count,
        "total_count":   n,
        "large_ratio":   ratio,
    }))
}

/// GET /api/v1/search/index/segments/bytes-max — segmento com maior disk_bytes. Sprint #1093.
pub async fn segment_bytes_max(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() {
        return Json(serde_json::json!({"segment": null}));
    }
    let max = segs.into_iter().max_by_key(|(_, _, db)| *db);
    if let Some((id, nd, db)) = max {
        Json(serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
    } else {
        Json(serde_json::json!({"segment": null}))
    }
}

/// GET /api/v1/search/index/segments/bytes-min — segmento com menor disk_bytes. Sprint #1083.
pub async fn segment_bytes_min(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() {
        return Json(serde_json::json!({"segment": null}));
    }
    let min = segs.into_iter().min_by_key(|(_, _, db)| *db);
    if let Some((id, nd, db)) = min {
        Json(serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
    } else {
        Json(serde_json::json!({"segment": null}))
    }
}

/// GET /api/v1/search/index/segments/docs-max — segmento com maior num_docs. Sprint #1088.
pub async fn segment_docs_max(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() {
        return Json(serde_json::json!({"segment": null}));
    }
    let max = segs.into_iter().max_by_key(|(_, nd, _)| *nd);
    if let Some((id, nd, db)) = max {
        Json(serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
    } else {
        Json(serde_json::json!({"segment": null}))
    }
}

/// GET /api/v1/search/index/segments/docs-min — segmento com menor num_docs. Sprint #1078.
pub async fn segment_docs_min(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() {
        return Json(serde_json::json!({"segment": null}));
    }
    let min = segs.into_iter().min_by_key(|(_, nd, _)| *nd);
    if let Some((id, nd, db)) = min {
        Json(serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
    } else {
        Json(serde_json::json!({"segment": null}))
    }
}

/// GET /api/v1/search/index/segments/bottom-n-by-bytes — bottom-N segmentos por disk_bytes. Sprint #1050.
pub async fn segment_bottom_n_by_bytes(
    State(store): State<IndexStore>,
    Query(q):     Query<StatsLimitQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(5).clamp(1, 50) as usize;
    let mut segs = store.list_segments().unwrap_or_default();
    segs.sort_by(|a, b| a.2.cmp(&b.2));
    segs.truncate(limit);
    let rows: Vec<serde_json::Value> = segs.into_iter()
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"rows": rows, "limit": limit}))
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

/// GET /api/v1/search/index/segments/total-bytes — soma total de disk_bytes em todos os segmentos. Sprint #1248.
pub async fn segment_total_bytes(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    Json(serde_json::json!({"total_bytes": total, "segment_count": segs.len()}))
}

/// GET /api/v1/search/index/segments/avg-docs-per-segment — média de num_docs por segmento. Sprint #1253.
pub async fn segment_avg_docs_per_segment(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_docs_per_segment": null, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let avg = total as f64 / n as f64;
    Json(serde_json::json!({"avg_docs_per_segment": avg, "total_docs": total, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum — soma total de disk_bytes em todos os segmentos (alias detalhado). Sprint #1258.
pub async fn segment_bytes_sum(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let n = segs.len();
    let avg = if n == 0 { 0.0 } else { total as f64 / n as f64 };
    Json(serde_json::json!({"bytes_sum": total, "segment_count": n, "avg_bytes_per_segment": avg}))
}

/// GET /api/v1/search/index/segments/docs-bytes-product — produto (num_docs × disk_bytes) por segmento. Sprint #1263.
pub async fn segment_docs_bytes_product(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let rows: Vec<serde_json::Value> = segs.iter()
        .map(|(name, nd, db)| {
            let product = (*nd as u64).saturating_mul(*db);
            serde_json::json!({"segment": name, "num_docs": nd, "disk_bytes": db, "product": product})
        })
        .collect();
    let total_product: u64 = segs.iter().map(|(_, nd, db)| (*nd as u64).saturating_mul(*db)).sum();
    Json(serde_json::json!({"segments": rows, "total_product": total_product, "segment_count": segs.len()}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p99 — 99th percentile de bytes/doc nos segmentos. Sprint #1278.
pub async fn segment_bytes_per_doc_p99(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_p99": null, "segment_count": 0}));
    }
    let mut vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *nd == 0 { 0.0 } else { *db as f64 / *nd as f64 })
        .collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"bytes_per_doc_p99": vals[idx], "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-variance — variância de num_docs nos segmentos. Sprint #1283.
pub async fn segment_docs_variance(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_variance": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"docs_variance": variance, "docs_mean": mean, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-variance — variância de disk_bytes nos segmentos. Sprint #1288.
pub async fn segment_bytes_variance(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_variance": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_variance": variance, "bytes_mean": mean, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-variance — variância combinada de (num_docs + disk_bytes) normalizada. Sprint #1293.
pub async fn segment_size_variance(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"size_variance": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, nd, db)| (*nd as f64) + (*db as f64)).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"size_variance": variance, "size_mean": mean, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-variance — variância de bytes/doc nos segmentos. Sprint #1298.
pub async fn segment_bytes_per_doc_variance(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_variance": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *nd == 0 { 0.0 } else { *db as f64 / *nd as f64 })
        .collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_per_doc_variance": variance, "bytes_per_doc_mean": mean, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-entropy — entropia de Shannon de num_docs. Sprint #1303.
pub async fn segment_docs_entropy(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_entropy": null, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    if total == 0 {
        return Json(serde_json::json!({"docs_entropy": 0.0, "segment_count": n}));
    }
    let entropy: f64 = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(_, nd, _)| {
            let p = *nd as f64 / total as f64;
            -p * p.ln()
        })
        .sum();
    Json(serde_json::json!({"docs_entropy": entropy, "total_docs": total, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-entropy — entropia de Shannon de disk_bytes. Sprint #1308.
pub async fn segment_bytes_entropy(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_entropy": null, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    if total == 0 {
        return Json(serde_json::json!({"bytes_entropy": 0.0, "segment_count": n}));
    }
    let entropy: f64 = segs.iter()
        .filter(|(_, _, db)| *db > 0)
        .map(|(_, _, db)| {
            let p = *db as f64 / total as f64;
            -p * p.ln()
        })
        .sum();
    Json(serde_json::json!({"bytes_entropy": entropy, "total_bytes": total, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-herfindahl — índice HHI de concentração de disk_bytes. Sprint #1373.
pub async fn segment_bytes_herfindahl(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_herfindahl": null, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    if total == 0 {
        return Json(serde_json::json!({"bytes_herfindahl": 0.0, "segment_count": n}));
    }
    let hhi: f64 = segs.iter()
        .map(|(_, _, db)| { let s = *db as f64 / total as f64; s * s })
        .sum();
    Json(serde_json::json!({"bytes_herfindahl": hhi, "segment_count": n, "total_bytes": total}))
}

/// GET /api/v1/search/index/segments/docs-herfindahl — índice HHI de concentração de num_docs. Sprint #1368.
pub async fn segment_docs_herfindahl(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_herfindahl": null, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    if total == 0 {
        return Json(serde_json::json!({"docs_herfindahl": 0.0, "segment_count": n}));
    }
    let hhi: f64 = segs.iter()
        .map(|(_, nd, _)| { let s = *nd as f64 / total as f64; s * s })
        .sum();
    Json(serde_json::json!({"docs_herfindahl": hhi, "segment_count": n, "total_docs": total}))
}

/// GET /api/v1/search/index/segments/count-gini — coeficiente de Gini combinado (docs+bytes) normalizado. Sprint #1363.
pub async fn segment_count_gini(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_gini": null, "segment_count": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, nd, db)| *nd as f64 + *db as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"count_gini": 0.0, "segment_count": n}));
    }
    let gini_num: f64 = vals.iter().enumerate().map(|(i, v)| {
        (2.0 * (i as f64 + 1.0) - n as f64 - 1.0) * v
    }).sum();
    let gini = gini_num / (n as f64 * total);
    Json(serde_json::json!({"count_gini": gini, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-gini — coeficiente de Gini de bytes/doc nos segmentos. Sprint #1358.
pub async fn segment_bytes_per_doc_gini(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_gini": null, "segment_count": 0}));
    }
    let mut vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *nd == 0 { 0.0 } else { *db as f64 / *nd as f64 })
        .collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"bytes_per_doc_gini": 0.0, "segment_count": n}));
    }
    let gini_num: f64 = vals.iter().enumerate().map(|(i, v)| {
        (2.0 * (i as f64 + 1.0) - n as f64 - 1.0) * v
    }).sum();
    let gini = gini_num / (n as f64 * total);
    Json(serde_json::json!({"bytes_per_doc_gini": gini, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-gini — coeficiente de Gini de disk_bytes nos segmentos. Sprint #1353.
pub async fn segment_bytes_gini(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_gini": null, "segment_count": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"bytes_gini": 0.0, "segment_count": n}));
    }
    let gini_num: f64 = vals.iter().enumerate().map(|(i, v)| {
        (2.0 * (i as f64 + 1.0) - n as f64 - 1.0) * v
    }).sum();
    let gini = gini_num / (n as f64 * total);
    Json(serde_json::json!({"bytes_gini": gini, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-gini — coeficiente de Gini de num_docs nos segmentos. Sprint #1348.
pub async fn segment_docs_gini(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_gini": null, "segment_count": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"docs_gini": 0.0, "segment_count": n}));
    }
    let mut cum_sum = 0.0f64;
    let gini_num: f64 = vals.iter().enumerate().map(|(i, v)| {
        cum_sum += v;
        (2.0 * (i as f64 + 1.0) - n as f64 - 1.0) * v
    }).sum();
    let gini = gini_num / (n as f64 * total);
    Json(serde_json::json!({"docs_gini": gini, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-kurtosis — curtose (excess kurtosis) de bytes/doc nos segmentos. Sprint #1343.
pub async fn segment_bytes_per_doc_kurtosis(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"bytes_per_doc_kurtosis": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *nd == 0 { 0.0 } else { *db as f64 / *nd as f64 })
        .collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    if variance == 0.0 {
        return Json(serde_json::json!({"bytes_per_doc_kurtosis": 0.0, "segment_count": n}));
    }
    let kurtosis = vals.iter().map(|v| ((v - mean) / variance.sqrt()).powi(4)).sum::<f64>() / n as f64 - 3.0;
    Json(serde_json::json!({"bytes_per_doc_kurtosis": kurtosis, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-skewness — skewness de bytes/doc nos segmentos. Sprint #1338.
pub async fn segment_bytes_per_doc_skewness(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"bytes_per_doc_skewness": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *nd == 0 { 0.0 } else { *db as f64 / *nd as f64 })
        .collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    if std_dev == 0.0 {
        return Json(serde_json::json!({"bytes_per_doc_skewness": 0.0, "segment_count": n}));
    }
    let skewness = vals.iter().map(|v| ((v - mean) / std_dev).powi(3)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_per_doc_skewness": skewness, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-skewness — assimetria (skewness) de disk_bytes nos segmentos. Sprint #1323.
pub async fn segment_bytes_skewness(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"bytes_skewness": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    if std_dev == 0.0 {
        return Json(serde_json::json!({"bytes_skewness": 0.0, "segment_count": n}));
    }
    let skewness = vals.iter().map(|v| ((v - mean) / std_dev).powi(3)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_skewness": skewness, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-kurtosis — curtose (excess kurtosis) de disk_bytes nos segmentos. Sprint #1333.
pub async fn segment_bytes_kurtosis(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"bytes_kurtosis": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    if variance == 0.0 {
        return Json(serde_json::json!({"bytes_kurtosis": 0.0, "segment_count": n}));
    }
    let kurtosis = vals.iter().map(|v| ((v - mean) / variance.sqrt()).powi(4)).sum::<f64>() / n as f64 - 3.0;
    Json(serde_json::json!({"bytes_kurtosis": kurtosis, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-kurtosis — curtose (excess kurtosis) de num_docs nos segmentos. Sprint #1328.
pub async fn segment_docs_kurtosis(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"docs_kurtosis": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    if variance == 0.0 {
        return Json(serde_json::json!({"docs_kurtosis": 0.0, "segment_count": n}));
    }
    let kurtosis = vals.iter().map(|v| ((v - mean) / variance.sqrt()).powi(4)).sum::<f64>() / n as f64 - 3.0;
    Json(serde_json::json!({"docs_kurtosis": kurtosis, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-skewness — assimetria (skewness) de num_docs nos segmentos. Sprint #1318.
pub async fn segment_docs_skewness(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"docs_skewness": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    if std_dev == 0.0 {
        return Json(serde_json::json!({"docs_skewness": 0.0, "segment_count": n}));
    }
    let skewness = vals.iter().map(|v| ((v - mean) / std_dev).powi(3)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"docs_skewness": skewness, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-entropy — entropia de Shannon de bytes/doc. Sprint #1313.
pub async fn segment_bytes_per_doc_entropy(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_entropy": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *nd == 0 { 0.0 } else { *db as f64 / *nd as f64 })
        .collect();
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"bytes_per_doc_entropy": 0.0, "segment_count": n}));
    }
    let entropy: f64 = vals.iter()
        .filter(|&&v| v > 0.0)
        .map(|&v| { let p = v / total; -p * p.ln() })
        .sum();
    Json(serde_json::json!({"bytes_per_doc_entropy": entropy, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-p99 — 99th percentile de disk_bytes nos segmentos. Sprint #1268.
pub async fn segment_bytes_p99(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_p99": null, "segment_count": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"bytes_p99": vals[idx], "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-p99 — 99th percentile de num_docs nos segmentos. Sprint #1273.
pub async fn segment_docs_p99(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_p99": null, "segment_count": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"docs_p99": vals[idx], "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-p95-count — contagem de segmentos acima do P95 de disk_bytes. Sprint #1443.
pub async fn segment_bytes_p95_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_bytes": null, "above_p95_count": 0, "segment_count": n}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted_bytes[p95_idx.min(n - 1)];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p95).count();
    Json(serde_json::json!({"p95_bytes": p95, "above_p95_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-p95-count — contagem de segmentos acima do P95 de num_docs. Sprint #1448.
pub async fn segment_docs_p95_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_docs": null, "above_p95_count": 0, "segment_count": n}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted_docs[p95_idx.min(n - 1)];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p95).count();
    Json(serde_json::json!({"p95_docs": p95, "above_p95_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-p95-count — contagem de segmentos acima do P95 de (num_docs+disk_bytes). Sprint #1453.
pub async fn segment_count_p95_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_count": null, "above_p95_count": 0, "segment_count": n}));
    }
    let mut sorted_counts: Vec<u64> = segs.iter().map(|(_, nd, db)| nd + db).collect();
    sorted_counts.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted_counts[p95_idx.min(n - 1)];
    let above_count = segs.iter().filter(|(_, nd, db)| nd + db > p95).count();
    Json(serde_json::json!({"p95_count": p95, "above_p95_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-above-p95 — segmentos com bytes/doc > P95. Sprint #1458.
pub async fn segment_bytes_per_doc_above_p95(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_bytes_per_doc": null, "above_p95": [], "segment_count": n}));
    }
    let bpd: Vec<(String, u64, u64, u64)> = segs.iter()
        .map(|(id, nd, db)| (id.clone(), *nd, *db, if *nd > 0 { db / nd } else { 0 }))
        .collect();
    let mut sorted_bpd: Vec<u64> = bpd.iter().map(|(_, _, _, v)| *v).collect();
    sorted_bpd.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted_bpd[p95_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = bpd.iter()
        .filter(|(_, _, _, v)| *v > p95)
        .map(|(id, nd, db, v)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": v}))
        .collect();
    Json(serde_json::json!({"p95_bytes_per_doc": p95, "above_p95": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p95-count — contagem de segmentos acima do P95 de bytes/doc. Sprint #1468.
pub async fn segment_bytes_per_doc_p95_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_bytes_per_doc": null, "above_p95_count": 0, "segment_count": n}));
    }
    let bpd: Vec<u64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { db / nd } else { 0 }).collect();
    let mut sorted = bpd.clone();
    sorted.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted[p95_idx.min(n - 1)];
    let above_count = bpd.iter().filter(|&&v| v > p95).count();
    Json(serde_json::json!({"p95_bytes_per_doc": p95, "above_p95_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p99-count — contagem de segmentos com num_docs > P99. Sprint #1473.
pub async fn segment_docs_above_p99_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_docs": null, "above_p99_count": 0, "segment_count": n}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted_docs[p99_idx.min(n - 1)];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p99).count();
    Json(serde_json::json!({"p99_docs": p99, "above_p99_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-p99-count — contagem de segmentos com disk_bytes > P99. Sprint #1478.
pub async fn segment_bytes_above_p99_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_bytes": null, "above_p99_count": 0, "segment_count": n}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted_bytes[p99_idx.min(n - 1)];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p99).count();
    Json(serde_json::json!({"p99_bytes": p99, "above_p99_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-above-p99-count — contagem de segmentos com bytes/doc > P99. Sprint #1483.
pub async fn segment_bytes_per_doc_above_p99_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_bytes_per_doc": null, "above_p99_count": 0, "segment_count": n}));
    }
    let bpd: Vec<u64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { db / nd } else { 0 }).collect();
    let mut sorted = bpd.clone();
    sorted.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted[p99_idx.min(n - 1)];
    let above_count = bpd.iter().filter(|&&v| v > p99).count();
    Json(serde_json::json!({"p99_bytes_per_doc": p99, "above_p99_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-above-p99 — segmentos com (num_docs+disk_bytes) > P99. Sprint #1463.
pub async fn segment_count_above_p99(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_count": null, "above_p99": [], "segment_count": n}));
    }
    let mut sorted_counts: Vec<u64> = segs.iter().map(|(_, nd, db)| nd + db).collect();
    sorted_counts.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted_counts[p99_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, db)| nd + db > p99)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "count": nd + db}))
        .collect();
    Json(serde_json::json!({"p99_count": p99, "above_p99": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-p95 — segmentos com disk_bytes > P95. Sprint #1438.
pub async fn segment_bytes_above_p95(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_bytes": null, "above_p95": [], "segment_count": n}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted_bytes[p95_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db > p95)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p95_bytes": p95, "above_p95": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p95 — segmentos com num_docs > P95. Sprint #1433.
pub async fn segment_docs_above_p95(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_docs": null, "above_p95": [], "segment_count": n}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted_docs[p95_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd > p95)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p95_docs": p95, "above_p95": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-kurtosis — curtose (excess kurtosis) de (num_docs + disk_bytes). Sprint #1428.
pub async fn segment_count_kurtosis(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"count_kurtosis": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, nd, db)| *nd as f64 + *db as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    if variance == 0.0 {
        return Json(serde_json::json!({"count_kurtosis": 0.0, "segment_count": n}));
    }
    let kurtosis = vals.iter().map(|v| ((v - mean) / variance.sqrt()).powi(4)).sum::<f64>() / n as f64 - 3.0;
    Json(serde_json::json!({"count_kurtosis": kurtosis, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-skewness — skewness de (num_docs + disk_bytes) nos segmentos. Sprint #1423.
pub async fn segment_count_skewness(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"count_skewness": null, "segment_count": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, nd, db)| *nd as f64 + *db as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    if std_dev == 0.0 {
        return Json(serde_json::json!({"count_skewness": 0.0, "segment_count": n}));
    }
    let skewness = vals.iter().map(|v| ((v - mean) / std_dev).powi(3)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"count_skewness": skewness, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-p99-count — contagem de segmentos acima do P99 de num_docs. Sprint #1418.
pub async fn segment_docs_p99_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_docs": null, "above_p99_count": 0, "segment_count": n}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted_docs[p99_idx.min(n - 1)];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p99).count();
    Json(serde_json::json!({"p99_docs": p99, "above_p99_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-p99-count — contagem de segmentos acima do P99 de disk_bytes. Sprint #1413.
pub async fn segment_bytes_p99_count(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_bytes": null, "above_p99_count": 0, "segment_count": n}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted_bytes[p99_idx.min(n - 1)];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p99).count();
    Json(serde_json::json!({"p99_bytes": p99, "above_p99_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-p99 — segmentos com disk_bytes > P99. Sprint #1408.
pub async fn segment_bytes_above_p99(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_bytes": null, "above_p99": [], "segment_count": n}));
    }
    let mut sorted_bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sorted_bytes.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted_bytes[p99_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db > p99)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p99_bytes": p99, "above_p99": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p99 — segmentos com num_docs > P99. Sprint #1403.
pub async fn segment_docs_above_p99(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_docs": null, "above_p99": [], "segment_count": n}));
    }
    let mut sorted_docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    sorted_docs.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted_docs[p99_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd > p99)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p99_docs": p99, "above_p99": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-herfindahl — índice HHI de concentração de (num_docs + disk_bytes). Sprint #1398.
pub async fn segment_count_herfindahl(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_herfindahl": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| *nd as f64 + *db as f64)
        .collect();
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"count_herfindahl": 0.0, "segment_count": n}));
    }
    let hhi: f64 = vals.iter().map(|&x| { let s = x / total; s * s }).sum();
    Json(serde_json::json!({"count_herfindahl": hhi, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-theil — índice de Theil (T0) de (num_docs + disk_bytes) normalizado por segmento. Sprint #1393.
pub async fn segment_count_theil(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_theil": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| *nd as f64 + *db as f64)
        .collect();
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"count_theil": 0.0, "segment_count": n}));
    }
    let mean = total / n as f64;
    let theil: f64 = vals.iter()
        .map(|&x| if x == 0.0 { 0.0 } else { (x / mean) * (x / mean).ln() })
        .sum::<f64>() / n as f64;
    Json(serde_json::json!({"count_theil": theil, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-theil — índice de Theil (T0) do ratio disk_bytes/num_docs. Sprint #1388.
pub async fn segment_bytes_per_doc_theil(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_theil": null, "segment_count": 0}));
    }
    let vals: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *nd == 0 { 0.0 } else { *db as f64 / *nd as f64 })
        .collect();
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"bytes_per_doc_theil": 0.0, "segment_count": n}));
    }
    let mean = total / n as f64;
    let theil: f64 = vals.iter()
        .map(|&x| if x == 0.0 { 0.0 } else { (x / mean) * (x / mean).ln() })
        .sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_per_doc_theil": theil, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-theil — índice de Theil (entropia generalizada T0) de disk_bytes. Sprint #1383.
pub async fn segment_bytes_theil(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_theil": null, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    if total == 0 {
        return Json(serde_json::json!({"bytes_theil": 0.0, "segment_count": n}));
    }
    let mean = total as f64 / n as f64;
    let theil: f64 = segs.iter()
        .map(|(_, _, db)| {
            let x = *db as f64;
            if x == 0.0 { 0.0 } else { (x / mean) * (x / mean).ln() }
        })
        .sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_theil": theil, "segment_count": n, "total_bytes": total}))
}

/// GET /api/v1/search/index/segments/docs-theil — índice de Theil (entropia generalizada T0) de num_docs. Sprint #1378.
pub async fn segment_docs_theil(
    State(store): State<IndexStore>,
) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_theil": null, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    if total == 0 {
        return Json(serde_json::json!({"docs_theil": 0.0, "segment_count": n}));
    }
    let mean = total as f64 / n as f64;
    let theil: f64 = segs.iter()
        .map(|(_, nd, _)| {
            let x = *nd as f64;
            if x == 0.0 { 0.0 } else { (x / mean) * (x / mean).ln() }
        })
        .sum::<f64>() / n as f64;
    Json(serde_json::json!({"docs_theil": theil, "segment_count": n, "total_docs": total}))
}

/// GET /api/v1/search/index/segments/count-p90-count — número de segmentos acima do P90 de (num_docs+disk_bytes). Sprint #1548.
pub async fn segment_count_p90_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"p90_count": null, "above_p90_count": 0, "segment_count": n}));
    }
    let scores: Vec<u64> = segs.iter().map(|(_, nd, db)| nd + db).collect();
    let mut sorted = scores.clone();
    sorted.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted[p90_idx.min(n - 1)];
    let above_count = scores.iter().filter(|&&v| v > p90).count();
    Json(serde_json::json!({"p90_count": p90, "above_p90_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-p75-count — número de segmentos acima do P75 de num_docs. Sprint #1553.
pub async fn segment_docs_p75_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"p75_docs": null, "above_p75_count": 0, "segment_count": n}));
    }
    let docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    let mut sorted = docs.clone();
    sorted.sort_unstable();
    let p75_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let p75 = sorted[p75_idx.min(n - 1)];
    let above_count = docs.iter().filter(|&&v| v > p75).count();
    Json(serde_json::json!({"p75_docs": p75, "above_p75_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-p90-count — número de segmentos acima do P90 de num_docs. Sprint #1558.
pub async fn segment_docs_p90_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"p90_docs": null, "above_p90_count": 0, "segment_count": n}));
    }
    let docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    let mut sorted = docs.clone();
    sorted.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted[p90_idx.min(n - 1)];
    let above_count = docs.iter().filter(|&&v| v > p90).count();
    Json(serde_json::json!({"p90_docs": p90, "above_p90_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-p75-count — número de segmentos acima do P75 de disk_bytes. Sprint #1563.
pub async fn segment_bytes_p75_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"p75_bytes": null, "above_p75_count": 0, "segment_count": n}));
    }
    let bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    let mut sorted = bytes.clone();
    sorted.sort_unstable();
    let p75_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let p75 = sorted[p75_idx.min(n - 1)];
    let above_count = bytes.iter().filter(|&&v| v > p75).count();
    Json(serde_json::json!({"p75_bytes": p75, "above_p75_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-p75-count — número de segmentos com disk_bytes > P75. Sprint #1528.
pub async fn segment_bytes_above_p75_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"p75_bytes": null, "above_p75_count": 0, "segment_count": n}));
    }
    let bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    let mut sorted = bytes.clone();
    sorted.sort_unstable();
    let p75_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let p75 = sorted[p75_idx.min(n - 1)];
    let above_count = bytes.iter().filter(|&&v| v > p75).count();
    Json(serde_json::json!({"p75_bytes": p75, "above_p75_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-above-p75 — segmentos com (num_docs+disk_bytes) > P75. Sprint #1533.
pub async fn segment_count_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"p75_count": null, "segments": [], "segment_count": n}));
    }
    let scores: Vec<u64> = segs.iter().map(|(_, nd, db)| nd + db).collect();
    let mut sorted = scores.clone();
    sorted.sort_unstable();
    let p75_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let p75 = sorted[p75_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter().zip(scores.iter())
        .filter(|(_, &sc)| sc > p75)
        .map(|((id, nd, db), _)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p75_count": p75, "segments": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-above-p90 — segmentos com (num_docs+disk_bytes) > P90. Sprint #1538.
pub async fn segment_count_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"p90_count": null, "segments": [], "segment_count": n}));
    }
    let scores: Vec<u64> = segs.iter().map(|(_, nd, db)| nd + db).collect();
    let mut sorted = scores.clone();
    sorted.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted[p90_idx.min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter().zip(scores.iter())
        .filter(|(_, &sc)| sc > p90)
        .map(|((id, nd, db), _)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p90_count": p90, "segments": above, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-p75-count — número de segmentos acima do P75 de (num_docs+disk_bytes). Sprint #1543.
pub async fn segment_count_p75_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"p75_count": null, "above_p75_count": 0, "segment_count": n}));
    }
    let scores: Vec<u64> = segs.iter().map(|(_, nd, db)| nd + db).collect();
    let mut sorted = scores.clone();
    sorted.sort_unstable();
    let p75_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let p75 = sorted[p75_idx.min(n - 1)];
    let above_count = scores.iter().filter(|&&v| v > p75).count();
    Json(serde_json::json!({"p75_count": p75, "above_p75_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-p90-count — número de segmentos com disk_bytes > P90. Sprint #1508.
pub async fn segment_bytes_above_p90_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"p90_bytes": null, "above_p90_count": 0, "segment_count": n}));
    }
    let bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    let mut sorted = bytes.clone();
    sorted.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted[p90_idx.min(n - 1)];
    let above_count = bytes.iter().filter(|&&v| v > p90).count();
    Json(serde_json::json!({"p90_bytes": p90, "above_p90_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p75-count — número de segmentos com num_docs > P75. Sprint #1513.
pub async fn segment_docs_above_p75_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"p75_docs": null, "above_p75_count": 0, "segment_count": n}));
    }
    let docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    let mut sorted = docs.clone();
    sorted.sort_unstable();
    let p75_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let p75 = sorted[p75_idx.min(n - 1)];
    let above_count = docs.iter().filter(|&&v| v > p75).count();
    Json(serde_json::json!({"p75_docs": p75, "above_p75_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p90-count — número de segmentos acima do P90 de bytes/doc. Sprint #1518.
pub async fn segment_bytes_per_doc_p90_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"p90_bytes_per_doc": null, "above_p90_count": 0, "segment_count": n}));
    }
    let bpd: Vec<u64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { db / nd } else { 0 }).collect();
    let mut sorted = bpd.clone();
    sorted.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted[p90_idx.min(n - 1)];
    let above_count = bpd.iter().filter(|&&v| v > p90).count();
    Json(serde_json::json!({"p90_bytes_per_doc": p90, "above_p90_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p75-count — número de segmentos acima do P75 de bytes/doc. Sprint #1523.
pub async fn segment_bytes_per_doc_p75_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"p75_bytes_per_doc": null, "above_p75_count": 0, "segment_count": n}));
    }
    let bpd: Vec<u64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { db / nd } else { 0 }).collect();
    let mut sorted = bpd.clone();
    sorted.sort_unstable();
    let p75_idx = ((n as f64 - 1.0) * 0.75) as usize;
    let p75 = sorted[p75_idx.min(n - 1)];
    let above_count = bpd.iter().filter(|&&v| v > p75).count();
    Json(serde_json::json!({"p75_bytes_per_doc": p75, "above_p75_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/count-p99-count — número de segmentos acima do P99 de (num_docs+disk_bytes). Sprint #1488.
pub async fn segment_count_p99_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_count": null, "above_p99_count": 0, "segment_count": n}));
    }
    let scores: Vec<u64> = segs.iter().map(|(_, nd, db)| nd + db).collect();
    let mut sorted = scores.clone();
    sorted.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted[p99_idx.min(n - 1)];
    let above_count = scores.iter().filter(|&&v| v > p99).count();
    Json(serde_json::json!({"p99_count": p99, "above_p99_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p99-count — número de segmentos acima do P99 de bytes/doc. Sprint #1493.
pub async fn segment_bytes_per_doc_p99_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 10 {
        return Json(serde_json::json!({"p99_bytes_per_doc": null, "above_p99_count": 0, "segment_count": n}));
    }
    let bpd: Vec<u64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { db / nd } else { 0 }).collect();
    let mut sorted = bpd.clone();
    sorted.sort_unstable();
    let p99_idx = ((n as f64 - 1.0) * 0.99) as usize;
    let p99 = sorted[p99_idx.min(n - 1)];
    let above_count = bpd.iter().filter(|&&v| v > p99).count();
    Json(serde_json::json!({"p99_bytes_per_doc": p99, "above_p99_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-p95-count — número de segmentos com disk_bytes > P95. Sprint #1498.
pub async fn segment_bytes_above_p95_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_bytes": null, "above_p95_count": 0, "segment_count": n}));
    }
    let bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    let mut sorted = bytes.clone();
    sorted.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted[p95_idx.min(n - 1)];
    let above_count = bytes.iter().filter(|&&v| v > p95).count();
    Json(serde_json::json!({"p95_bytes": p95, "above_p95_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p95-count — número de segmentos com num_docs > P95. Sprint #1503.
pub async fn segment_docs_above_p95_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 5 {
        return Json(serde_json::json!({"p95_docs": null, "above_p95_count": 0, "segment_count": n}));
    }
    let docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    let mut sorted = docs.clone();
    sorted.sort_unstable();
    let p95_idx = ((n as f64 - 1.0) * 0.95) as usize;
    let p95 = sorted[p95_idx.min(n - 1)];
    let above_count = docs.iter().filter(|&&v| v > p95).count();
    Json(serde_json::json!({"p95_docs": p95, "above_p95_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-p90-count — número de segmentos acima do P90 de disk_bytes. Sprint #1568.
pub async fn segment_bytes_p90_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 { return Json(serde_json::json!({"p90_bytes": null, "above_p90_count": 0, "segment_count": n})); }
    let bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    let mut sorted = bytes.clone();
    sorted.sort_unstable();
    let p90_idx = ((n as f64 - 1.0) * 0.90) as usize;
    let p90 = sorted[p90_idx.min(n - 1)];
    let above_count = bytes.iter().filter(|&&v| v > p90).count();
    Json(serde_json::json!({"p90_bytes": p90, "above_p90_count": above_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/max-docs — segmento com maior num_docs. Sprint #1573.
pub async fn segment_max_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() { return Json(serde_json::json!({"segment_id": null, "max_docs": null})); }
    let (id, max_docs, _) = segs.iter().max_by_key(|(_, nd, _)| nd).unwrap();
    Json(serde_json::json!({"segment_id": id, "max_docs": max_docs}))
}

/// GET /api/v1/search/index/segments/max-bytes — segmento com maior disk_bytes. Sprint #1578.
pub async fn segment_max_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() { return Json(serde_json::json!({"segment_id": null, "max_bytes": null})); }
    let (id, _, max_bytes) = segs.iter().max_by_key(|(_, _, db)| db).unwrap();
    Json(serde_json::json!({"segment_id": id, "max_bytes": max_bytes}))
}

/// GET /api/v1/search/index/segments/avg-bytes-per-segment — média de disk_bytes por segmento. Sprint #1583.
pub async fn segment_avg_bytes_per_segment(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 { return Json(serde_json::json!({"avg_bytes": null, "segment_count": 0})); }
    let total: u64 = segs.iter().map(|(_, _, db)| db).sum();
    let avg = total / n as u64;
    Json(serde_json::json!({"avg_bytes": avg, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-min — segmento com menor ratio bytes/doc. Sprint #1588.
pub async fn segment_bytes_per_doc_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() { return Json(serde_json::json!({"segment_id": null, "bytes_per_doc": null})); }
    let ratios: Vec<(&String, u64)> = segs.iter()
        .filter(|(_, nd, _)| *nd > 0)
        .map(|(id, nd, db)| (id, db / nd))
        .collect();
    if ratios.is_empty() { return Json(serde_json::json!({"segment_id": null, "bytes_per_doc": null})); }
    let (id, ratio) = ratios.into_iter().min_by_key(|(_, r)| *r).unwrap();
    Json(serde_json::json!({"segment_id": id, "bytes_per_doc": ratio}))
}

/// GET /api/v1/search/index/segments/min-docs — segmento com menor num_docs. Sprint #1593.
pub async fn segment_min_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() { return Json(serde_json::json!({"segment_id": null, "min_docs": null})); }
    let (id, min_docs, _) = segs.iter().min_by_key(|(_, nd, _)| nd).unwrap();
    Json(serde_json::json!({"segment_id": id, "min_docs": min_docs}))
}

/// GET /api/v1/search/index/segments/min-bytes — segmento com menor disk_bytes. Sprint #1598.
pub async fn segment_min_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    if segs.is_empty() { return Json(serde_json::json!({"segment_id": null, "min_bytes": null})); }
    let (id, _, min_bytes) = segs.iter().min_by_key(|(_, _, db)| db).unwrap();
    Json(serde_json::json!({"segment_id": id, "min_bytes": min_bytes}))
}

/// GET /api/v1/search/index/segments/total-segment-count — número total de segmentos no índice. Sprint #1603.
pub async fn segment_total_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    Json(serde_json::json!({"total_segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-p50 — percentil 50 (mediana) de bytes/doc. Sprint #1608.
pub async fn segment_bytes_per_doc_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut ratios: Vec<u64> = segs.iter().filter(|(_, nd, _)| *nd > 0).map(|(_, nd, db)| db / nd).collect();
    let n = ratios.len();
    if n == 0 { return Json(serde_json::json!({"p50_bytes_per_doc": null})); }
    ratios.sort_unstable();
    let p50 = ratios[n / 2];
    Json(serde_json::json!({"p50_bytes_per_doc": p50, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-p50 — mediana de num_docs dos segmentos. Sprint #1613.
pub async fn segment_docs_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 { return Json(serde_json::json!({"p50_docs": null, "segment_count": 0})); }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p50 = docs[n / 2];
    Json(serde_json::json!({"p50_docs": p50, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bytes-p50 — mediana de disk_bytes dos segmentos. Sprint #1618.
pub async fn segment_bytes_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 { return Json(serde_json::json!({"p50_bytes": null, "segment_count": 0})); }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p50 = bytes[n / 2];
    Json(serde_json::json!({"p50_bytes": p50, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/top-segments-by-docs — top-5 segmentos por num_docs. Sprint #1623.
pub async fn segment_top_by_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted = segs.clone();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    let top: Vec<serde_json::Value> = sorted.into_iter().take(5)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"top_segments": top}))
}

/// GET /api/v1/search/index/segments/top-segments-by-bytes — top-5 segmentos por disk_bytes. Sprint #1628.
pub async fn segment_top_by_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted = segs.clone();
    sorted.sort_by(|a, b| b.2.cmp(&a.2));
    let top: Vec<serde_json::Value> = sorted.into_iter().take(5)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"top_segments": top}))
}

/// GET /api/v1/search/index/segments/bottom-segments-by-docs — bottom-5 segmentos por num_docs (menores). Sprint #1633.
pub async fn segment_bottom_by_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted = segs.clone();
    sorted.sort_by(|a, b| a.1.cmp(&b.1));
    let bottom: Vec<serde_json::Value> = sorted.into_iter().take(5)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"bottom_segments": bottom}))
}

/// GET /api/v1/search/index/segments/bottom-segments-by-bytes — bottom-5 segmentos por disk_bytes (menores). Sprint #1638.
pub async fn segment_bottom_by_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted = segs.clone();
    sorted.sort_by(|a, b| a.2.cmp(&b.2));
    let bottom: Vec<serde_json::Value> = sorted.into_iter().take(5)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"bottom_segments": bottom}))
}

/// GET /api/v1/search/index/segments/ratio-above-p50 — segmentos com bytes/doc acima do P50. Sprint #1643.
pub async fn segment_ratio_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<u64> = segs.iter().filter(|(_, nd, _)| *nd > 0).map(|(_, nd, db)| db / nd).collect();
    let n = ratios.len();
    if n == 0 { return Json(serde_json::json!({"p50_ratio": null, "above_count": 0, "segment_count": 0})); }
    let mut sorted = ratios.clone();
    sorted.sort_unstable();
    let p50 = sorted[n / 2];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, db)| *nd > 0 && db / nd > p50)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": db / nd}))
        .collect();
    Json(serde_json::json!({"p50_ratio": p50, "above_count": above.len(), "segments": above}))
}

/// GET /api/v1/search/index/segments/ratio-above-p75 — segments with bytes/doc > P75. Sprint #1648.
pub async fn segment_ratio_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<u64> = segs.iter().filter(|(_, nd, _)| *nd > 0).map(|(_, nd, db)| db / nd).collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let mut sorted = ratios.clone();
    sorted.sort_unstable();
    let p75 = sorted[(n * 3) / 4];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, db)| *nd > 0 && db / nd > p75)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": db / nd}))
        .collect();
    Json(serde_json::json!({"p75_ratio": p75, "above_count": above.len(), "segments": above}))
}

/// GET /api/v1/search/index/segments/ratio-above-p90 — segments with bytes/doc > P90. Sprint #1653.
pub async fn segment_ratio_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<u64> = segs.iter().filter(|(_, nd, _)| *nd > 0).map(|(_, nd, db)| db / nd).collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let mut sorted = ratios.clone();
    sorted.sort_unstable();
    let p90 = sorted[(n * 9) / 10];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, db)| *nd > 0 && db / nd > p90)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": db / nd}))
        .collect();
    Json(serde_json::json!({"p90_ratio": p90, "above_count": above.len(), "segments": above}))
}

/// GET /api/v1/search/index/segments/ratio-above-p95 — segments with bytes/doc > P95. Sprint #1658.
pub async fn segment_ratio_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<u64> = segs.iter().filter(|(_, nd, _)| *nd > 0).map(|(_, nd, db)| db / nd).collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let mut sorted = ratios.clone();
    sorted.sort_unstable();
    let p95 = sorted[(n * 19) / 20];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, db)| *nd > 0 && db / nd > p95)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": db / nd}))
        .collect();
    Json(serde_json::json!({"p95_ratio": p95, "above_count": above.len(), "segments": above}))
}

/// GET /api/v1/search/index/segments/above-avg-docs — segments with num_docs > avg. Sprint #1663.
pub async fn segment_above_avg_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_docs": null, "above_count": 0, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, nd, _)| nd).sum();
    let avg = total / n as u64;
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd > avg)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"avg_docs": avg, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/above-avg-bytes — segments with disk_bytes > avg. Sprint #1668.
pub async fn segment_above_avg_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, db)| db).sum();
    let avg = total / n as u64;
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db > avg)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"avg_bytes": avg, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/above-avg-ratio — segments with bytes/doc > avg ratio. Sprint #1673.
pub async fn segment_above_avg_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<(usize, u64)> = segs.iter().enumerate()
        .filter(|(_, (_, nd, _))| *nd > 0)
        .map(|(i, (_, nd, db))| (i, db / nd))
        .collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let total: u64 = ratios.iter().map(|(_, r)| r).sum();
    let avg = total / n as u64;
    let above: Vec<serde_json::Value> = ratios.iter()
        .filter(|(_, r)| *r > avg)
        .map(|(i, r)| {
            let (id, nd, db) = &segs[*i];
            serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r})
        })
        .collect();
    Json(serde_json::json!({"avg_ratio": avg, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/count-above-avg — count of segments above avg docs. Sprint #1678.
pub async fn segment_count_above_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_docs": null, "above_count": 0, "below_count": 0, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, nd, _)| nd).sum();
    let avg = total / n as u64;
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > avg).count();
    let below_count = segs.iter().filter(|(_, nd, _)| *nd <= avg).count();
    Json(serde_json::json!({"avg_docs": avg, "above_count": above_count, "below_count": below_count, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-below-avg — segments with num_docs <= avg. Sprint #1683.
pub async fn segment_docs_below_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, nd, _)| nd).sum();
    let avg = total / n as u64;
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= avg)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"avg_docs": avg, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-avg — segments with disk_bytes <= avg. Sprint #1688.
pub async fn segment_bytes_below_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, db)| db).sum();
    let avg = total / n as u64;
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= avg)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"avg_bytes": avg, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/ratio-below-avg — segments with bytes/doc <= avg ratio. Sprint #1693.
pub async fn segment_ratio_below_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<(usize, u64)> = segs.iter().enumerate()
        .filter(|(_, (_, nd, _))| *nd > 0)
        .map(|(i, (_, nd, db))| (i, db / nd))
        .collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let total: u64 = ratios.iter().map(|(_, r)| r).sum();
    let avg = total / n as u64;
    let below: Vec<serde_json::Value> = ratios.iter()
        .filter(|(_, r)| *r <= avg)
        .map(|(i, r)| {
            let (id, nd, db) = &segs[*i];
            serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r})
        })
        .collect();
    Json(serde_json::json!({"avg_ratio": avg, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/docs-below-p50 — segments with num_docs <= P50. Sprint #1698.
pub async fn segment_docs_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p50 = docs[n / 2];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p50)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p50_docs": p50, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-p50 — segments with disk_bytes <= P50. Sprint #1703.
pub async fn segment_bytes_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p50 = bytes[n / 2];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p50)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p50_bytes": p50, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/ratio-below-p50 — segments with bytes/doc <= P50 ratio. Sprint #1708.
pub async fn segment_ratio_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<(usize, u64)> = segs.iter().enumerate()
        .filter(|(_, (_, nd, _))| *nd > 0)
        .map(|(i, (_, nd, db))| (i, db / nd))
        .collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let mut sorted_ratios: Vec<u64> = ratios.iter().map(|(_, r)| *r).collect();
    sorted_ratios.sort_unstable();
    let p50 = sorted_ratios[n / 2];
    let below: Vec<serde_json::Value> = ratios.iter()
        .filter(|(_, r)| *r <= p50)
        .map(|(i, r)| {
            let (id, nd, db) = &segs[*i];
            serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r})
        })
        .collect();
    Json(serde_json::json!({"p50_ratio": p50, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/docs-below-p75 — segments with num_docs <= P75. Sprint #1713.
pub async fn segment_docs_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p75 = docs[(n * 3) / 4];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p75)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p75_docs": p75, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-p75 — segments with disk_bytes <= P75. Sprint #1718.
pub async fn segment_bytes_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p75 = bytes[(n * 3) / 4];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p75)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p75_bytes": p75, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/ratio-below-p75 — segments with bytes/doc <= P75 ratio. Sprint #1723.
pub async fn segment_ratio_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<(usize, u64)> = segs.iter().enumerate()
        .filter(|(_, (_, nd, _))| *nd > 0)
        .map(|(i, (_, nd, db))| (i, db / nd))
        .collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let mut sorted_ratios: Vec<u64> = ratios.iter().map(|(_, r)| *r).collect();
    sorted_ratios.sort_unstable();
    let p75 = sorted_ratios[(n * 3) / 4];
    let below: Vec<serde_json::Value> = ratios.iter()
        .filter(|(_, r)| *r <= p75)
        .map(|(i, r)| {
            let (id, nd, db) = &segs[*i];
            serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r})
        })
        .collect();
    Json(serde_json::json!({"p75_ratio": p75, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/docs-below-p90 — segments with num_docs <= P90. Sprint #1728.
pub async fn segment_docs_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p90 = docs[(n * 9) / 10];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p90)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p90_docs": p90, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-p90 — segments with disk_bytes <= P90. Sprint #1733.
pub async fn segment_bytes_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p90 = bytes[(n * 9) / 10];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p90)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p90_bytes": p90, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/ratio-below-p90 — segments with bytes/doc <= P90 ratio. Sprint #1738.
pub async fn segment_ratio_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<(usize, u64)> = segs.iter().enumerate()
        .filter(|(_, (_, nd, _))| *nd > 0)
        .map(|(i, (_, nd, db))| (i, db / nd))
        .collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let mut sorted_ratios: Vec<u64> = ratios.iter().map(|(_, r)| *r).collect();
    sorted_ratios.sort_unstable();
    let p90 = sorted_ratios[(n * 9) / 10];
    let below: Vec<serde_json::Value> = ratios.iter()
        .filter(|(_, r)| *r <= p90)
        .map(|(i, r)| {
            let (id, nd, db) = &segs[*i];
            serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r})
        })
        .collect();
    Json(serde_json::json!({"p90_ratio": p90, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/docs-below-p95 — segments with num_docs <= P95. Sprint #1743.
pub async fn segment_docs_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p95 = docs[(n * 19) / 20];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p95)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p95_docs": p95, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-p95 — segments with disk_bytes <= P95. Sprint #1748.
pub async fn segment_bytes_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p95 = bytes[(n * 19) / 20];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p95)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p95_bytes": p95, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/ratio-below-p95 — segments with bytes/doc <= P95 ratio. Sprint #1753.
pub async fn segment_ratio_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let ratios: Vec<(usize, u64)> = segs.iter().enumerate()
        .filter(|(_, (_, nd, _))| *nd > 0)
        .map(|(i, (_, nd, db))| (i, db / nd))
        .collect();
    let n = ratios.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let mut sorted_ratios: Vec<u64> = ratios.iter().map(|(_, r)| *r).collect();
    sorted_ratios.sort_unstable();
    let p95 = sorted_ratios[(n * 19) / 20];
    let below: Vec<serde_json::Value> = ratios.iter()
        .filter(|(_, r)| *r <= p95)
        .map(|(i, r)| {
            let (id, nd, db) = &segs[*i];
            serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r})
        })
        .collect();
    Json(serde_json::json!({"p95_ratio": p95, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/docs-below-p99 — segments with num_docs <= P99. Sprint #1758.
pub async fn segment_docs_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p99 = docs[(n * 99) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p99)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p99_docs": p99, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-p99 — segments with disk_bytes <= P99. Sprint #1763.
pub async fn segment_bytes_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p99 = bytes[(n * 99) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p99)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p99_bytes": p99, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/ratio-below-p99 — ratio de segmentos com disk_bytes ≤ P99. Sprint #1768.
pub async fn segment_ratio_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_bytes": null, "below_ratio": null, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p99 = bytes[(n * 99) / 100];
    let below_count = segs.iter().filter(|(_, _, db)| *db <= p99).count();
    let ratio = below_count as f64 / n as f64;
    Json(serde_json::json!({"p99_bytes": p99, "below_count": below_count, "below_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-below-p10 — segmentos com disk_bytes ≤ P10. Sprint #1773.
pub async fn segment_size_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p10 = bytes[(n * 10) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p10)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p10_bytes": p10, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/docs-above-p10 — segmentos com num_docs > P10. Sprint #1778.
pub async fn segment_docs_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_docs": null, "above_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p10 = docs[(n * 10) / 100];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd > p10)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p10_docs": p10, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/bytes-above-p10 — segmentos com disk_bytes > P10. Sprint #1783.
pub async fn segment_bytes_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p10 = bytes[(n * 10) / 100];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db > p10)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p10_bytes": p10, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/ratio-below-p10 — ratio de segmentos com disk_bytes ≤ P10. Sprint #1788.
pub async fn segment_ratio_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_bytes": null, "below_ratio": null, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p10 = bytes[(n * 10) / 100];
    let below_count = segs.iter().filter(|(_, _, db)| *db <= p10).count();
    let ratio = below_count as f64 / n as f64;
    Json(serde_json::json!({"p10_bytes": p10, "below_count": below_count, "below_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p25 — ratio de segmentos com disk_bytes ≤ P25. Sprint #1793.
pub async fn segment_ratio_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_bytes": null, "below_ratio": null, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p25 = bytes[(n * 25) / 100];
    let below_count = segs.iter().filter(|(_, _, db)| *db <= p25).count();
    let ratio = below_count as f64 / n as f64;
    Json(serde_json::json!({"p25_bytes": p25, "below_count": below_count, "below_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-below-p10 — segmentos com num_docs ≤ P10. Sprint #1798.
pub async fn segment_docs_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p10 = docs[(n * 10) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p10)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p10_docs": p10, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-p10 — segmentos com disk_bytes ≤ P10 (list). Sprint #1803.
pub async fn segment_bytes_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p10 = bytes[(n * 10) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p10)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p10_bytes": p10, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/size-above-p10 — segmentos com disk_bytes > P10 (alias bytes-above-p10 com name). Sprint #1808.
pub async fn segment_size_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p10 = bytes[(n * 10) / 100];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p10).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p10_bytes": p10, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-above-p25 — ratio de segmentos com disk_bytes > P25. Sprint #1813.
pub async fn segment_size_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p25 = bytes[(n * 25) / 100];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p25).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p25_bytes": p25, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-above-p50 — ratio de segmentos com disk_bytes > P50. Sprint #1818.
pub async fn segment_size_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p50 = bytes[n / 2];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p50).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p50_bytes": p50, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p25 — ratio de segmentos com num_docs > P25. Sprint #1823.
pub async fn segment_docs_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_docs": null, "above_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p25 = docs[(n * 25) / 100];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p25).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p25_docs": p25, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-above-p75 — ratio de segmentos com disk_bytes > P75. Sprint #1828.
pub async fn segment_size_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p75 = bytes[(n * 75) / 100];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p75).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p75_bytes": p75, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-above-p90 — ratio de segmentos com disk_bytes > P90. Sprint #1833.
pub async fn segment_size_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p90 = bytes[(n * 90) / 100];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p90).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p90_bytes": p90, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p50 — ratio de segmentos com num_docs > P50. Sprint #1838.
pub async fn segment_docs_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_docs": null, "above_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p50 = docs[n / 2];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p50).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p50_docs": p50, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-above-p95 — ratio de segmentos com disk_bytes > P95. Sprint #1843.
pub async fn segment_size_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p95 = bytes[(n * 95) / 100];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p95).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p95_bytes": p95, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-above-p99 — ratio de segmentos com disk_bytes > P99. Sprint #1848.
pub async fn segment_size_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p99 = bytes[(n * 99) / 100];
    let above_count = segs.iter().filter(|(_, _, db)| *db > p99).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p99_bytes": p99, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p10 — ratio de segmentos com bytes/doc > P10. Sprint #1853.
pub async fn segment_ratio_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let mut ratios: Vec<u64> = segs
        .iter()
        .map(|(_, nd, db)| if *nd > 0 { db / nd } else { *db })
        .collect();
    ratios.sort_unstable();
    let p10 = ratios[(n * 10) / 100];
    let above_count = segs
        .iter()
        .filter(|(_, nd, db)| {
            let r = if *nd > 0 { db / nd } else { *db };
            r > p10
        })
        .count();
    let above_ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p10_ratio": p10, "above_count": above_count, "above_ratio": above_ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p25 — ratio de segmentos com bytes/doc > P25. Sprint #1858.
pub async fn segment_ratio_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let mut ratios: Vec<u64> = segs
        .iter()
        .map(|(_, nd, db)| if *nd > 0 { db / nd } else { *db })
        .collect();
    ratios.sort_unstable();
    let p25 = ratios[(n * 25) / 100];
    let above_count = segs
        .iter()
        .filter(|(_, nd, db)| {
            let r = if *nd > 0 { db / nd } else { *db };
            r > p25
        })
        .count();
    let above_ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p25_ratio": p25, "above_count": above_count, "above_ratio": above_ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p99-ratio — ratio de segmentos com num_docs > P99. Sprint #1863.
pub async fn segment_docs_above_p99_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_docs": null, "above_ratio": null, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p99 = docs[(n * 99) / 100];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p99).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p99_docs": p99, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p99 — ratio de segmentos com bytes/doc > P99. Sprint #1868.
pub async fn segment_ratio_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let mut ratios: Vec<u64> = segs
        .iter()
        .map(|(_, nd, db)| if *nd > 0 { db / nd } else { *db })
        .collect();
    ratios.sort_unstable();
    let p99 = ratios[(n * 99) / 100];
    let above_count = segs
        .iter()
        .filter(|(_, nd, db)| {
            let r = if *nd > 0 { db / nd } else { *db };
            r > p99
        })
        .count();
    let above_ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p99_ratio": p99, "above_count": above_count, "above_ratio": above_ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p50-ratio — ratio de segmentos com num_docs > P50. Sprint #1873.
pub async fn segment_docs_above_p50_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_docs": null, "above_ratio": null, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p50 = docs[n / 2];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p50).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p50_docs": p50, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/docs-above-p25-ratio — ratio de segmentos com num_docs > P25. Sprint #1878.
pub async fn segment_docs_above_p25_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_docs": null, "above_ratio": null, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p25 = docs[(n * 25) / 100];
    let above_count = segs.iter().filter(|(_, nd, _)| *nd > p25).count();
    let ratio = above_count as f64 / n as f64;
    Json(serde_json::json!({"p25_docs": p25, "above_count": above_count, "above_ratio": ratio, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/size-below-p25 — segmentos com disk_bytes ≤ P25. Sprint #1883.
pub async fn segment_size_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p25 = bytes[(n * 25) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p25)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_bytes": p25, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/size-below-p50 — segments whose disk_bytes ≤ p50. Sprint #1888.
pub async fn segment_size_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p50 = bytes[n / 2];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p50)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p50_bytes": p50, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/size-below-p75 — segments whose disk_bytes ≤ p75. Sprint #1893.
pub async fn segment_size_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p75 = bytes[(n * 75) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p75)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p75_bytes": p75, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/size-below-p90 — segments whose disk_bytes ≤ p90. Sprint #1898.
pub async fn segment_size_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p90 = bytes[(n * 90) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p90)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p90_bytes": p90, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/size-below-p95 — segments whose disk_bytes ≤ p95. Sprint #1903.
pub async fn segment_size_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p95 = bytes[(n * 95) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p95)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p95_bytes": p95, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-above-p25 — segments whose disk_bytes > p25. Sprint #1908.
pub async fn segment_bytes_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p25 = bytes[(n * 25) / 100];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db > p25)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_bytes": p25, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/bytes-above-p50 — segments whose disk_bytes > p50. Sprint #1913.
pub async fn segment_bytes_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_bytes": null, "above_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p50 = bytes[n / 2];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db > p50)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p50_bytes": p50, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/docs-below-p25 — segments whose num_docs ≤ p25. Sprint #1918.
pub async fn segment_docs_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_docs": null, "below_count": 0, "segment_count": 0}));
    }
    let mut docs: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs.sort_unstable();
    let p25 = docs[(n * 25) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p25)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_docs": p25, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-below-p25 — segments whose disk_bytes ≤ p25. Sprint #1923.
pub async fn segment_bytes_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p25 = bytes[(n * 25) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p25)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_bytes": p25, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/count-below-p25 — segments whose segment count ≤ p25. Sprint #1928.
pub async fn segment_count_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_count": null, "below_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p25 = counts[(n * 25) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p25)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_count": p25, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/count-below-p50 — segments whose num_docs ≤ p50. Sprint #1933.
pub async fn segment_count_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_count": null, "below_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p50 = counts[n / 2];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p50)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p50_count": p50, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/count-below-p75 — segments whose num_docs ≤ p75. Sprint #1938.
pub async fn segment_count_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_count": null, "below_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p75 = counts[(n * 75) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p75)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p75_count": p75, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/count-below-p90 — segments whose num_docs ≤ p90. Sprint #1943.
pub async fn segment_count_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_count": null, "below_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p90 = counts[(n * 90) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p90)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p90_count": p90, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/count-below-p95 — segments whose num_docs ≤ p95. Sprint #1948.
pub async fn segment_count_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_count": null, "below_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p95 = counts[(n * 95) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p95)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p95_count": p95, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/count-below-p99 — segments whose num_docs ≤ p99. Sprint #1953.
pub async fn segment_count_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_count": null, "below_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p99 = counts[(n * 99) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd <= p99)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p99_count": p99, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/count-above-p25 — segments whose num_docs > p25. Sprint #1958.
pub async fn segment_count_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_count": null, "above_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p25 = counts[(n * 25) / 100];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd > p25)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_count": p25, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/count-above-p50 — segments whose num_docs > p50. Sprint #1963.
pub async fn segment_count_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_count": null, "above_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p50 = counts[n / 2];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd > p50)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p50_count": p50, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/count-above-p95 — segments whose num_docs > p95. Sprint #1968.
pub async fn segment_count_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_count": null, "above_count": 0, "segment_count": 0}));
    }
    let mut counts: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    counts.sort_unstable();
    let p95 = counts[(n * 95) / 100];
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, nd, _)| *nd > p95)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p95_count": p95, "above_count": above.len(), "segment_count": n, "segments": above}))
}

/// GET /api/v1/search/index/segments/size-below-p99 — segments whose disk_bytes ≤ p99. Sprint #1973.
pub async fn segment_size_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_bytes": null, "below_count": 0, "segment_count": 0}));
    }
    let mut bytes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes.sort_unstable();
    let p99 = bytes[(n * 99) / 100];
    let below: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, db)| *db <= p99)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p99_bytes": p99, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-below-p25 — segments whose bytes/doc ratio ≤ p25. Sprint #1978.
pub async fn segment_bytes_per_doc_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p25 = sorted[(n * 25) / 100];
    let below: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r <= p25)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p25_ratio": p25, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-below-p50 — segments whose bytes/doc ratio ≤ p50. Sprint #1983.
pub async fn segment_bytes_per_doc_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = sorted[n / 2];
    let below: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r <= p50)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p50_ratio": p50, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-below-p75 — segments whose bytes/doc ratio ≤ p75. Sprint #1988.
pub async fn segment_bytes_per_doc_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p75 = sorted[(n * 75) / 100];
    let below: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r <= p75)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p75_ratio": p75, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-below-p90 — segments whose bytes/doc ratio ≤ p90. Sprint #1993.
pub async fn segment_bytes_per_doc_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p90 = sorted[(n * 90) / 100];
    let below: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r <= p90)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p90_ratio": p90, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-below-p95 — segments whose bytes/doc ratio ≤ p95. Sprint #1998.
pub async fn segment_bytes_per_doc_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = sorted[(n * 95) / 100];
    let below: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r <= p95)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p95_ratio": p95, "below_count": below.len(), "segment_count": n, "segments": below}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-below-p99 — segments whose bytes/doc ratio ≤ p99. Sprint #2003.
pub async fn segment_bytes_per_doc_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_ratio": null, "below_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = sorted[(n * 99) / 100];
    let below: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r <= p99)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p99_ratio": p99, "below_count": below.len(), "segment_count": n, "segments": below}))
}

pub async fn segment_bytes_per_doc_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p75 = sorted[(n * 75) / 100];
    let above: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r > p75)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p75_ratio": p75, "above_count": above.len(), "segment_count": n, "segments": above}))
}

pub async fn segment_bytes_per_doc_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p90 = sorted[(n * 90) / 100];
    let above: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r > p90)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p90_ratio": p90, "above_count": above.len(), "segment_count": n, "segments": above}))
}

pub async fn segment_bytes_per_doc_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p25 = sorted[(n * 25) / 100];
    let above: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r > p25)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p25_ratio": p25, "above_count": above.len(), "segment_count": n, "segments": above}))
}

pub async fn segment_bytes_per_doc_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_ratio": null, "above_count": 0, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = sorted[n / 2];
    let above: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, &r)| r > p50)
        .map(|((id, nd, db), &r)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db, "bytes_per_doc": r}))
        .collect();
    Json(serde_json::json!({"p50_ratio": p50, "above_count": above.len(), "segment_count": n, "segments": above}))
}

pub async fn segment_empty_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let empty: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd == 0)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"empty_count": empty.len(), "segment_count": segs.len(), "segments": empty}))
}

pub async fn segment_singleton_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let singletons: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd == 1)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"singleton_count": singletons.len(), "segment_count": segs.len(), "segments": singletons}))
}

pub async fn segment_large_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_docs": null, "large_count": 0, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let p75 = docs_sorted[(n * 75) / 100];
    let large: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd > p75)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p75_docs": p75, "large_count": large.len(), "segment_count": n, "segments": large}))
}

pub async fn segment_small_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_docs": null, "small_count": 0, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let p25 = docs_sorted[(n * 25) / 100];
    let small: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd < p25)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_docs": p25, "small_count": small.len(), "segment_count": n, "segments": small}))
}

pub async fn segment_medium_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_docs": null, "p75_docs": null, "medium_count": 0, "segment_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, nd, _)| *nd).collect();
    docs_sorted.sort_unstable();
    let p25 = docs_sorted[(n * 25) / 100];
    let p75 = docs_sorted[(n * 75) / 100];
    let medium: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd >= p25 && *nd <= p75)
        .map(|(id, nd, db)| serde_json::json!({"segment_id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"p25_docs": p25, "p75_docs": p75, "medium_count": medium.len(), "segment_count": n, "segments": medium}))
}

pub async fn segment_docs_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"stddev_docs": null, "mean_docs": null, "segment_count": 0}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let variance = docs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    Json(serde_json::json!({"stddev_docs": stddev, "mean_docs": mean, "segment_count": n}))
}

pub async fn segment_bytes_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"stddev_bytes": null, "mean_bytes": null, "segment_count": 0}));
    }
    let bytes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = bytes.iter().sum::<f64>() / n as f64;
    let variance = bytes.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    Json(serde_json::json!({"stddev_bytes": stddev, "mean_bytes": mean, "segment_count": n}))
}

pub async fn segment_ratio_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"stddev_ratio": null, "mean_ratio": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let variance = ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    Json(serde_json::json!({"stddev_ratio": stddev, "mean_ratio": mean, "segment_count": n}))
}

pub async fn segment_ratio_cv(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"cv_ratio": null, "mean_ratio": null, "stddev_ratio": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let stddev = (ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_ratio": cv, "mean_ratio": mean, "stddev_ratio": stddev, "segment_count": n}))
}

pub async fn segment_ratio_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"iqr_ratio": null, "p25_ratio": null, "p75_ratio": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p25 = ratios[(n * 25) / 100];
    let p75 = ratios[(n * 75) / 100];
    Json(serde_json::json!({"iqr_ratio": p75 - p25, "p25_ratio": p25, "p75_ratio": p75, "segment_count": n}))
}

pub async fn segment_ratio_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"range_ratio": null, "min_ratio": null, "max_ratio": null, "segment_count": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let min = ratios.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = ratios.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    Json(serde_json::json!({"range_ratio": max - min, "min_ratio": min, "max_ratio": max, "segment_count": n}))
}

pub async fn segment_ratio_skew(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skew_ratio": null, "segment_count": n}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let stddev = (ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let skew = if stddev > 0.0 {
        ratios.iter().map(|r| ((r - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skew_ratio": skew, "mean_ratio": mean, "stddev_ratio": stddev, "segment_count": n}))
}

pub async fn segment_docs_skew(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skew_docs": null, "segment_count": n}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let stddev = (docs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let skew = if stddev > 0.0 {
        docs.iter().map(|d| ((d - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skew_docs": skew, "mean_docs": mean, "stddev_docs": stddev, "segment_count": n}))
}

pub async fn segment_bytes_skew(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skew_bytes": null, "segment_count": n}));
    }
    let bytes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = bytes.iter().sum::<f64>() / n as f64;
    let stddev = (bytes.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let skew = if stddev > 0.0 {
        bytes.iter().map(|b| ((b - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skew_bytes": skew, "mean_bytes": mean, "stddev_bytes": stddev, "segment_count": n}))
}

pub async fn segment_ratio_kurtosis(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"kurtosis_ratio": null, "segment_count": n}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| if *nd > 0 { *db as f64 / *nd as f64 } else { 0.0 }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let stddev = (ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let kurt = if stddev > 0.0 {
        ratios.iter().map(|r| ((r - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
    } else { 0.0 };
    Json(serde_json::json!({"kurtosis_ratio": kurt, "mean_ratio": mean, "stddev_ratio": stddev, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/byte-density-p75 — P75 da densidade bytes/doc. Sprint #2445.
pub async fn segment_byte_density_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_byte_density": null, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (n as f64 * 0.75) as usize;
    let p75 = densities[idx.min(n - 1)];
    Json(serde_json::json!({"p75_byte_density": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-p90 — P90 da densidade bytes/doc. Sprint #2450.
pub async fn segment_byte_density_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_byte_density": null, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (n as f64 * 0.90) as usize;
    let p90 = densities[idx.min(n - 1)];
    Json(serde_json::json!({"p90_byte_density": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-avg — média de docs por byte (inverso). Sprint #2455.
pub async fn segment_docs_density_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_docs_per_byte": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let avg = densities.iter().sum::<f64>() / n as f64;
    Json(serde_json::json!({"avg_docs_per_byte": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/mad-bytes — desvio absoluto mediano de bytes entre segmentos. Sprint #2728.
pub async fn segment_mad_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mad_bytes": null, "total_segments": 0}));
    }
    let mut bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    bytes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if n % 2 == 0 { (bytes[n / 2 - 1] + bytes[n / 2]) / 2.0 } else { bytes[n / 2] };
    let mut deviations: Vec<f64> = bytes.iter().map(|x| (x - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = if n % 2 == 0 { (deviations[n / 2 - 1] + deviations[n / 2]) / 2.0 } else { deviations[n / 2] };
    Json(serde_json::json!({"mad_bytes": mad, "median_bytes": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/mad-docs — desvio absoluto mediano de docs entre segmentos. Sprint #2733.
pub async fn segment_mad_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mad_docs": null, "total_segments": 0}));
    }
    let mut docs: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    docs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if n % 2 == 0 { (docs[n / 2 - 1] + docs[n / 2]) / 2.0 } else { docs[n / 2] };
    let mut deviations: Vec<f64> = docs.iter().map(|x| (x - median).abs()).collect();
    deviations.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = if n % 2 == 0 { (deviations[n / 2 - 1] + deviations[n / 2]) / 2.0 } else { deviations[n / 2] };
    Json(serde_json::json!({"mad_docs": mad, "median_docs": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/cv-bytes — coeficiente de variação de bytes entre segmentos. Sprint #2738.
pub async fn segment_cv_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"cv_bytes": null, "total_segments": 0}));
    }
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = bytes.iter().sum::<f64>() / n as f64;
    let stddev = (bytes.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_bytes": cv, "mean_bytes": mean, "stddev_bytes": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/cv-docs — coeficiente de variação de docs entre segmentos. Sprint #2743.
pub async fn segment_cv_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"cv_docs": null, "total_segments": 0}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let stddev = (docs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_docs": cv, "mean_docs": mean, "stddev_docs": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/cv-ratio — coeficiente de variação da razão docs/bytes entre segmentos. Sprint #2708.
pub async fn segment_cv_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"cv_ratio": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, d, b)| {
        if *b > 0 { *d as f64 / *b as f64 } else { 0.0 }
    }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let stddev = (ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_ratio": cv, "mean_ratio": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/range-bytes — range (max-min) de bytes entre segmentos. Sprint #2713.
pub async fn segment_range_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"range_bytes": null, "total_segments": 0}));
    }
    let max_b = segs.iter().map(|(_, _, b)| *b).max().unwrap_or(0);
    let min_b = segs.iter().map(|(_, _, b)| *b).min().unwrap_or(0);
    Json(serde_json::json!({"range_bytes": max_b - min_b, "max_bytes": max_b, "min_bytes": min_b, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/range-docs — range (max-min) de docs entre segmentos. Sprint #2718.
pub async fn segment_range_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"range_docs": null, "total_segments": 0}));
    }
    let max_d = segs.iter().map(|(_, d, _)| *d).max().unwrap_or(0);
    let min_d = segs.iter().map(|(_, d, _)| *d).min().unwrap_or(0);
    Json(serde_json::json!({"range_docs": max_d - min_d, "max_docs": max_d, "min_docs": min_d, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/stddev-ratio — desvio-padrão da razão docs/bytes entre segmentos. Sprint #2723.
pub async fn segment_stddev_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"stddev_ratio": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, d, b)| {
        if *b > 0 { *d as f64 / *b as f64 } else { 0.0 }
    }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let stddev = (ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    Json(serde_json::json!({"stddev_ratio": stddev, "mean_ratio": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/theil-bytes — índice Theil T de desigualdade de bytes entre segmentos. Sprint #2688.
pub async fn segment_theil_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"theil_bytes": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    let theil = if total > 0 && n > 1 {
        let mean = total as f64 / n as f64;
        segs.iter().fold(0.0_f64, |acc, (_, _, b)| {
            let x = *b as f64;
            if x > 0.0 { acc + (x / total as f64) * (x / mean).ln() } else { acc }
        })
    } else { 0.0 };
    Json(serde_json::json!({"theil_bytes": theil, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/theil-docs — índice Theil T de desigualdade de docs entre segmentos. Sprint #2693.
pub async fn segment_theil_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"theil_docs": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    let theil = if total > 0 && n > 1 {
        let mean = total as f64 / n as f64;
        segs.iter().fold(0.0_f64, |acc, (_, d, _)| {
            let x = *d as f64;
            if x > 0.0 { acc + (x / total as f64) * (x / mean).ln() } else { acc }
        })
    } else { 0.0 };
    Json(serde_json::json!({"theil_docs": theil, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/atkinson-bytes — índice Atkinson (ε=0.5) de bytes entre segmentos. Sprint #2698.
pub async fn segment_atkinson_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"atkinson_bytes": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    let atkinson = if total > 0 && n > 1 {
        let mean = total as f64 / n as f64;
        let epsilon: f64 = 0.5;
        let ede = segs.iter().fold(0.0_f64, |acc, (_, _, b)| {
            acc + (*b as f64).powf(1.0 - epsilon)
        }).powf(1.0 / (1.0 - epsilon)) / n as f64;
        1.0 - ede / mean
    } else { 0.0 };
    Json(serde_json::json!({"atkinson_bytes": atkinson, "epsilon": 0.5, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/atkinson-docs — índice Atkinson (ε=0.5) de docs entre segmentos. Sprint #2703.
pub async fn segment_atkinson_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"atkinson_docs": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    let atkinson = if total > 0 && n > 1 {
        let mean = total as f64 / n as f64;
        let epsilon: f64 = 0.5;
        let ede = segs.iter().fold(0.0_f64, |acc, (_, d, _)| {
            acc + (*d as f64).powf(1.0 - epsilon)
        }).powf(1.0 / (1.0 - epsilon)) / n as f64;
        1.0 - ede / mean
    } else { 0.0 };
    Json(serde_json::json!({"atkinson_docs": atkinson, "epsilon": 0.5, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/hhi-bytes — índice Herfindahl-Hirschman de bytes entre segmentos. Sprint #2668.
pub async fn segment_hhi_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"hhi_bytes": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    let hhi = if total > 0 {
        segs.iter().fold(0.0_f64, |acc, (_, _, b)| {
            let s = *b as f64 / total as f64;
            acc + s * s
        })
    } else { 0.0 };
    Json(serde_json::json!({"hhi_bytes": hhi, "total_bytes": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/hhi-docs — índice Herfindahl-Hirschman de docs entre segmentos. Sprint #2673.
pub async fn segment_hhi_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"hhi_docs": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    let hhi = if total > 0 {
        segs.iter().fold(0.0_f64, |acc, (_, d, _)| {
            let s = *d as f64 / total as f64;
            acc + s * s
        })
    } else { 0.0 };
    Json(serde_json::json!({"hhi_docs": hhi, "total_docs": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/lorenz-bytes — ponto Lorenz (razão cumulativa) de bytes entre segmentos. Sprint #2678.
pub async fn segment_lorenz_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_bytes": [], "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let total: u64 = vals.iter().sum();
    let lorenz: Vec<serde_json::Value> = if total > 0 {
        let mut cumsum = 0u64;
        vals.iter().enumerate().map(|(i, v)| {
            cumsum += v;
            serde_json::json!({"pop_pct": (i + 1) as f64 / n as f64, "bytes_pct": cumsum as f64 / total as f64})
        }).collect()
    } else { vec![] };
    Json(serde_json::json!({"lorenz_bytes": lorenz, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/lorenz-docs — ponto Lorenz (razão cumulativa) de docs entre segmentos. Sprint #2683.
pub async fn segment_lorenz_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_docs": [], "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let total: u64 = vals.iter().sum();
    let lorenz: Vec<serde_json::Value> = if total > 0 {
        let mut cumsum = 0u64;
        vals.iter().enumerate().map(|(i, v)| {
            cumsum += v;
            serde_json::json!({"pop_pct": (i + 1) as f64 / n as f64, "docs_pct": cumsum as f64 / total as f64})
        }).collect()
    } else { vec![] };
    Json(serde_json::json!({"lorenz_docs": lorenz, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/entropy-bytes — entropia de Shannon de bytes entre segmentos. Sprint #2648.
pub async fn segment_entropy_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"entropy_bytes": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    let entropy = if total > 0 {
        segs.iter().fold(0.0_f64, |acc, (_, _, b)| {
            let p = *b as f64 / total as f64;
            if p > 0.0 { acc - p * p.log2() } else { acc }
        })
    } else { 0.0 };
    Json(serde_json::json!({"entropy_bytes": entropy, "total_bytes": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/entropy-docs — entropia de Shannon de docs entre segmentos. Sprint #2653.
pub async fn segment_entropy_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"entropy_docs": null, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    let entropy = if total > 0 {
        segs.iter().fold(0.0_f64, |acc, (_, d, _)| {
            let p = *d as f64 / total as f64;
            if p > 0.0 { acc - p * p.log2() } else { acc }
        })
    } else { 0.0 };
    Json(serde_json::json!({"entropy_docs": entropy, "total_docs": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/gini-bytes — coeficiente de Gini de bytes entre segmentos. Sprint #2658.
pub async fn segment_gini_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"gini_bytes": 0.0, "total_segments": n}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let sum: f64 = vals.iter().sum();
    let gini = if sum > 0.0 {
        let weighted: f64 = vals.iter().enumerate().map(|(i, v)| (2 * (i + 1) - n - 1) as f64 * v).sum();
        weighted / (n as f64 * sum)
    } else { 0.0 };
    Json(serde_json::json!({"gini_bytes": gini, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/gini-docs — coeficiente de Gini de docs entre segmentos. Sprint #2663.
pub async fn segment_gini_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"gini_docs": 0.0, "total_segments": n}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let sum: f64 = vals.iter().sum();
    let gini = if sum > 0.0 {
        let weighted: f64 = vals.iter().enumerate().map(|(i, v)| (2 * (i + 1) - n - 1) as f64 * v).sum();
        weighted / (n as f64 * sum)
    } else { 0.0 };
    Json(serde_json::json!({"gini_docs": gini, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/skewness-bytes — assimetria (skewness) de bytes entre segmentos. Sprint #2628.
pub async fn segment_skewness_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skewness_bytes": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let skewness = if stddev > 0.0 {
        vals.iter().map(|v| ((v - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skewness_bytes": skewness, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/skewness-docs — assimetria (skewness) de docs entre segmentos. Sprint #2633.
pub async fn segment_skewness_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skewness_docs": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let skewness = if stddev > 0.0 {
        vals.iter().map(|v| ((v - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skewness_docs": skewness, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-p95 — P95 da razão bytes/doc entre segmentos. Sprint #2768.
pub async fn segment_ratio_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_p95": null, "total_segments": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((n as f64 * 0.95) as usize).min(n - 1);
    Json(serde_json::json!({"ratio_p95": ratios[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-p99 — P99 da razão bytes/doc entre segmentos. Sprint #2773.
pub async fn segment_ratio_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_p99": null, "total_segments": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((n as f64 * 0.99) as usize).min(n - 1);
    Json(serde_json::json!({"ratio_p99": ratios[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-skewness — assimetria (skewness) da razão bytes/doc entre segmentos. Sprint #2778.
pub async fn segment_ratio_skewness(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"ratio_skewness": null, "total_segments": n}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let variance = ratios.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let skewness = if stddev > 0.0 {
        ratios.iter().map(|v| ((v - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"ratio_skewness": skewness, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-mad — MAD da razão bytes/doc entre segmentos. Sprint #2783.
pub async fn segment_ratio_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_mad": null, "total_segments": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if n % 2 == 0 {
        (ratios[n / 2 - 1] + ratios[n / 2]) / 2.0
    } else {
        ratios[n / 2]
    };
    let mut abs_devs: Vec<f64> = ratios.iter().map(|v| (v - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = if n % 2 == 0 {
        (abs_devs[n / 2 - 1] + abs_devs[n / 2]) / 2.0
    } else {
        abs_devs[n / 2]
    };
    Json(serde_json::json!({"ratio_mad": mad, "ratio_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-entropy — entropia de Shannon da razão bytes/doc entre segmentos. Sprint #2788.
pub async fn segment_ratio_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_entropy": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let total: f64 = ratios.iter().sum();
    let entropy = if total > 0.0 {
        ratios.iter().map(|&r| {
            let p = r / total;
            if p > 0.0 { -p * p.ln() } else { 0.0 }
        }).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"ratio_entropy": entropy, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-gini — coeficiente de Gini da razão bytes/doc entre segmentos. Sprint #2793.
pub async fn segment_ratio_gini(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_gini": null, "total_segments": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = ratios.iter().sum();
    let gini = if total > 0.0 {
        let num: f64 = ratios.iter().enumerate().map(|(i, v)| (2 * (i + 1) - n - 1) as f64 * v).sum();
        num / (n as f64 * total)
    } else { 0.0 };
    Json(serde_json::json!({"ratio_gini": gini, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-hhi — índice Herfindahl-Hirschman da razão bytes/doc entre segmentos. Sprint #2798.
pub async fn segment_ratio_hhi(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_hhi": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let total: f64 = ratios.iter().sum();
    let hhi = if total > 0.0 {
        ratios.iter().map(|&r| { let s = r / total; s * s }).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"ratio_hhi": hhi, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-lorenz — curva de Lorenz da razão bytes/doc entre segmentos. Sprint #2803.
pub async fn segment_ratio_lorenz(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_lorenz": [], "total_segments": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let total: f64 = ratios.iter().sum();
    let lorenz: Vec<serde_json::Value> = if total > 0.0 {
        let mut cum = 0.0f64;
        ratios.iter().enumerate().map(|(i, &r)| {
            cum += r;
            serde_json::json!({"rank": i + 1, "cumulative_share": cum / total})
        }).collect()
    } else { vec![] };
    Json(serde_json::json!({"ratio_lorenz": lorenz, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-theil — índice de Theil da razão bytes/doc entre segmentos. Sprint #2808.
pub async fn segment_ratio_theil(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_theil": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let theil = if mean > 0.0 {
        ratios.iter().map(|&r| {
            if r > 0.0 { (r / mean) * (r / mean).ln() } else { 0.0 }
        }).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"ratio_theil": theil, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-atkinson — índice de Atkinson da razão bytes/doc entre segmentos. Sprint #2813.
pub async fn segment_ratio_atkinson(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_atkinson": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let log_sum: f64 = ratios.iter().map(|&r| if r > 0.0 { r.ln() } else { 0.0 }).sum();
    let geo_mean = (log_sum / n as f64).exp();
    let atkinson = if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 };
    Json(serde_json::json!({"ratio_atkinson": atkinson, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-variance — variância da razão bytes/doc entre segmentos. Sprint #2818.
pub async fn segment_ratio_variance(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"ratio_variance": null, "total_segments": n}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let variance = ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"ratio_variance": variance, "ratio_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-trimmed-mean — média aparada (10%) da razão bytes/doc entre segmentos. Sprint #2823.
pub async fn segment_ratio_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_trimmed_mean": null, "total_segments": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let trim = (n as f64 * 0.1) as usize;
    let trimmed = &ratios[trim..n.saturating_sub(trim)];
    let trimmed_mean = if trimmed.is_empty() {
        ratios.iter().sum::<f64>() / n as f64
    } else {
        trimmed.iter().sum::<f64>() / trimmed.len() as f64
    };
    Json(serde_json::json!({"ratio_trimmed_mean": trimmed_mean, "total_segments": n, "trimmed_count": trimmed.len()}))
}

/// GET /api/v1/search/index/segments/ratio-harmonic-mean — média harmônica de bytes/docs por segmento. Sprint #2828.
pub async fn segment_ratio_harmonic_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_harmonic_mean": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let nonzero: Vec<f64> = ratios.iter().copied().filter(|&r| r > 0.0).collect();
    let harmonic_mean = if nonzero.is_empty() {
        0.0
    } else {
        let recip_sum: f64 = nonzero.iter().map(|r| 1.0 / r).sum();
        nonzero.len() as f64 / recip_sum
    };
    Json(serde_json::json!({"ratio_harmonic_mean": harmonic_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-geometric-mean — média geométrica de bytes/docs por segmento. Sprint #2833.
pub async fn segment_ratio_geometric_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_geometric_mean": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let nonzero: Vec<f64> = ratios.iter().copied().filter(|&r| r > 0.0).collect();
    let geometric_mean = if nonzero.is_empty() {
        0.0
    } else {
        let log_sum: f64 = nonzero.iter().map(|r| r.ln()).sum();
        (log_sum / nonzero.len() as f64).exp()
    };
    Json(serde_json::json!({"ratio_geometric_mean": geometric_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-winsorized-mean — média winsorizada (10%–90%) de bytes/docs por segmento. Sprint #2838.
pub async fn segment_ratio_winsorized_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_winsorized_mean": null, "total_segments": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    ratios.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = (n as f64 * 0.10).floor() as usize;
    let hi = (n as f64 * 0.90).ceil() as usize;
    let hi = hi.min(n);
    let winsorized_mean = if lo < hi {
        let slice = &ratios[lo..hi];
        slice.iter().sum::<f64>() / slice.len() as f64
    } else {
        ratios[n / 2]
    };
    Json(serde_json::json!({"ratio_winsorized_mean": winsorized_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-coefficient-of-variation — coeficiente de variação de bytes/docs por segmento. Sprint #2843.
pub async fn segment_ratio_coefficient_of_variation(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_cv": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let variance = ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let cv = if mean.abs() > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"ratio_cv": cv, "ratio_mean": mean, "ratio_stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-p95 — P95 de disk_bytes entre segmentos. Sprint #2848.
pub async fn segment_bytes_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_p95": null, "total_segments": 0}));
    }
    let mut bytes_vals: Vec<f64> = segs.iter().map(|(_, _, bytes)| *bytes as f64).collect();
    bytes_vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"bytes_p95": bytes_vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-hhi — índice HHI de disk_bytes entre segmentos. Sprint #2853.
pub async fn segment_bytes_hhi(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_hhi": null, "total_segments": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| b).sum();
    let hhi = if total_bytes > 0 {
        segs.iter().map(|(_, _, b)| {
            let share = *b as f64 / total_bytes as f64;
            share * share
        }).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"bytes_hhi": hhi, "total_bytes": total_bytes, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-lorenz — curva de Lorenz de disk_bytes entre segmentos. Sprint #2858.
pub async fn segment_bytes_lorenz(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_curve": [], "total_segments": 0}));
    }
    let mut bytes_vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_vals.sort_unstable();
    let total: u64 = bytes_vals.iter().sum();
    let lorenz_points: Vec<serde_json::Value> = bytes_vals.iter().enumerate().scan(0u64, |acc, (i, b)| {
        *acc += b;
        Some(serde_json::json!({
            "cumulative_population": (i + 1) as f64 / n as f64,
            "cumulative_share": if total > 0 { *acc as f64 / total as f64 } else { 0.0 }
        }))
    }).collect();
    Json(serde_json::json!({"lorenz_curve": lorenz_points, "total_bytes": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-hhi — HHI de num_docs entre segmentos. Sprint #3415.
pub async fn segment_docs_hhi(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_hhi": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let total: f64 = vals.iter().sum();
    let hhi = if total == 0.0 { 0.0 } else { vals.iter().map(|&v| (v / total).powi(2)).sum() };
    Json(serde_json::json!({"docs_hhi": hhi, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-atkinson — índice de Atkinson de num_docs entre segmentos. Sprint #3416.
pub async fn segment_docs_atkinson(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_atkinson": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let nonzero: Vec<f64> = vals.iter().copied().filter(|&v| v > 0.0).collect();
    let atkinson = if mean > 0.0 && !nonzero.is_empty() {
        let geo_mean = (nonzero.iter().map(|v| v.ln()).sum::<f64>() / n as f64).exp();
        1.0 - geo_mean / mean
    } else { 0.0 };
    Json(serde_json::json!({"docs_atkinson": atkinson, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-lorenz — curva de Lorenz de num_docs entre segmentos. Sprint #3417.
pub async fn segment_docs_lorenz(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_curve": [], "total_segments": 0}));
    }
    let mut docs_vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_vals.sort_unstable();
    let total: u64 = docs_vals.iter().sum();
    let lorenz_points: Vec<serde_json::Value> = docs_vals.iter().enumerate().scan(0u64, |acc, (i, d)| {
        *acc += d;
        Some(serde_json::json!({
            "cumulative_population": (i + 1) as f64 / n as f64,
            "cumulative_share": if total > 0 { *acc as f64 / total as f64 } else { 0.0 }
        }))
    }).collect();
    Json(serde_json::json!({"lorenz_curve": lorenz_points, "total_docs": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-normalized-entropy — entropia normalizada de num_docs entre segmentos. Sprint #3418.
pub async fn segment_docs_normalized_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"docs_normalized_entropy": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let total: f64 = vals.iter().sum();
    let norm_entropy = if total == 0.0 { 0.0 } else {
        let entropy: f64 = vals.iter().map(|&v| { let p = v / total; if p > 0.0 { -p * p.ln() } else { 0.0 } }).sum();
        entropy / (n as f64).ln()
    };
    Json(serde_json::json!({"docs_normalized_entropy": norm_entropy, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-normalized-entropy — entropia normalizada de disk_bytes entre segmentos. Sprint #3437.
pub async fn segment_bytes_normalized_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"bytes_normalized_entropy": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let total: f64 = vals.iter().sum();
    let norm_entropy = if total == 0.0 { 0.0 } else {
        let entropy: f64 = vals.iter().map(|&v| { let p = v / total; if p > 0.0 { -p * p.ln() } else { 0.0 } }).sum();
        entropy / (n as f64).ln()
    };
    Json(serde_json::json!({"bytes_normalized_entropy": norm_entropy, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-trimmed-mean — média aparada de num_docs entre segmentos. Sprint #3438.
pub async fn segment_docs_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_trimmed_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
    let trimmed_mean = if n < 2 { vals[0] } else {
        let trim = (n as f64 * 0.1) as usize;
        let t = &vals[trim..n - trim];
        if t.is_empty() { 0.0 } else { t.iter().sum::<f64>() / t.len() as f64 }
    };
    Json(serde_json::json!({"docs_trimmed_mean": trimmed_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-harmonic-mean — média harmônica de num_docs entre segmentos. Sprint #3439.
pub async fn segment_docs_harmonic_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_harmonic_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).filter(|&v| v > 0.0).collect();
    let harmonic_mean = if vals.is_empty() { 0.0 } else {
        let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
        if recip_sum == 0.0 { 0.0 } else { n as f64 / recip_sum }
    };
    Json(serde_json::json!({"docs_harmonic_mean": harmonic_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-geometric-mean — média geométrica de num_docs entre segmentos. Sprint #3440.
pub async fn segment_docs_geometric_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_geometric_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).filter(|&v| v > 0.0).collect();
    let geometric_mean = if vals.is_empty() { 0.0 } else {
        (vals.iter().map(|&v| v.ln()).sum::<f64>() / n as f64).exp()
    };
    Json(serde_json::json!({"docs_geometric_mean": geometric_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-atkinson — índice de Atkinson de disk_bytes entre segmentos. Sprint #2863.
pub async fn segment_bytes_atkinson(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_atkinson": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let nonzero: Vec<f64> = vals.iter().copied().filter(|&v| v > 0.0).collect();
    let atkinson = if mean > 0.0 && !nonzero.is_empty() {
        let log_sum: f64 = nonzero.iter().map(|v| v.ln()).sum();
        let geo_mean = (log_sum / n as f64).exp();
        1.0 - geo_mean / mean
    } else { 0.0 };
    Json(serde_json::json!({"bytes_atkinson": atkinson, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-mad — MAD de disk_bytes entre segmentos. Sprint #2868.
pub async fn segment_bytes_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_mad": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2.0 };
    let mut abs_devs: Vec<f64> = vals.iter().map(|v| (v - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = if n % 2 == 1 { abs_devs[n / 2] } else { (abs_devs[n / 2 - 1] + abs_devs[n / 2]) / 2.0 };
    Json(serde_json::json!({"bytes_mad": mad, "bytes_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-winsorized-mean — média winsorizada (10%–90%) de doc_count entre segmentos. Sprint #3457.
pub async fn segment_docs_winsorized_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_winsorized_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p10 = vals[(n as f64 * 0.10) as usize];
    let p90 = vals[((n as f64 * 0.90) as usize).min(n - 1)];
    let clamped: Vec<f64> = vals.iter().map(|&v| v.clamp(p10, p90)).collect();
    let winsorized_mean = clamped.iter().sum::<f64>() / clamped.len() as f64;
    Json(serde_json::json!({"docs_winsorized_mean": winsorized_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-harmonic-mean — média harmônica de disk_bytes entre segmentos. Sprint #3458.
pub async fn segment_bytes_harmonic_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_harmonic_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).filter(|&v| v > 0.0).collect();
    let m = vals.len();
    let harmonic_mean = if m == 0 { 0.0 } else {
        let recip_sum: f64 = vals.iter().map(|&v| 1.0 / v).sum();
        if recip_sum == 0.0 { 0.0 } else { m as f64 / recip_sum }
    };
    Json(serde_json::json!({"bytes_harmonic_mean": harmonic_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-mad — MAD de doc_count entre segmentos. Sprint #3459.
pub async fn segment_docs_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_mad": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2.0 };
    let mut abs_devs: Vec<f64> = vals.iter().map(|v| (v - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = if n % 2 == 1 { abs_devs[n / 2] } else { (abs_devs[n / 2 - 1] + abs_devs[n / 2]) / 2.0 };
    Json(serde_json::json!({"docs_mad": mad, "docs_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-coeff-var — coeficiente de variação de doc_count entre segmentos. Sprint #3460.
pub async fn segment_docs_coeff_var(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_coeff_var": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let coeff_var = if mean == 0.0 { 0.0 } else {
        let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
        variance.sqrt() / mean
    };
    Json(serde_json::json!({"docs_coeff_var": coeff_var, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-coeff-var — coeficiente de variação de disk_bytes entre segmentos. Sprint #3477.
pub async fn segment_bytes_coeff_var(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_coeff_var": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let coeff_var = if mean == 0.0 { 0.0 } else {
        let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
        variance.sqrt() / mean
    };
    Json(serde_json::json!({"bytes_coeff_var": coeff_var, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-winsorized-stdev — desvio padrão winsorizado de disk_bytes. Sprint #3478.
pub async fn segment_bytes_winsorized_stdev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_winsorized_stdev": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p10 = vals[(n as f64 * 0.10) as usize];
    let p90 = vals[((n as f64 * 0.90) as usize).min(n - 1)];
    let clamped: Vec<f64> = vals.iter().map(|&v| v.clamp(p10, p90)).collect();
    let mean = clamped.iter().sum::<f64>() / clamped.len() as f64;
    let variance = clamped.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / clamped.len() as f64;
    Json(serde_json::json!({"bytes_winsorized_stdev": variance.sqrt(), "bytes_winsorized_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-winsorized-stdev — desvio padrão winsorizado de doc_count. Sprint #3479.
pub async fn segment_docs_winsorized_stdev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_winsorized_stdev": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p10 = vals[(n as f64 * 0.10) as usize];
    let p90 = vals[((n as f64 * 0.90) as usize).min(n - 1)];
    let clamped: Vec<f64> = vals.iter().map(|&v| v.clamp(p10, p90)).collect();
    let mean = clamped.iter().sum::<f64>() / clamped.len() as f64;
    let variance = clamped.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / clamped.len() as f64;
    Json(serde_json::json!({"docs_winsorized_stdev": variance.sqrt(), "docs_winsorized_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-trimmed-stdev — desvio padrão aparado de doc_count. Sprint #3480.
pub async fn segment_docs_trimmed_stdev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_trimmed_stdev": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = (n as f64 * 0.10).floor() as usize;
    let hi = (n as f64 * 0.90).ceil() as usize;
    let hi = hi.min(n);
    let trimmed_stdev = if lo < hi {
        let slice = &vals[lo..hi];
        let mean = slice.iter().sum::<f64>() / slice.len() as f64;
        let variance = slice.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / slice.len() as f64;
        variance.sqrt()
    } else { 0.0 };
    Json(serde_json::json!({"docs_trimmed_stdev": trimmed_stdev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-trimmed-stdev — desvio padrão aparado de disk_bytes. Sprint #3497.
pub async fn segment_bytes_trimmed_stdev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_trimmed_stdev": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = (n as f64 * 0.10).floor() as usize;
    let hi = (n as f64 * 0.90).ceil() as usize;
    let hi = hi.min(n);
    let trimmed_stdev = if lo < hi {
        let slice = &vals[lo..hi];
        let mean = slice.iter().sum::<f64>() / slice.len() as f64;
        let variance = slice.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / slice.len() as f64;
        variance.sqrt()
    } else { 0.0 };
    Json(serde_json::json!({"bytes_trimmed_stdev": trimmed_stdev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-p05 — P5 de doc_count entre segmentos. Sprint #3498.
pub async fn segment_docs_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_p05": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p05 = vals[((n as f64 * 0.05) as usize).min(n - 1)];
    Json(serde_json::json!({"docs_p05": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-p05 — P5 de disk_bytes entre segmentos. Sprint #3499.
pub async fn segment_bytes_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_p05": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p05 = vals[((n as f64 * 0.05) as usize).min(n - 1)];
    Json(serde_json::json!({"bytes_p05": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-iqr-ratio — razão IQR/mediana de doc_count entre segmentos. Sprint #3500.
pub async fn segment_docs_iqr_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_iqr_ratio": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = vals[((n as f64 * 0.25) as usize).min(n - 1)];
    let q3 = vals[((n as f64 * 0.75) as usize).min(n - 1)];
    let median = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2.0 };
    let iqr_ratio = if median == 0.0 { 0.0 } else { (q3 - q1) / median };
    Json(serde_json::json!({"docs_iqr_ratio": iqr_ratio, "docs_q1": q1, "docs_q3": q3, "docs_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-iqr-ratio — razão IQR/mediana de disk_bytes entre segmentos. Sprint #3517.
pub async fn segment_bytes_iqr_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_iqr_ratio": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = vals[((n as f64 * 0.25) as usize).min(n - 1)];
    let q3 = vals[((n as f64 * 0.75) as usize).min(n - 1)];
    let median = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2.0 };
    let iqr_ratio = if median == 0.0 { 0.0 } else { (q3 - q1) / median };
    Json(serde_json::json!({"bytes_iqr_ratio": iqr_ratio, "bytes_q1": q1, "bytes_q3": q3, "bytes_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-mean — soma de doc_count dos segmentos acima da média. Sprint #3518.
pub async fn segment_docs_sum_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_sum_above_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let sum_above: f64 = vals.iter().filter(|&&v| v > mean).sum();
    let count_above = vals.iter().filter(|&&v| v > mean).count();
    Json(serde_json::json!({"docs_sum_above_mean": sum_above, "docs_count_above_mean": count_above, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-mean — soma de disk_bytes dos segmentos acima da média. Sprint #3519.
pub async fn segment_bytes_sum_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_sum_above_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let sum_above: f64 = vals.iter().filter(|&&v| v > mean).sum();
    let count_above = vals.iter().filter(|&&v| v > mean).count();
    Json(serde_json::json!({"bytes_sum_above_mean": sum_above, "bytes_count_above_mean": count_above, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p99-bytes — fração de segmentos acima do P99 de disk_bytes. Sprint #3520.
pub async fn segment_ratio_above_p99_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p99_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p99 = vals[((n as f64 * 0.99) as usize).min(n - 1)];
    let count_above = vals.iter().filter(|&&v| v > p99).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"ratio_above_p99_bytes": ratio, "count_above_p99": count_above, "bytes_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-trimmed-mean — média aparada (10%–90%) de disk_bytes entre segmentos. Sprint #2873.
pub async fn segment_bytes_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_trimmed_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo = (n as f64 * 0.10).floor() as usize;
    let hi = (n as f64 * 0.90).ceil() as usize;
    let hi = hi.min(n);
    let trimmed_mean = if lo < hi {
        let slice = &vals[lo..hi];
        slice.iter().sum::<f64>() / slice.len() as f64
    } else { vals[n / 2] };
    Json(serde_json::json!({"bytes_trimmed_mean": trimmed_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-winsorized-mean — média winsorizada (10%–90%) de disk_bytes entre segmentos. Sprint #2878.
pub async fn segment_bytes_winsorized_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_winsorized_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lo_val = vals[((n as f64 * 0.10).floor() as usize).min(n - 1)];
    let hi_val = vals[((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1)];
    let clamped: Vec<f64> = vals.iter().map(|&v| v.max(lo_val).min(hi_val)).collect();
    let winsorized_mean = clamped.iter().sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_winsorized_mean": winsorized_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-geometric-mean — média geométrica de disk_bytes entre segmentos. Sprint #2883.
pub async fn segment_bytes_geometric_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_geometric_mean": null, "total_segments": 0}));
    }
    let nonzero: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).filter(|&v| v > 0.0).collect();
    let geometric_mean = if nonzero.is_empty() {
        0.0
    } else {
        let log_sum: f64 = nonzero.iter().map(|v| v.ln()).sum();
        (log_sum / nonzero.len() as f64).exp()
    };
    Json(serde_json::json!({"bytes_geometric_mean": geometric_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-lorenz — curva de Lorenz de densidade docs/byte. Sprint #2948.
pub async fn segment_docs_density_lorenz(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_curve": [], "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = vals.iter().sum();
    let lorenz: Vec<serde_json::Value> = vals.iter().enumerate().scan(0.0f64, |acc, (i, v)| {
        *acc += v;
        Some(serde_json::json!({
            "cumulative_population": (i + 1) as f64 / n as f64,
            "cumulative_share": if total > 0.0 { *acc / total } else { 0.0 }
        }))
    }).collect();
    Json(serde_json::json!({"lorenz_curve": lorenz, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-theil — índice Theil de densidade docs/byte. Sprint #2953.
pub async fn segment_docs_density_theil(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_theil": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let theil = if mean > 0.0 {
        vals.iter().map(|&x| if x > 0.0 { (x / mean) * (x / mean).ln() } else { 0.0 }).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"docs_density_theil": theil, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-mad — MAD da densidade docs/byte. Sprint #2958.
pub async fn segment_docs_density_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_mad": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2.0 };
    let mut abs_devs: Vec<f64> = vals.iter().map(|v| (v - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = if n % 2 == 1 { abs_devs[n / 2] } else { (abs_devs[n / 2 - 1] + abs_devs[n / 2]) / 2.0 };
    Json(serde_json::json!({"docs_density_mad": mad, "docs_density_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-entropy — entropia de densidade docs/byte. Sprint #2963.
pub async fn segment_docs_density_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_entropy": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let total: f64 = vals.iter().sum();
    let entropy = if total > 0.0 {
        vals.iter().map(|&v| {
            let p = v / total;
            if p > 0.0 { -p * p.ln() } else { 0.0 }
        }).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"docs_density_entropy": entropy, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-segment-mean — média de bytes por segmento. Sprint #3043.
pub async fn segment_bytes_per_segment_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_segment_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(_, _, bytes)| *bytes as f64).sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_per_segment_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-per-segment-mean — média de docs por segmento. Sprint #3038.
pub async fn segment_docs_per_segment_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_per_segment_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(_, docs, _)| *docs as f64).sum::<f64>() / n as f64;
    Json(serde_json::json!({"docs_per_segment_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-variance — variância de docs_density. Sprint #3033.
pub async fn segment_docs_density_variance(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_variance": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"docs_density_variance": variance, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-max — máximo de bytes_per_doc entre segmentos. Sprint #3028.
pub async fn segment_bytes_per_doc_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_max": null, "total_segments": 0}));
    }
    let max = segs.iter()
        .map(|(_, docs, bytes)| if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 })
        .fold(f64::NEG_INFINITY, f64::max);
    Json(serde_json::json!({"bytes_per_doc_max": max, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-count — contagem de segmentos com bytes_per_doc > 0. Sprint #3023.
pub async fn segment_bytes_per_doc_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let count = segs.iter().filter(|(_, docs, bytes)| *docs > 0 && *bytes > 0).count();
    Json(serde_json::json!({"bytes_per_doc_nonzero_count": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-sum — soma de bytes_per_doc por segmento. Sprint #3018.
pub async fn segment_bytes_per_doc_sum(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let sum: f64 = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).sum();
    Json(serde_json::json!({"bytes_per_doc_sum": sum, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-stddev — desvio padrão de bytes_per_doc. Sprint #3013.
pub async fn segment_bytes_per_doc_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_stddev": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"bytes_per_doc_stddev": variance.sqrt(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-count — contagem de segmentos com docs_density > 0. Sprint #3008.
pub async fn segment_docs_density_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let count = segs.iter().filter(|(_, docs, bytes)| *docs > 0 && *bytes > 0).count();
    Json(serde_json::json!({"docs_density_nonzero_count": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-sum — soma de densidade docs/byte. Sprint #3003.
pub async fn segment_docs_density_sum(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let sum: f64 = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).sum();
    Json(serde_json::json!({"docs_density_sum": sum, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-mean — média de densidade docs/byte. Sprint #2998.
pub async fn segment_docs_density_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    Json(serde_json::json!({"docs_density_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-p99 — P99 de densidade docs/byte. Sprint #2993.
pub async fn segment_docs_density_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"docs_density_p99": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-p95 — P95 de densidade docs/byte. Sprint #2988.
pub async fn segment_docs_density_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"docs_density_p95": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-harmonic-mean — média harmônica de densidade docs/byte. Sprint #2983.
pub async fn segment_docs_density_harmonic_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_harmonic_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let recip_sum: f64 = vals.iter().map(|&v| if v > 0.0 { 1.0 / v } else { 0.0 }).sum();
    let harmonic_mean = if recip_sum > 0.0 { n as f64 / recip_sum } else { 0.0 };
    Json(serde_json::json!({"docs_density_harmonic_mean": harmonic_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-geometric-mean — média geométrica de densidade docs/byte. Sprint #2978.
pub async fn segment_docs_density_geometric_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_geometric_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let log_sum: f64 = vals.iter().map(|&v| if v > 0.0 { v.ln() } else { f64::NEG_INFINITY }).sum();
    let geometric_mean = if log_sum.is_finite() { (log_sum / n as f64).exp() } else { 0.0 };
    Json(serde_json::json!({"docs_density_geometric_mean": geometric_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-winsorized-mean — média winsorizada (10%) de densidade docs/byte. Sprint #2973.
pub async fn segment_docs_density_winsorized_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_winsorized_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let trim = (n as f64 * 0.1).floor() as usize;
    let lo = if trim < n { vals[trim] } else { vals[0] };
    let hi = if trim < n { vals[n - 1 - trim] } else { vals[n - 1] };
    let winsorized: Vec<f64> = vals.iter().map(|&v| v.max(lo).min(hi)).collect();
    let mean = winsorized.iter().sum::<f64>() / n as f64;
    Json(serde_json::json!({"docs_density_winsorized_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-trimmed-mean — média aparada (10%) de densidade docs/byte. Sprint #2968.
pub async fn segment_docs_density_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_trimmed_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let trim = (n as f64 * 0.1).floor() as usize;
    let trimmed: &[f64] = if 2 * trim < n { &vals[trim..n - trim] } else { &vals };
    let trimmed_mean = if trimmed.is_empty() { 0.0 } else { trimmed.iter().sum::<f64>() / trimmed.len() as f64 };
    Json(serde_json::json!({"docs_density_trimmed_mean": trimmed_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-iqr — IQR de bytes/doc entre segmentos. Sprint #2928.
pub async fn segment_bytes_per_doc_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"bytes_per_doc_iqr": null, "total_segments": n}));
    }
    let mut vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = vals.len();
    if m < 4 {
        return Json(serde_json::json!({"bytes_per_doc_iqr": null, "total_segments": n}));
    }
    let q1 = vals[m / 4];
    let q3 = vals[3 * m / 4];
    Json(serde_json::json!({"bytes_per_doc_iqr": q3 - q1, "q1": q1, "q3": q3, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-gini — índice Gini de densidade docs/byte. Sprint #2933.
pub async fn segment_docs_density_gini(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_gini": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let sum: f64 = vals.iter().sum();
    let gini = if n > 1 && sum > 0.0 {
        let mut rank_sum = 0.0f64;
        for (i, v) in vals.iter().enumerate() {
            rank_sum += (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v;
        }
        rank_sum / (n as f64 * sum)
    } else { 0.0 };
    Json(serde_json::json!({"docs_density_gini": gini, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-hhi — índice HHI de densidade docs/byte. Sprint #2938.
pub async fn segment_docs_density_hhi(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_hhi": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let total: f64 = vals.iter().sum();
    let hhi = if total > 0.0 {
        vals.iter().map(|v| (v / total).powi(2)).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"docs_density_hhi": hhi, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-atkinson — índice Atkinson de densidade docs/byte. Sprint #2943.
pub async fn segment_docs_density_atkinson(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_density_atkinson": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).filter(|&v| v > 0.0).collect();
    let m = vals.len();
    let atkinson = if m > 1 {
        let mean = vals.iter().sum::<f64>() / m as f64;
        let log_sum: f64 = vals.iter().map(|v| v.ln()).sum();
        let geo_mean = (log_sum / m as f64).exp();
        if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
    } else { 0.0 };
    Json(serde_json::json!({"docs_density_atkinson": atkinson, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-mad — MAD de bytes/doc. Sprint #2908.
pub async fn segment_bytes_per_doc_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_mad": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = vals.len();
    if m == 0 {
        return Json(serde_json::json!({"bytes_per_doc_mad": null, "total_segments": n}));
    }
    let median = if m % 2 == 1 { vals[m / 2] } else { (vals[m / 2 - 1] + vals[m / 2]) / 2.0 };
    let mut abs_devs: Vec<f64> = vals.iter().map(|v| (v - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = if m % 2 == 1 { abs_devs[m / 2] } else { (abs_devs[m / 2 - 1] + abs_devs[m / 2]) / 2.0 };
    Json(serde_json::json!({"bytes_per_doc_mad": mad, "bytes_per_doc_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-atkinson — índice Atkinson de bytes/doc. Sprint #2913.
pub async fn segment_bytes_per_doc_atkinson(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_atkinson": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).filter(|&v| v > 0.0).collect();
    let m = vals.len();
    let atkinson = if m > 1 {
        let mean = vals.iter().sum::<f64>() / m as f64;
        let log_sum: f64 = vals.iter().map(|v| v.ln()).sum();
        let geo_mean = (log_sum / m as f64).exp();
        if mean > 0.0 { 1.0 - geo_mean / mean } else { 0.0 }
    } else { 0.0 };
    Json(serde_json::json!({"bytes_per_doc_atkinson": atkinson, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-lorenz — curva de Lorenz de bytes/doc. Sprint #2918.
pub async fn segment_bytes_per_doc_lorenz(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_curve": [], "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = vals.len();
    let total: f64 = vals.iter().sum();
    let lorenz: Vec<serde_json::Value> = vals.iter().enumerate().scan(0.0f64, |acc, (i, v)| {
        *acc += v;
        Some(serde_json::json!({
            "cumulative_population": (i + 1) as f64 / m as f64,
            "cumulative_share": if total > 0.0 { *acc / total } else { 0.0 }
        }))
    }).collect();
    Json(serde_json::json!({"lorenz_curve": lorenz, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-hhi — índice HHI de bytes/doc. Sprint #2923.
pub async fn segment_bytes_per_doc_hhi(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_hhi": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).collect();
    let m = vals.len();
    let total: f64 = vals.iter().sum();
    let hhi = if total > 0.0 {
        vals.iter().map(|v| (v / total).powi(2)).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"bytes_per_doc_hhi": hhi, "total_segments": n, "segment_count_with_docs": m}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-harmonic-mean — média harmônica de bytes/doc. Sprint #2888.
pub async fn segment_bytes_per_doc_harmonic_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_harmonic_mean": null, "total_segments": 0}));
    }
    let ratios: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).collect();
    let m = ratios.len();
    let harmonic_mean = if m == 0 { 0.0 } else {
        let recip_sum: f64 = ratios.iter().map(|v| 1.0 / v).sum();
        m as f64 / recip_sum
    };
    Json(serde_json::json!({"bytes_per_doc_harmonic_mean": harmonic_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-trimmed-mean — média aparada de bytes/doc. Sprint #2893.
pub async fn segment_bytes_per_doc_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_trimmed_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = vals.len();
    let trimmed_mean = if m == 0 {
        0.0
    } else {
        let lo = (m as f64 * 0.10).floor() as usize;
        let hi = ((m as f64 * 0.90).ceil() as usize).min(m);
        if lo < hi { vals[lo..hi].iter().sum::<f64>() / (hi - lo) as f64 } else { vals[m / 2] }
    };
    Json(serde_json::json!({"bytes_per_doc_trimmed_mean": trimmed_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-winsorized-mean — média winsorizada de bytes/doc. Sprint #2898.
pub async fn segment_bytes_per_doc_winsorized_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_winsorized_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = vals.len();
    let winsorized_mean = if m == 0 {
        0.0
    } else {
        let lo_val = vals[((m as f64 * 0.10).floor() as usize).min(m - 1)];
        let hi_val = vals[((m as f64 * 0.90).ceil() as usize).saturating_sub(1).min(m - 1)];
        let clamped: Vec<f64> = vals.iter().map(|&v| v.max(lo_val).min(hi_val)).collect();
        clamped.iter().sum::<f64>() / m as f64
    };
    Json(serde_json::json!({"bytes_per_doc_winsorized_mean": winsorized_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc-geometric-mean — média geométrica de bytes/doc. Sprint #2903.
pub async fn segment_bytes_per_doc_geometric_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc_geometric_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().filter_map(|(_, d, b)| {
        if *d > 0 { Some(*b as f64 / *d as f64) } else { None }
    }).filter(|&v| v > 0.0).collect();
    let m = vals.len();
    let geo_mean = if m == 0 { 0.0 } else {
        let log_sum: f64 = vals.iter().map(|v| v.ln()).sum();
        (log_sum / m as f64).exp()
    };
    Json(serde_json::json!({"bytes_per_doc_geometric_mean": geo_mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/kurtosis-bytes — curtose de bytes entre segmentos. Sprint #2638.
pub async fn segment_kurtosis_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"kurtosis_bytes": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let kurtosis = if stddev > 0.0 {
        vals.iter().map(|v| ((v - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
    } else { 0.0 };
    Json(serde_json::json!({"kurtosis_bytes": kurtosis, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/kurtosis-docs — curtose de docs entre segmentos. Sprint #2643.
pub async fn segment_kurtosis_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"kurtosis_docs": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let kurtosis = if stddev > 0.0 {
        vals.iter().map(|v| ((v - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
    } else { 0.0 };
    Json(serde_json::json!({"kurtosis_docs": kurtosis, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p95-bytes — P95 de bytes entre segmentos. Sprint #2608.
pub async fn segment_p95_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = (n * 19 / 20).min(n - 1);
    Json(serde_json::json!({"p95_bytes": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p95-docs — P95 de docs entre segmentos. Sprint #2613.
pub async fn segment_p95_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p95_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = (n * 19 / 20).min(n - 1);
    Json(serde_json::json!({"p95_docs": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p99-bytes — P99 de bytes entre segmentos. Sprint #2618.
pub async fn segment_p99_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = (n * 99 / 100).min(n - 1);
    Json(serde_json::json!({"p99_bytes": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p99-docs — P99 de docs entre segmentos. Sprint #2623.
pub async fn segment_p99_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p99_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = (n * 99 / 100).min(n - 1);
    Json(serde_json::json!({"p99_docs": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p99-docs — fração de segmentos acima do P99 de docs. Sprint #3537.
pub async fn segment_ratio_above_p99_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p99_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p99 = vals[(n * 99 / 100).min(n - 1)];
    let count_above = vals.iter().filter(|&&v| v > p99).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"ratio_above_p99_docs": ratio, "count_above_p99": count_above, "docs_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-mean — soma de docs dos segmentos abaixo da média. Sprint #3538.
pub async fn segment_docs_sum_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_sum_below_mean": 0, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let sum_below: f64 = vals.iter().filter(|&&v| v < mean).sum();
    Json(serde_json::json!({"docs_sum_below_mean": sum_below, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-below-mean — soma de bytes dos segmentos abaixo da média. Sprint #3539.
pub async fn segment_bytes_sum_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_sum_below_mean": 0, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let sum_below: f64 = vals.iter().filter(|&&v| v < mean).sum();
    Json(serde_json::json!({"bytes_sum_below_mean": sum_below, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p05-bytes — fração de segmentos acima do P05 de bytes. Sprint #3540.
pub async fn segment_ratio_above_p05_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p05_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p05 = vals[(n * 5 / 100).min(n - 1)];
    let count_above = vals.iter().filter(|&&v| v > p05).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"ratio_above_p05_bytes": ratio, "count_above_p05": count_above, "bytes_p05": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p05-docs — fração de segmentos acima do P05 de docs. Sprint #3557.
pub async fn segment_ratio_above_p05_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p05_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p05 = vals[(n * 5 / 100).min(n - 1)];
    let count_above = vals.iter().filter(|&&v| v > p05).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"ratio_above_p05_docs": ratio, "count_above_p05": count_above, "docs_p05": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-mean — nº de segmentos abaixo da média de docs. Sprint #3558.
pub async fn segment_docs_count_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_below_mean": 0, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let count_below = vals.iter().filter(|&&v| v < mean).count();
    Json(serde_json::json!({"docs_count_below_mean": count_below, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-mean — nº de segmentos abaixo da média de bytes. Sprint #3559.
pub async fn segment_bytes_count_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_below_mean": 0, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let count_below = vals.iter().filter(|&&v| v < mean).count();
    Json(serde_json::json!({"bytes_count_below_mean": count_below, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-below-mean — fração de segmentos abaixo da média de docs. Sprint #3560.
pub async fn segment_docs_ratio_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_below_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let count_below = vals.iter().filter(|&&v| v < mean).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_below_mean": ratio, "count_below_mean": count_below, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-below-mean — fração de segmentos abaixo da média de bytes. Sprint #3577.
pub async fn segment_bytes_ratio_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_below_mean": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let count_below = vals.iter().filter(|&&v| v < mean).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_below_mean": ratio, "count_below_mean": count_below, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p25 — nº de segmentos abaixo do P25 de docs. Sprint #3578.
pub async fn segment_docs_count_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_below_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p25 = vals[(n * 25 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p25).count();
    Json(serde_json::json!({"docs_count_below_p25": count_below, "docs_p25": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p25 — nº de segmentos abaixo do P25 de bytes. Sprint #3579.
pub async fn segment_bytes_count_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_below_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p25 = vals[(n * 25 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p25).count();
    Json(serde_json::json!({"bytes_count_below_p25": count_below, "bytes_p25": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-below-p25 — fração de segmentos abaixo do P25 de docs. Sprint #3580.
pub async fn segment_docs_ratio_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_below_p25": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p25 = vals[(n * 25 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p25).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_below_p25": ratio, "count_below_p25": count_below, "docs_p25": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-below-p25 — fração de segmentos abaixo do P25 de bytes. Sprint #3597.
pub async fn segment_bytes_ratio_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_below_p25": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p25 = vals[(n * 25 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p25).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_below_p25": ratio, "count_below_p25": count_below, "bytes_p25": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p50 — nº de segmentos abaixo do P50 de docs. Sprint #3598.
pub async fn segment_docs_count_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_below_p50": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2 };
    let count_below = vals.iter().filter(|&&v| v < p50).count();
    Json(serde_json::json!({"docs_count_below_p50": count_below, "docs_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p50 — nº de segmentos abaixo do P50 de bytes. Sprint #3599.
pub async fn segment_bytes_count_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_below_p50": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2 };
    let count_below = vals.iter().filter(|&&v| v < p50).count();
    Json(serde_json::json!({"bytes_count_below_p50": count_below, "bytes_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-below-p50 — fração de segmentos abaixo do P50 de docs. Sprint #3600.
pub async fn segment_docs_ratio_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_below_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2 };
    let count_below = vals.iter().filter(|&&v| v < p50).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_below_p50": ratio, "count_below_p50": count_below, "docs_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-below-p50 — fração de segmentos abaixo do P50 de bytes. Sprint #3617.
pub async fn segment_bytes_ratio_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_below_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 1 { vals[n / 2] } else { (vals[n / 2 - 1] + vals[n / 2]) / 2 };
    let count_below = vals.iter().filter(|&&v| v < p50).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_below_p50": ratio, "count_below_p50": count_below, "bytes_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p75 — nº de segmentos abaixo do P75 de docs. Sprint #3618.
pub async fn segment_docs_count_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_below_p75": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p75 = vals[(n * 75 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p75).count();
    Json(serde_json::json!({"docs_count_below_p75": count_below, "docs_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p75 — nº de segmentos abaixo do P75 de bytes. Sprint #3619.
pub async fn segment_bytes_count_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_below_p75": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p75 = vals[(n * 75 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p75).count();
    Json(serde_json::json!({"bytes_count_below_p75": count_below, "bytes_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-below-p75 — fração de segmentos abaixo do P75 de docs. Sprint #3620.
pub async fn segment_docs_ratio_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_below_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p75 = vals[(n * 75 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p75).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_below_p75": ratio, "count_below_p75": count_below, "docs_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-below-p75 — fração de segmentos com bytes abaixo de P75. Sprint #3637.
pub async fn segment_bytes_ratio_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_below_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p75 = vals[(n * 75 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p75).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_below_p75": ratio, "count_below_p75": count_below, "bytes_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p90 — contagem de segmentos com docs abaixo de P90. Sprint #3638.
pub async fn segment_docs_count_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_below_p90": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p90 = vals[(n * 90 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p90).count();
    Json(serde_json::json!({"docs_count_below_p90": count_below, "docs_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p90 — contagem de segmentos com bytes abaixo de P90. Sprint #3639.
pub async fn segment_bytes_count_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_below_p90": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p90 = vals[(n * 90 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p90).count();
    Json(serde_json::json!({"bytes_count_below_p90": count_below, "bytes_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-below-p90 — fração de segmentos com docs abaixo de P90. Sprint #3640.
pub async fn segment_docs_ratio_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_below_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p90 = vals[(n * 90 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p90).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_below_p90": ratio, "count_below_p90": count_below, "docs_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-below-p90 — fração de segmentos com bytes abaixo de P90. Sprint #3657.
pub async fn segment_bytes_ratio_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_below_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p90 = vals[(n * 90 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p90).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_below_p90": ratio, "count_below_p90": count_below, "bytes_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p95 — contagem de segmentos com docs abaixo de P95. Sprint #3658.
pub async fn segment_docs_count_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_below_p95": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p95 = vals[(n * 95 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p95).count();
    Json(serde_json::json!({"docs_count_below_p95": count_below, "docs_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p95 — contagem de segmentos com bytes abaixo de P95. Sprint #3659.
pub async fn segment_bytes_count_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_below_p95": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p95 = vals[(n * 95 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p95).count();
    Json(serde_json::json!({"bytes_count_below_p95": count_below, "bytes_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-below-p95 — fração de segmentos com docs abaixo de P95. Sprint #3660.
pub async fn segment_docs_ratio_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_below_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p95 = vals[(n * 95 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p95).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_below_p95": ratio, "count_below_p95": count_below, "docs_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-below-p95 — fração de segmentos com bytes abaixo de P95. Sprint #3677.
pub async fn segment_bytes_ratio_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_below_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p95 = vals[(n * 95 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p95).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_below_p95": ratio, "count_below_p95": count_below, "bytes_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p99 — contagem de segmentos com docs abaixo de P99. Sprint #3678.
pub async fn segment_docs_count_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_below_p99": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p99 = vals[(n * 99 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p99).count();
    Json(serde_json::json!({"docs_count_below_p99": count_below, "docs_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p99 — contagem de segmentos com bytes abaixo de P99. Sprint #3679.
pub async fn segment_bytes_count_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_below_p99": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p99 = vals[(n * 99 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p99).count();
    Json(serde_json::json!({"bytes_count_below_p99": count_below, "bytes_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-below-p99 — fração de segmentos com docs abaixo de P99. Sprint #3680.
pub async fn segment_docs_ratio_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_below_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p99 = vals[(n * 99 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p99).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_below_p99": ratio, "count_below_p99": count_below, "docs_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-below-p99 — fração de segmentos com bytes abaixo de P99. Sprint #3697.
pub async fn segment_bytes_ratio_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_below_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p99 = vals[(n * 99 / 100).min(n - 1)];
    let count_below = vals.iter().filter(|&&v| v < p99).count();
    let ratio = count_below as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_below_p99": ratio, "count_below_p99": count_below, "bytes_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-mean — contagem de segmentos com docs acima da média. Sprint #3698.
pub async fn segment_docs_count_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_above_mean": 0, "total_segments": 0}));
    }
    let vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    let mean = vals.iter().sum::<u64>() as f64 / n as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > mean).count();
    Json(serde_json::json!({"docs_count_above_mean": count_above, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-mean — contagem de segmentos com bytes acima da média. Sprint #3699.
pub async fn segment_bytes_count_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_above_mean": 0, "total_segments": 0}));
    }
    let vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    let mean = vals.iter().sum::<u64>() as f64 / n as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > mean).count();
    Json(serde_json::json!({"bytes_count_above_mean": count_above, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-above-mean — fração de segmentos com docs acima da média. Sprint #3700.
pub async fn segment_docs_ratio_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_above_mean": null, "total_segments": 0}));
    }
    let vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    let mean = vals.iter().sum::<u64>() as f64 / n as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > mean).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_above_mean": ratio, "count_above_mean": count_above, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-above-mean — Fração de segmentos com bytes acima da média. Sprint #3717.
pub async fn segment_bytes_ratio_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_above_mean": null, "total_segments": 0}));
    }
    let vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    let mean = vals.iter().sum::<u64>() as f64 / n as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > mean).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_above_mean": ratio, "count_above_mean": count_above, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p50 — Segmentos com docs acima do P50. Sprint #3718.
pub async fn segment_docs_count_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_above_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let mid = n / 2;
    let p50 = if n % 2 == 0 { (vals[mid - 1] + vals[mid]) as f64 / 2.0 } else { vals[mid] as f64 };
    let count_above = vals.iter().filter(|&&v| v as f64 > p50).count();
    Json(serde_json::json!({"docs_count_above_p50": count_above, "docs_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p50 — Segmentos com bytes acima do P50. Sprint #3719.
pub async fn segment_bytes_count_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_above_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let mid = n / 2;
    let p50 = if n % 2 == 0 { (vals[mid - 1] + vals[mid]) as f64 / 2.0 } else { vals[mid] as f64 };
    let count_above = vals.iter().filter(|&&v| v as f64 > p50).count();
    Json(serde_json::json!({"bytes_count_above_p50": count_above, "bytes_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-above-p50 — Fração de segmentos com docs acima do P50. Sprint #3720.
pub async fn segment_docs_ratio_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_above_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let mid = n / 2;
    let p50 = if n % 2 == 0 { (vals[mid - 1] + vals[mid]) as f64 / 2.0 } else { vals[mid] as f64 };
    let count_above = vals.iter().filter(|&&v| v as f64 > p50).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_above_p50": ratio, "count_above_p50": count_above, "docs_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-above-p50 — Fração de segmentos com bytes acima do P50. Sprint #3737.
pub async fn segment_bytes_ratio_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_above_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let mid = n / 2;
    let p50 = if n % 2 == 0 { (vals[mid - 1] + vals[mid]) as f64 / 2.0 } else { vals[mid] as f64 };
    let count_above = vals.iter().filter(|&&v| v as f64 > p50).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_above_p50": ratio, "count_above_p50": count_above, "bytes_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p75 — Segmentos com docs acima do P75. Sprint #3738.
pub async fn segment_docs_count_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_above_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p75).count();
    Json(serde_json::json!({"docs_count_above_p75": count_above, "docs_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p75 — Segmentos com bytes acima do P75. Sprint #3739.
pub async fn segment_bytes_count_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_above_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p75).count();
    Json(serde_json::json!({"bytes_count_above_p75": count_above, "bytes_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-above-p75 — Fração de segmentos com docs acima do P75. Sprint #3740.
pub async fn segment_docs_ratio_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_above_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p75).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_above_p75": ratio, "count_above_p75": count_above, "docs_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-above-p75 — Fração de segmentos com bytes acima do P75. Sprint #3757.
pub async fn segment_bytes_ratio_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_above_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p75).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_above_p75": ratio, "count_above_p75": count_above, "bytes_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p90 — Segmentos com docs acima do P90. Sprint #3758.
pub async fn segment_docs_count_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_above_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1);
    let p90 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p90).count();
    Json(serde_json::json!({"docs_count_above_p90": count_above, "docs_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p90 — Segmentos com bytes acima do P90. Sprint #3759.
pub async fn segment_bytes_count_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_above_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1);
    let p90 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p90).count();
    Json(serde_json::json!({"bytes_count_above_p90": count_above, "bytes_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-above-p90 — Fração de segmentos com docs acima do P90. Sprint #3760.
pub async fn segment_docs_ratio_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_above_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1);
    let p90 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p90).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_above_p90": ratio, "count_above_p90": count_above, "docs_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-above-p90 — Fração de segmentos com bytes acima do P90. Sprint #3777.
pub async fn segment_bytes_ratio_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_above_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1);
    let p90 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p90).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_above_p90": ratio, "count_above_p90": count_above, "bytes_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p95 — Segmentos com docs acima do P95. Sprint #3778.
pub async fn segment_docs_count_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_above_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    let p95 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p95).count();
    Json(serde_json::json!({"docs_count_above_p95": count_above, "docs_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p95 — Segmentos com bytes acima do P95. Sprint #3779.
pub async fn segment_bytes_count_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_above_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    let p95 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p95).count();
    Json(serde_json::json!({"bytes_count_above_p95": count_above, "bytes_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-above-p95 — Fração de segmentos com docs acima do P95. Sprint #3780.
pub async fn segment_docs_ratio_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_above_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    let p95 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p95).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_above_p95": ratio, "count_above_p95": count_above, "docs_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-above-p95 — Fração de segmentos com bytes acima do P95. Sprint #3797.
pub async fn segment_bytes_ratio_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_above_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    let p95 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p95).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_above_p95": ratio, "count_above_p95": count_above, "bytes_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p99 — Segmentos com docs acima do P99. Sprint #3798.
pub async fn segment_docs_count_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_count_above_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    let p99 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p99).count();
    Json(serde_json::json!({"docs_count_above_p99": count_above, "docs_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p99 — Segmentos com bytes acima do P99. Sprint #3799.
pub async fn segment_bytes_count_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_count_above_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    let p99 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p99).count();
    Json(serde_json::json!({"bytes_count_above_p99": count_above, "bytes_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-above-p99 — Fração de segmentos com docs acima do P99. Sprint #3800.
pub async fn segment_docs_ratio_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_above_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    let p99 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p99).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"docs_ratio_above_p99": ratio, "count_above_p99": count_above, "docs_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-above-p99 — Fração de segmentos com bytes acima do P99. Sprint #3817.
pub async fn segment_bytes_ratio_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_above_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    let p99 = vals[idx] as f64;
    let count_above = vals.iter().filter(|&&v| v as f64 > p99).count();
    let ratio = count_above as f64 / n as f64;
    Json(serde_json::json!({"bytes_ratio_above_p99": ratio, "count_above_p99": count_above, "bytes_p99": p99, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-min — Segmento com mínimo de docs. Sprint #3818.
pub async fn segment_docs_count_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_docs": null, "total_segments": 0}));
    }
    let min_docs = segs.iter().map(|(_, d, _)| *d).min().unwrap_or(0);
    Json(serde_json::json!({"min_docs": min_docs, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-min — Segmento com mínimo de bytes. Sprint #3819.
pub async fn segment_bytes_count_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_bytes": null, "total_segments": 0}));
    }
    let min_bytes = segs.iter().map(|(_, _, b)| *b).min().unwrap_or(0);
    Json(serde_json::json!({"min_bytes": min_bytes, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-max — Segmento com máximo de docs. Sprint #3820.
pub async fn segment_docs_count_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_docs": null, "total_segments": 0}));
    }
    let max_docs = segs.iter().map(|(_, d, _)| *d).max().unwrap_or(0);
    Json(serde_json::json!({"max_docs": max_docs, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-max — número de segmentos acima do max de bytes. Sprint #3837.
pub async fn segment_bytes_count_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_bytes": null, "total_segments": 0}));
    }
    let max_bytes = segs.iter().map(|(_, _, b)| *b).max().unwrap_or(0);
    Json(serde_json::json!({"max_bytes": max_bytes, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-min — ratio mínimo de docs entre segmentos. Sprint #3838.
pub async fn segment_docs_ratio_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_min": null, "total_segments": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    if total_docs == 0 {
        return Json(serde_json::json!({"docs_ratio_min": 0.0, "total_segments": n}));
    }
    let min_ratio = segs.iter().map(|(_, d, _)| *d as f64 / total_docs as f64).fold(f64::INFINITY, f64::min);
    Json(serde_json::json!({"docs_ratio_min": min_ratio, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-min — ratio mínimo de bytes entre segmentos. Sprint #3839.
pub async fn segment_bytes_ratio_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_min": null, "total_segments": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    if total_bytes == 0 {
        return Json(serde_json::json!({"bytes_ratio_min": 0.0, "total_segments": n}));
    }
    let min_ratio = segs.iter().map(|(_, _, b)| *b as f64 / total_bytes as f64).fold(f64::INFINITY, f64::min);
    Json(serde_json::json!({"bytes_ratio_min": min_ratio, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-ratio-max — ratio máximo de docs entre segmentos. Sprint #3840.
pub async fn segment_docs_ratio_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_ratio_max": null, "total_segments": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    if total_docs == 0 {
        return Json(serde_json::json!({"docs_ratio_max": 0.0, "total_segments": n}));
    }
    let max_ratio = segs.iter().map(|(_, d, _)| *d as f64 / total_docs as f64).fold(f64::NEG_INFINITY, f64::max);
    Json(serde_json::json!({"docs_ratio_max": max_ratio, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-ratio-max — ratio máximo de bytes entre segmentos. Sprint #3857.
pub async fn segment_bytes_ratio_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_ratio_max": null, "total_segments": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    if total_bytes == 0 {
        return Json(serde_json::json!({"bytes_ratio_max": 0.0, "total_segments": n}));
    }
    let max_ratio = segs.iter().map(|(_, _, b)| *b as f64 / total_bytes as f64).fold(f64::NEG_INFINITY, f64::max);
    Json(serde_json::json!({"bytes_ratio_max": max_ratio, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-per-doc — média de bytes por documento entre segmentos. Sprint #3858.
pub async fn segment_bytes_per_doc(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_bytes_per_doc": null, "total_segments": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    let total_docs: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    let avg = if total_docs == 0 { 0.0 } else { total_bytes as f64 / total_docs as f64 };
    Json(serde_json::json!({"avg_bytes_per_doc": avg, "total_bytes": total_bytes, "total_docs": total_docs, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/total-docs-count — total de documentos em todos os segmentos. Sprint #3859.
pub async fn segment_total_docs_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let total_docs: u64 = segs.iter().map(|(_, d, _)| *d).sum();
    Json(serde_json::json!({"total_docs": total_docs, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/total-bytes-count — total de bytes em todos os segmentos. Sprint #3860.
pub async fn segment_total_bytes_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| *b).sum();
    Json(serde_json::json!({"total_bytes": total_bytes, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-std-dev — desvio padrão de docs entre segmentos. Sprint #3877.
pub async fn segment_docs_std_dev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_std_dev": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    Json(serde_json::json!({"docs_std_dev": std_dev, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-std-dev — desvio padrão de bytes entre segmentos. Sprint #3878.
pub async fn segment_bytes_std_dev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_std_dev": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    Json(serde_json::json!({"bytes_std_dev": std_dev, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-active — número de segmentos ativos (docs > 0). Sprint #3879.
pub async fn segment_count_active(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let active = segs.iter().filter(|(_, d, _)| *d > 0).count();
    Json(serde_json::json!({"active_segments": active, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-avg — comprimento médio do nome dos segmentos. Sprint #3880.
pub async fn segment_name_length_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_name_length": null, "total_segments": 0}));
    }
    let avg_len = segs.iter().map(|(name, _, _)| name.len() as f64).sum::<f64>() / n as f64;
    Json(serde_json::json!({"avg_name_length": avg_len, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-empty — número de segmentos sem documentos. Sprint #3897.
pub async fn segment_docs_count_empty(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let empty = segs.iter().filter(|(_, d, _)| *d == 0).count();
    Json(serde_json::json!({"empty_segments": empty, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-empty-ratio — ratio de segmentos sem documentos. Sprint #3898.
pub async fn segment_docs_empty_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"empty_ratio": null, "total_segments": 0}));
    }
    let empty = segs.iter().filter(|(_, d, _)| *d == 0).count();
    let ratio = empty as f64 / n as f64;
    Json(serde_json::json!({"empty_ratio": ratio, "empty_count": empty, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-empty — número de segmentos sem bytes. Sprint #3899.
pub async fn segment_bytes_count_empty(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let empty = segs.iter().filter(|(_, _, b)| *b == 0).count();
    Json(serde_json::json!({"zero_bytes_segments": empty, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-std-dev — desvio padrão do comprimento do nome dos segmentos. Sprint #3900.
pub async fn segment_name_length_std_dev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_std_dev": null, "total_segments": 0}));
    }
    let lens: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = lens.iter().sum::<f64>() / n as f64;
    let variance = lens.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let std_dev = variance.sqrt();
    Json(serde_json::json!({"name_length_std_dev": std_dev, "name_length_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-mean-ratio — ratio de segmentos com docs acima da média. Sprint #3917.
pub async fn segment_docs_count_above_mean_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_above_mean_ratio": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(_, d, _)| *d as f64).sum::<f64>() / n as f64;
    let above = segs.iter().filter(|(_, d, _)| *d as f64 > mean).count();
    let ratio = above as f64 / n as f64;
    Json(serde_json::json!({"docs_above_mean_ratio": ratio, "above_mean_count": above, "docs_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-mean-vs-median — comparação da média vs mediana de docs. Sprint #3918.
pub async fn segment_docs_mean_vs_median(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_mean": null, "docs_median": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) / 2.0 } else { vals[n / 2] };
    Json(serde_json::json!({"docs_mean": mean, "docs_median": median, "mean_over_median": if median == 0.0 { null } else { serde_json::json!(mean / median) }, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-mean-vs-median — comparação da média vs mediana de bytes. Sprint #3919.
pub async fn segment_bytes_mean_vs_median(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_mean": null, "bytes_median": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) / 2.0 } else { vals[n / 2] };
    Json(serde_json::json!({"bytes_mean": mean, "bytes_median": median, "mean_over_median": if median == 0.0 { null } else { serde_json::json!(mean / median) }, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-empty — total de docs em segmentos vazios (0 docs). Sprint #3920.
pub async fn segment_docs_sum_empty(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let empty_count = segs.iter().filter(|(_, d, _)| *d == 0).count();
    Json(serde_json::json!({"empty_segment_count": empty_count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-mean-ratio — ratio de segmentos com bytes acima da média. Sprint #3937.
pub async fn segment_bytes_count_above_mean_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_above_mean_ratio": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(_, _, b)| *b as f64).sum::<f64>() / n as f64;
    let above = segs.iter().filter(|(_, _, b)| *b as f64 > mean).count();
    let ratio = above as f64 / n as f64;
    Json(serde_json::json!({"bytes_above_mean_ratio": ratio, "above_mean_count": above, "bytes_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-trimmed-p95 — média trimmed (5-95%) de docs entre segmentos. Sprint #3938.
pub async fn segment_docs_trimmed_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_trimmed_mean_5_95": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lo = ((n as f64 * 0.05).ceil() as usize).min(n);
    let hi = ((n as f64 * 0.95).floor() as usize).min(n);
    let trimmed = &vals[lo..hi];
    let mean = if trimmed.is_empty() { 0.0 } else { trimmed.iter().sum::<f64>() / trimmed.len() as f64 };
    Json(serde_json::json!({"docs_trimmed_mean_5_95": mean, "trimmed_count": trimmed.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-single-doc — número de segmentos com exatamente 1 documento. Sprint #3939.
pub async fn segment_docs_count_single(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let single = segs.iter().filter(|(_, d, _)| *d == 1).count();
    Json(serde_json::json!({"single_doc_segments": single, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-trimmed-p95 — média trimmed (5-95%) de bytes entre segmentos. Sprint #3940.
pub async fn segment_bytes_trimmed_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_trimmed_mean_5_95": null, "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let lo = ((n as f64 * 0.05).ceil() as usize).min(n);
    let hi = ((n as f64 * 0.95).floor() as usize).min(n);
    let trimmed = &vals[lo..hi];
    let mean = if trimmed.is_empty() { 0.0 } else { trimmed.iter().sum::<f64>() / trimmed.len() as f64 };
    Json(serde_json::json!({"bytes_trimmed_mean_5_95": mean, "trimmed_count": trimmed.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-large — segmentos com docs acima de 1000. Sprint #3957.
pub async fn segment_docs_count_large(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let large = segs.iter().filter(|(_, d, _)| *d > 1000).count();
    Json(serde_json::json!({"large_segments": large, "threshold_docs": 1000, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-large — segmentos com bytes acima de 1MB. Sprint #3958.
pub async fn segment_bytes_count_large(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let large = segs.iter().filter(|(_, _, b)| *b > 1_048_576).count();
    Json(serde_json::json!({"large_segments": large, "threshold_bytes": 1_048_576, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p75 — soma de docs em segmentos acima do P75. Sprint #3959.
pub async fn segment_docs_sum_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_sum_above_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[idx];
    let sum: u64 = segs.iter().filter(|(_, d, _)| *d > p75).map(|(_, d, _)| *d).sum();
    let count = segs.iter().filter(|(_, d, _)| *d > p75).count();
    Json(serde_json::json!({"docs_sum_above_p75": sum, "count_above_p75": count, "docs_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-p75 — soma de bytes em segmentos acima do P75. Sprint #3960.
pub async fn segment_bytes_sum_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_sum_above_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[idx];
    let sum: u64 = segs.iter().filter(|(_, _, b)| *b > p75).map(|(_, _, b)| *b).sum();
    let count = segs.iter().filter(|(_, _, b)| *b > p75).count();
    Json(serde_json::json!({"bytes_sum_above_p75": sum, "count_above_p75": count, "bytes_p75": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p90 — soma de docs em segmentos acima do P90. Sprint #3977.
pub async fn segment_docs_sum_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_sum_above_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1);
    let p90 = vals[idx];
    let sum: u64 = segs.iter().filter(|(_, d, _)| *d > p90).map(|(_, d, _)| *d).sum();
    let count = segs.iter().filter(|(_, d, _)| *d > p90).count();
    Json(serde_json::json!({"docs_sum_above_p90": sum, "count_above_p90": count, "docs_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-p90 — soma de bytes em segmentos acima do P90. Sprint #3978.
pub async fn segment_bytes_sum_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_sum_above_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1);
    let p90 = vals[idx];
    let sum: u64 = segs.iter().filter(|(_, _, b)| *b > p90).map(|(_, _, b)| *b).sum();
    let count = segs.iter().filter(|(_, _, b)| *b > p90).count();
    Json(serde_json::json!({"bytes_sum_above_p90": sum, "count_above_p90": count, "bytes_p90": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p95 — soma de docs em segmentos acima do P95. Sprint #3979.
pub async fn segment_docs_sum_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_sum_above_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    let p95 = vals[idx];
    let sum: u64 = segs.iter().filter(|(_, d, _)| *d > p95).map(|(_, d, _)| *d).sum();
    let count = segs.iter().filter(|(_, d, _)| *d > p95).count();
    Json(serde_json::json!({"docs_sum_above_p95": sum, "count_above_p95": count, "docs_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-p95 — soma de bytes em segmentos acima do P95. Sprint #3980.
pub async fn segment_bytes_sum_above_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_sum_above_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    let p95 = vals[idx];
    let sum: u64 = segs.iter().filter(|(_, _, b)| *b > p95).map(|(_, _, b)| *b).sum();
    let count = segs.iter().filter(|(_, _, b)| *b > p95).count();
    Json(serde_json::json!({"bytes_sum_above_p95": sum, "count_above_p95": count, "bytes_p95": p95, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-trimmed-p90 — média aparada P5–P95 de docs entre segmentos. Sprint #3997.
pub async fn segment_docs_trimmed_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_trimmed_p90_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let lo = ((n as f64 * 0.05).ceil() as usize).min(n);
    let hi = ((n as f64 * 0.95).ceil() as usize).min(n);
    let trimmed = &vals[lo..hi];
    let mean = if trimmed.is_empty() { None } else {
        Some(trimmed.iter().sum::<u64>() as f64 / trimmed.len() as f64)
    };
    Json(serde_json::json!({"docs_trimmed_p90_mean": mean, "trimmed_count": trimmed.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-trimmed-p90 — média aparada P5–P95 de bytes entre segmentos. Sprint #3998.
pub async fn segment_bytes_trimmed_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_trimmed_p90_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let lo = ((n as f64 * 0.05).ceil() as usize).min(n);
    let hi = ((n as f64 * 0.95).ceil() as usize).min(n);
    let trimmed = &vals[lo..hi];
    let mean = if trimmed.is_empty() { None } else {
        Some(trimmed.iter().sum::<u64>() as f64 / trimmed.len() as f64)
    };
    Json(serde_json::json!({"bytes_trimmed_p90_mean": mean, "trimmed_count": trimmed.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-max — comprimento máximo de nome de segmento. Sprint #3999.
pub async fn segment_name_length_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let max = segs.iter().map(|(name, _, _)| name.len()).max();
    Json(serde_json::json!({"name_length_max": max, "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/name-length-min — comprimento mínimo de nome de segmento. Sprint #4000.
pub async fn segment_name_length_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let min = segs.iter().map(|(name, _, _)| name.len()).min();
    Json(serde_json::json!({"name_length_min": min, "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/name-length-skewness — skewness de comprimento de nome de segmento. Sprint #4037.
pub async fn segment_name_length_skewness(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"name_length_skewness": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let skewness = if stddev == 0.0 { 0.0 } else {
        vals.iter().map(|&v| ((v - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    };
    Json(serde_json::json!({"name_length_skewness": skewness, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-kurtosis — kurtosis de comprimento de nome de segmento. Sprint #4038.
pub async fn segment_name_length_kurtosis(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"name_length_kurtosis": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let kurtosis = if stddev == 0.0 { 0.0 } else {
        vals.iter().map(|&v| ((v - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
    };
    Json(serde_json::json!({"name_length_kurtosis": kurtosis, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-coeff-var — coeficiente de variação de comprimento de nome. Sprint #4039.
pub async fn segment_name_length_coeff_var(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"name_length_coeff_var": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let coeff_var = if mean == 0.0 { None } else { Some(stddev / mean) };
    Json(serde_json::json!({"name_length_coeff_var": coeff_var, "name_length_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-mad — MAD de comprimento de nome de segmento. Sprint #4040.
pub async fn segment_name_length_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_mad": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let median = if n % 2 == 0 {
        (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0
    } else {
        vals[n / 2] as f64
    };
    let mut abs_devs: Vec<f64> = vals.iter().map(|&v| (v as f64 - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = if n % 2 == 0 {
        (abs_devs[n / 2 - 1] + abs_devs[n / 2]) / 2.0
    } else {
        abs_devs[n / 2]
    };
    Json(serde_json::json!({"name_length_mad": mad, "name_length_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p95 — P95 de comprimento de nome de segmento. Sprint #4077.
pub async fn segment_name_length_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_p95": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.95).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"name_length_p95": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p99 — P99 de comprimento de nome de segmento. Sprint #4078.
pub async fn segment_name_length_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_p99": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.99).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"name_length_p99": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum — soma de comprimentos de nome de segmentos. Sprint #4079.
pub async fn segment_name_length_sum(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let sum: usize = segs.iter().map(|(name, _, _)| name.len()).sum();
    Json(serde_json::json!({"name_length_sum": sum, "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/name-length-mean — média de comprimento de nome de segmentos. Sprint #4080.
pub async fn segment_name_length_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let mean = if n == 0 { None } else {
        let sum: usize = segs.iter().map(|(name, _, _)| name.len()).sum();
        Some(sum as f64 / n as f64)
    };
    Json(serde_json::json!({"name_length_mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-mad — MAD de contagem de docs por segmento. Sprint #4097.
pub async fn segment_docs_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_mad": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let median = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let mut abs_devs: Vec<f64> = vals.iter().map(|&v| (v as f64 - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = if n % 2 == 0 { (abs_devs[n / 2 - 1] + abs_devs[n / 2]) / 2.0 } else { abs_devs[n / 2] };
    Json(serde_json::json!({"docs_mad": mad, "docs_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-mad — MAD de bytes por segmento. Sprint #4098.
pub async fn segment_bytes_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_mad": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let median = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let mut abs_devs: Vec<f64> = vals.iter().map(|&v| (v as f64 - median).abs()).collect();
    abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = if n % 2 == 0 { (abs_devs[n / 2 - 1] + abs_devs[n / 2]) / 2.0 } else { abs_devs[n / 2] };
    Json(serde_json::json!({"bytes_mad": mad, "bytes_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-range — range (max-min) de docs por segmento. Sprint #4099.
pub async fn segment_docs_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_range": null, "total_segments": 0}));
    }
    let vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    let min = vals.iter().copied().min().unwrap_or(0);
    let max = vals.iter().copied().max().unwrap_or(0);
    Json(serde_json::json!({"docs_range": max - min, "docs_min": min, "docs_max": max, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-range — range (max-min) de bytes por segmento. Sprint #4100.
pub async fn segment_bytes_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_range": null, "total_segments": 0}));
    }
    let vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    let min = vals.iter().copied().min().unwrap_or(0);
    let max = vals.iter().copied().max().unwrap_or(0);
    Json(serde_json::json!({"bytes_range": max - min, "bytes_min": min, "bytes_max": max, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-entropy — entropia de Shannon dos comprimentos de nome. Sprint #4117.
pub async fn segment_name_length_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_entropy": null, "total_segments": 0}));
    }
    let total = n as f64;
    let mut freq: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (name, _, _) in &segs { *freq.entry(name.len()).or_insert(0) += 1; }
    let entropy: f64 = freq.values().map(|&c| { let p = c as f64 / total; -p * p.ln() }).sum();
    Json(serde_json::json!({"name_length_entropy": entropy, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-gini — coeficiente Gini dos comprimentos de nome. Sprint #4118.
pub async fn segment_name_length_gini(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"name_length_gini": null, "total_segments": n}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let sum: f64 = vals.iter().sum();
    if sum == 0.0 {
        return Json(serde_json::json!({"name_length_gini": 0.0, "total_segments": n}));
    }
    let gini: f64 = vals.iter().enumerate().map(|(i, &v)| (2.0 * (i + 1) as f64 - n as f64 - 1.0) * v).sum::<f64>() / (n as f64 * sum);
    Json(serde_json::json!({"name_length_gini": gini, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-hhi — índice Herfindahl-Hirschman dos comprimentos de nome. Sprint #4119.
pub async fn segment_name_length_hhi(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_hhi": null, "total_segments": 0}));
    }
    let total: usize = segs.iter().map(|(name, _, _)| name.len()).sum();
    if total == 0 {
        return Json(serde_json::json!({"name_length_hhi": 0.0, "total_segments": n}));
    }
    let hhi: f64 = segs.iter().map(|(name, _, _)| { let s = name.len() as f64 / total as f64; s * s }).sum();
    Json(serde_json::json!({"name_length_hhi": hhi, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-theil — índice Theil T dos comprimentos de nome. Sprint #4120.
pub async fn segment_name_length_theil(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_theil": null, "total_segments": 0}));
    }
    let total: f64 = segs.iter().map(|(name, _, _)| name.len() as f64).sum();
    if total == 0.0 {
        return Json(serde_json::json!({"name_length_theil": 0.0, "total_segments": n}));
    }
    let mean = total / n as f64;
    let theil: f64 = segs.iter().map(|(name, _, _)| {
        let x = name.len() as f64;
        if x == 0.0 { 0.0 } else { (x / mean) * (x / mean).ln() }
    }).sum::<f64>() / n as f64;
    Json(serde_json::json!({"name_length_theil": theil, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-atkinson — índice Atkinson (ε=0.5) dos comprimentos de nome. Sprint #4137.
pub async fn segment_name_length_atkinson(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_atkinson": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    if mean == 0.0 {
        return Json(serde_json::json!({"name_length_atkinson": 0.0, "total_segments": n}));
    }
    let epsilon = 0.5_f64;
    let geometric_part: f64 = vals.iter().map(|&v| if v == 0.0 { 0.0 } else { v.powf(1.0 - epsilon) }).sum::<f64>() / n as f64;
    let atkinson = 1.0 - (geometric_part.powf(1.0 / (1.0 - epsilon))) / mean;
    Json(serde_json::json!({"name_length_atkinson": atkinson, "total_segments": n, "epsilon": epsilon}))
}

/// GET /api/v1/search/index/segments/name-length-lorenz — curva de Lorenz dos comprimentos de nome. Sprint #4138.
pub async fn segment_name_length_lorenz(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_curve": [], "total_segments": 0}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = vals.iter().sum();
    if total == 0.0 {
        return Json(serde_json::json!({"lorenz_curve": [], "total_segments": n}));
    }
    let curve: Vec<serde_json::Value> = vals.iter().enumerate().scan(0.0_f64, |cum, (i, &v)| {
        *cum += v;
        Some(serde_json::json!({
            "population_share": (i + 1) as f64 / n as f64,
            "name_length_share": *cum / total
        }))
    }).collect();
    Json(serde_json::json!({"lorenz_curve": curve, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-above-mean — segmentos com nome mais longo que a média. Sprint #4139.
pub async fn segment_name_length_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"above_mean_count": 0, "total_segments": 0, "mean_name_length": null}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| name.len() as f64 > mean).count();
    Json(serde_json::json!({"above_mean_count": count, "total_segments": n, "mean_name_length": mean}))
}

/// GET /api/v1/search/index/segments/name-length-above-p50 — segmentos com nome mais longo que a mediana. Sprint #4140.
pub async fn segment_name_length_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"above_p50_count": 0, "total_segments": 0, "p50_name_length": null}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let count = vals.iter().filter(|&&v| v as f64 > p50).count();
    Json(serde_json::json!({"above_p50_count": count, "total_segments": n, "p50_name_length": p50}))
}

/// GET /api/v1/search/index/segments/name-length-below-mean — segmentos com nome mais curto que a média. Sprint #4157.
pub async fn segment_name_length_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"below_mean_count": 0, "total_segments": 0, "mean_name_length": null}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| (name.len() as f64) < mean).count();
    Json(serde_json::json!({"below_mean_count": count, "total_segments": n, "mean_name_length": mean}))
}

/// GET /api/v1/search/index/segments/name-length-below-p50 — segmentos com nome mais curto que a mediana. Sprint #4158.
pub async fn segment_name_length_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"below_p50_count": 0, "total_segments": 0, "p50_name_length": null}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let count = vals.iter().filter(|&&v| (v as f64) < p50).count();
    Json(serde_json::json!({"below_p50_count": count, "total_segments": n, "p50_name_length": p50}))
}

/// GET /api/v1/search/index/segments/name-length-normalized — comprimentos de nome normalizados (min-max). Sprint #4159.
pub async fn segment_name_length_normalized(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"normalized": [], "total_segments": 0}));
    }
    let lens: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    let min = lens.iter().copied().min().unwrap_or(0);
    let max = lens.iter().copied().max().unwrap_or(0);
    let range = (max - min) as f64;
    if range == 0.0 {
        return Json(serde_json::json!({"normalized": lens.iter().map(|_| 0.0).collect::<Vec<f64>>(), "min": min, "max": max, "total_segments": n}));
    }
    let normalized: Vec<f64> = lens.iter().map(|&v| (v - min) as f64 / range).collect();
    Json(serde_json::json!({"normalized": normalized, "min_name_length": min, "max_name_length": max, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-rank-max — segmento com nome mais longo (rank 1). Sprint #4160.
pub async fn segment_name_length_rank_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"rank_max_name": null, "rank_max_length": null, "total_segments": 0}));
    }
    let max_entry = segs.iter().max_by_key(|(name, _, _)| name.len());
    match max_entry {
        Some((name, docs, bytes)) => Json(serde_json::json!({"rank_max_name": name, "rank_max_length": name.len(), "docs": docs, "bytes": bytes, "total_segments": n})),
        None => Json(serde_json::json!({"rank_max_name": null, "total_segments": n})),
    }
}

/// GET /api/v1/search/index/segments/name-length-max-among-below-mean — maior comprimento de nome entre segmentos abaixo da média. Sprint #4277.
pub async fn segment_name_length_max_among_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_among_below_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let max = segs.iter().filter_map(|(name, _, _)| if (name.len() as f64) < mean { Some(name.len()) } else { None }).max();
    Json(serde_json::json!({"max_among_below_mean": max, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-max-among-above-mean — maior comprimento de nome entre segmentos acima da média. Sprint #4278.
pub async fn segment_name_length_max_among_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_among_above_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let max = segs.iter().filter_map(|(name, _, _)| if name.len() as f64 > mean { Some(name.len()) } else { None }).max();
    Json(serde_json::json!({"max_among_above_mean": max, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-p50 — soma dos comprimentos de nome acima da mediana. Sprint #4279.
pub async fn segment_name_length_sum_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_above_p50": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let sum: usize = vals.iter().filter(|&&v| v as f64 > p50).sum();
    Json(serde_json::json!({"sum_above_p50": sum, "p50_name_length": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-below-p50 — soma dos comprimentos de nome abaixo da mediana. Sprint #4280.
pub async fn segment_name_length_sum_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_below_p50": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let sum: usize = vals.iter().filter(|&&v| (v as f64) < p50).sum();
    Json(serde_json::json!({"sum_below_p50": sum, "p50_name_length": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-below-mean — soma dos comprimentos de nome abaixo da média. Sprint #4257.
pub async fn segment_name_length_sum_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_below_mean": 0, "total_segments": 0, "mean_name_length": null}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let sum: usize = segs.iter().filter_map(|(name, _, _)| if (name.len() as f64) < mean { Some(name.len()) } else { None }).sum();
    Json(serde_json::json!({"sum_below_mean": sum, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-p75 — número de segmentos com nome abaixo do P75. Sprint #4258.
pub async fn segment_name_length_count_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p75": 0, "total_segments": 0, "p75_name_length": null}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[p75_idx];
    let count = vals.iter().filter(|&&v| v < p75).count();
    Json(serde_json::json!({"count_below_p75": count, "total_segments": n, "p75_name_length": p75}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-p75 — número de segmentos com nome acima do P75. Sprint #4259.
pub async fn segment_name_length_count_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p75": 0, "total_segments": 0, "p75_name_length": null}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[p75_idx];
    let count = vals.iter().filter(|&&v| v > p75).count();
    Json(serde_json::json!({"count_above_p75": count, "total_segments": n, "p75_name_length": p75}))
}

/// GET /api/v1/search/index/segments/name-length-min-among-above-mean — menor comprimento de nome entre segmentos acima da média. Sprint #4260.
pub async fn segment_name_length_min_among_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_among_above_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let min = segs.iter().filter_map(|(name, _, _)| if name.len() as f64 > mean { Some(name.len()) } else { None }).min();
    Json(serde_json::json!({"min_among_above_mean": min, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-ratio-below-p50 — fração de segmentos com nome abaixo da mediana. Sprint #4237.
pub async fn segment_name_length_ratio_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let count = vals.iter().filter(|&&v| (v as f64) < p50).count();
    Json(serde_json::json!({"ratio_below_p50": count as f64 / n as f64, "below_p50_count": count, "p50_name_length": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-ratio-below-p75 — fração de segmentos com nome abaixo do P75. Sprint #4238.
pub async fn segment_name_length_ratio_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[p75_idx] as f64;
    let count = vals.iter().filter(|&&v| (v as f64) < p75).count();
    Json(serde_json::json!({"ratio_below_p75": count as f64 / n as f64, "below_p75_count": count, "p75_name_length": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-mean — soma dos comprimentos de nome acima da média. Sprint #4239.
pub async fn segment_name_length_sum_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_above_mean": 0, "total_segments": 0, "mean_name_length": null}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let sum: usize = segs.iter().filter_map(|(name, _, _)| if name.len() as f64 > mean { Some(name.len()) } else { None }).sum();
    Json(serde_json::json!({"sum_above_mean": sum, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-mode — moda de comprimento de nome de segmento. Sprint #4240.
pub async fn segment_name_length_mode(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_mode": null, "mode_count": 0, "total_segments": 0}));
    }
    let mut freq: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (name, _, _) in &segs {
        *freq.entry(name.len()).or_insert(0) += 1;
    }
    let (mode, count) = freq.into_iter().max_by_key(|&(_, c)| c).unwrap_or((0, 0));
    Json(serde_json::json!({"name_length_mode": mode, "mode_count": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-above-p75 — segmentos com nome mais longo que o P75. Sprint #4217.
pub async fn segment_name_length_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"above_p75_count": 0, "total_segments": 0, "p75_name_length": null}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[p75_idx] as f64;
    let count = vals.iter().filter(|&&v| v as f64 > p75).count();
    Json(serde_json::json!({"above_p75_count": count, "total_segments": n, "p75_name_length": p75}))
}

/// GET /api/v1/search/index/segments/name-length-ratio-above-mean — fração de segmentos com nome acima da média. Sprint #4218.
pub async fn segment_name_length_ratio_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| name.len() as f64 > mean).count();
    Json(serde_json::json!({"ratio_above_mean": count as f64 / n as f64, "above_mean_count": count, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-ratio-above-p50 — fração de segmentos com nome acima da mediana. Sprint #4219.
pub async fn segment_name_length_ratio_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let count = vals.iter().filter(|&&v| v as f64 > p50).count();
    Json(serde_json::json!({"ratio_above_p50": count as f64 / n as f64, "above_p50_count": count, "p50_name_length": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-ratio-below-mean — fração de segmentos com nome abaixo da média. Sprint #4220.
pub async fn segment_name_length_ratio_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| (name.len() as f64) < mean).count();
    Json(serde_json::json!({"ratio_below_mean": count as f64 / n as f64, "below_mean_count": count, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-harmonic-mean — média harmônica de comprimento de nome de segmento. Sprint #4197.
pub async fn segment_name_length_harmonic_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_harmonic_mean": null, "total_segments": 0}));
    }
    let inv_sum: f64 = segs.iter().map(|(name, _, _)| if name.len() > 0 { 1.0 / name.len() as f64 } else { 0.0 }).sum();
    let hm = if inv_sum > 0.0 { Some(n as f64 / inv_sum) } else { None };
    Json(serde_json::json!({"name_length_harmonic_mean": hm, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-geometric-mean — média geométrica de comprimento de nome de segmento. Sprint #4198.
pub async fn segment_name_length_geometric_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_geometric_mean": null, "total_segments": 0}));
    }
    let ln_sum: f64 = segs.iter().filter_map(|(name, _, _)| if name.len() > 0 { Some((name.len() as f64).ln()) } else { None }).sum();
    let valid_n = segs.iter().filter(|(name, _, _)| name.len() > 0).count();
    let gm = if valid_n > 0 { Some((ln_sum / valid_n as f64).exp()) } else { None };
    Json(serde_json::json!({"name_length_geometric_mean": gm, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-median — mediana de comprimento de nome de segmento. Sprint #4199.
pub async fn segment_name_length_median(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_median": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let median = if n % 2 == 0 {
        (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0
    } else {
        vals[n / 2] as f64
    };
    Json(serde_json::json!({"name_length_median": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-below-p75 — segmentos com nome mais curto que o P75. Sprint #4200.
pub async fn segment_name_length_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"below_p75_count": 0, "total_segments": 0, "p75_name_length": null}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75 = vals[p75_idx] as f64;
    let count = vals.iter().filter(|&&v| (v as f64) < p75).count();
    Json(serde_json::json!({"below_p75_count": count, "total_segments": n, "p75_name_length": p75}))
}

/// GET /api/v1/search/index/segments/name-length-rank-min — segmento com nome mais curto (rank 1). Sprint #4177.
pub async fn segment_name_length_rank_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"rank_min_name": null, "rank_min_length": null, "total_segments": 0}));
    }
    let min_entry = segs.iter().min_by_key(|(name, _, _)| name.len());
    match min_entry {
        Some((name, docs, bytes)) => Json(serde_json::json!({"rank_min_name": name, "rank_min_length": name.len(), "docs": docs, "bytes": bytes, "total_segments": n})),
        None => Json(serde_json::json!({"rank_min_name": null, "total_segments": n})),
    }
}

/// GET /api/v1/search/index/segments/name-length-iqr — IQR de comprimento de nome de segmento. Sprint #4178.
pub async fn segment_name_length_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_iqr": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).saturating_sub(1).min(n - 1);
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    let iqr = vals[p75_idx] as f64 - vals[p25_idx] as f64;
    Json(serde_json::json!({"name_length_iqr": iqr, "p25_name_length": vals[p25_idx], "p75_name_length": vals[p75_idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-trimmed-mean — média trimmed (10–90%) de comprimento de nome. Sprint #4179.
pub async fn segment_name_length_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_trimmed_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let lo = (n as f64 * 0.10).ceil() as usize;
    let hi = (n as f64 * 0.90).floor() as usize;
    let trimmed = if lo < hi { &vals[lo..hi] } else { &vals[..] };
    let mean = trimmed.iter().sum::<usize>() as f64 / trimmed.len() as f64;
    Json(serde_json::json!({"name_length_trimmed_mean": mean, "trimmed_count": trimmed.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-winsorized-mean — média winsorized (10–90%) de comprimento de nome. Sprint #4180.
pub async fn segment_name_length_winsorized_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_winsorized_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let lo = (n as f64 * 0.10).ceil() as usize;
    let hi_idx = ((n as f64 * 0.90).floor() as usize).min(n - 1);
    let lo_val = vals[lo.min(n - 1)];
    let hi_val = vals[hi_idx];
    let winsorized: Vec<usize> = vals.iter().map(|&v| v.clamp(lo_val, hi_val)).collect();
    let mean = winsorized.iter().sum::<usize>() as f64 / n as f64;
    Json(serde_json::json!({"name_length_winsorized_mean": mean, "clamp_lo": lo_val, "clamp_hi": hi_val, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p25 — P25 de comprimento de nome de segmento. Sprint #4057.
pub async fn segment_name_length_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_p25": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.25).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"name_length_p25": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p50 — P50 de comprimento de nome de segmento. Sprint #4058.
pub async fn segment_name_length_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_p50": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 {
        (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0
    } else {
        vals[n / 2] as f64
    };
    Json(serde_json::json!({"name_length_p50": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p75 — P75 de comprimento de nome de segmento. Sprint #4059.
pub async fn segment_name_length_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_p75": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.75).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"name_length_p75": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p90 — P90 de comprimento de nome de segmento. Sprint #4060.
pub async fn segment_name_length_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_p90": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.90).ceil() as usize).saturating_sub(1).min(n - 1);
    Json(serde_json::json!({"name_length_p90": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-trimmed-mean — média aparada P10–P90 de docs. Sprint #4017.
pub async fn segment_docs_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"docs_trimmed_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let lo = ((n as f64 * 0.10).ceil() as usize).min(n);
    let hi = ((n as f64 * 0.90).ceil() as usize).min(n);
    let trimmed = &vals[lo..hi];
    let mean = if trimmed.is_empty() { None } else {
        Some(trimmed.iter().sum::<u64>() as f64 / trimmed.len() as f64)
    };
    Json(serde_json::json!({"docs_trimmed_mean": mean, "trimmed_count": trimmed.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-trimmed-mean — média aparada P10–P90 de bytes. Sprint #4018.
pub async fn segment_bytes_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_trimmed_mean": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let lo = ((n as f64 * 0.10).ceil() as usize).min(n);
    let hi = ((n as f64 * 0.90).ceil() as usize).min(n);
    let trimmed = &vals[lo..hi];
    let mean = if trimmed.is_empty() { None } else {
        Some(trimmed.iter().sum::<u64>() as f64 / trimmed.len() as f64)
    };
    Json(serde_json::json!({"bytes_trimmed_mean": mean, "trimmed_count": trimmed.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-range — range (max-min) de comprimento de nome. Sprint #4019.
pub async fn segment_name_length_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_range": null, "total_segments": 0}));
    }
    let lens: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    let min = lens.iter().copied().min().unwrap_or(0);
    let max = lens.iter().copied().max().unwrap_or(0);
    Json(serde_json::json!({"name_length_range": max - min, "name_length_min": min, "name_length_max": max, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-variance — variância de comprimento de nome. Sprint #4020.
pub async fn segment_name_length_variance(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"name_length_variance": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"name_length_variance": variance, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-p25 — número de segmentos com nome acima do P25. Sprint #4397.
pub async fn segment_name_length_count_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx] as f64;
    let count = vals.iter().filter(|&&v| (v as f64) > p25).count();
    Json(serde_json::json!({"count_above_p25": count, "p25_name_length": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-below-p25 — soma dos comprimentos de nome abaixo do P25. Sprint #4398.
pub async fn segment_name_length_sum_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_below_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx] as f64;
    let sum: usize = vals.iter().filter(|&&v| (v as f64) < p25).sum();
    Json(serde_json::json!({"sum_below_p25": sum, "p25_name_length": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-mean-ratio — fração da soma total dos comprimentos que está acima da média. Sprint #4399.
pub async fn segment_name_length_sum_above_mean_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_sum_above_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let total_sum: usize = segs.iter().map(|(name, _, _)| name.len()).sum();
    let above_sum: usize = segs.iter().filter_map(|(name, _, _)| if (name.len() as f64) > mean { Some(name.len()) } else { None }).sum();
    let ratio = if total_sum > 0 { Some(above_sum as f64 / total_sum as f64) } else { None };
    Json(serde_json::json!({"ratio_sum_above_mean": ratio, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-range — range (max - min) de comprimento de nome. Sprint #4400.
pub async fn segment_name_length_range_by_name(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_range": null, "total_segments": 0}));
    }
    let min = segs.iter().map(|(name, _, _)| name.len()).min().unwrap_or(0);
    let max = segs.iter().map(|(name, _, _)| name.len()).max().unwrap_or(0);
    Json(serde_json::json!({"name_length_range": max - min, "min_name_length": min, "max_name_length": max, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-entropy — entropia de Shannon dos nomes dos segmentos (por caractere). Sprint #4377.
pub async fn segment_name_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_entropy": null, "total_segments": 0}));
    }
    let mut freq: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    let mut total_chars = 0usize;
    for (name, _, _) in &segs {
        for c in name.chars() { *freq.entry(c).or_insert(0) += 1; total_chars += 1; }
    }
    let entropy = if total_chars > 0 {
        freq.values().map(|&cnt| {
            let p = cnt as f64 / total_chars as f64;
            -p * p.log2()
        }).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"name_entropy": entropy, "total_chars": total_chars, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-max-freq — comprimento de nome mais frequente. Sprint #4378.
pub async fn segment_name_length_max_freq(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_freq_name_length": null, "frequency": 0, "total_segments": 0}));
    }
    let mut freq: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (name, _, _) in &segs { *freq.entry(name.len()).or_insert(0) += 1; }
    let (length, count) = freq.into_iter().max_by_key(|&(_, c)| c).unwrap_or((0, 0));
    Json(serde_json::json!({"max_freq_name_length": length, "frequency": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-kurtosis — kurtosis de comprimento de nome. Sprint #4379.
pub async fn segment_name_length_kurtosis_name(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"name_length_kurtosis": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let kurtosis = if variance > 0.0 {
        let stddev = variance.sqrt();
        vals.iter().map(|&v| ((v - mean) / stddev).powi(4)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"name_length_kurtosis": kurtosis, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-p25 — soma dos comprimentos de nome acima do P25. Sprint #4380.
pub async fn segment_name_length_sum_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_above_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx] as f64;
    let sum: usize = vals.iter().filter(|&&v| (v as f64) > p25).sum();
    Json(serde_json::json!({"sum_above_p25": sum, "p25_name_length": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-p25 — número de segmentos com nome abaixo do P25. Sprint #4357.
pub async fn segment_name_length_count_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx] as f64;
    let count = vals.iter().filter(|&&v| (v as f64) < p25).count();
    Json(serde_json::json!({"count_below_p25": count, "p25_name_length": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-skewness — skewness de comprimento de nome. Sprint #4358.
pub async fn segment_name_length_skewness_name(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"name_length_skewness": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let skewness = if stddev > 0.0 {
        vals.iter().map(|&v| ((v - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"name_length_skewness": skewness, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-distinct-count — número de comprimentos de nome distintos. Sprint #4359.
pub async fn segment_name_length_distinct_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let distinct: std::collections::HashSet<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    Json(serde_json::json!({"distinct_name_length_count": distinct.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-below-mean-ratio — fração da soma total dos comprimentos que está abaixo da média. Sprint #4360.
pub async fn segment_name_length_sum_below_mean_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_sum_below_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let total_sum: usize = segs.iter().map(|(name, _, _)| name.len()).sum();
    let below_sum: usize = segs.iter().filter_map(|(name, _, _)| if (name.len() as f64) < mean { Some(name.len()) } else { None }).sum();
    let ratio = if total_sum > 0 { Some(below_sum as f64 / total_sum as f64) } else { None };
    Json(serde_json::json!({"ratio_sum_below_mean": ratio, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-mean-ratio — fração de segmentos com nome acima da média. Sprint #4337.
pub async fn segment_name_length_count_above_mean_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| (name.len() as f64) > mean).count();
    Json(serde_json::json!({"ratio_above_mean": count as f64 / n as f64, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-min-among-below-mean — mínimo de comprimento de nome entre segmentos abaixo da média. Sprint #4338.
pub async fn segment_name_length_min_among_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_among_below_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let min = segs.iter().filter_map(|(name, _, _)| if (name.len() as f64) < mean { Some(name.len()) } else { None }).min();
    Json(serde_json::json!({"min_among_below_mean": min, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p10 — P10 de comprimento de nome dos segmentos. Sprint #4339.
pub async fn segment_name_length_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_name_length": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    Json(serde_json::json!({"p10_name_length": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-mean-ratio — fração de segmentos com nome abaixo da média. Sprint #4340.
pub async fn segment_name_length_count_below_mean_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_mean": null, "total_segments": 0}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| (name.len() as f64) < mean).count();
    Json(serde_json::json!({"ratio_below_mean": count as f64 / n as f64, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-p50 — número de segmentos com nome abaixo da mediana. Sprint #4317.
pub async fn segment_name_length_count_below_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p50": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let count = vals.iter().filter(|&&v| (v as f64) < p50).count();
    Json(serde_json::json!({"count_below_p50": count, "p50_name_length": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-stddev — desvio padrão de comprimento de nome. Sprint #4318.
pub async fn segment_name_length_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"name_length_stddev": null, "total_segments": 0}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    Json(serde_json::json!({"name_length_stddev": stddev, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p01 — P01 de comprimento de nome dos segmentos. Sprint #4319.
pub async fn segment_name_length_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p01_name_length": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    Json(serde_json::json!({"p01_name_length": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-p75 — soma dos comprimentos de nome acima do P75. Sprint #4320.
pub async fn segment_name_length_sum_above_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_above_p75": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).min(n - 1);
    let p75 = vals[p75_idx] as f64;
    let sum: usize = vals.iter().filter(|&&v| (v as f64) > p75).sum();
    Json(serde_json::json!({"sum_above_p75": sum, "p75_name_length": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-mean — número de segmentos com nome acima da média. Sprint #4297.
pub async fn segment_name_length_count_above_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_mean": 0, "total_segments": 0, "mean_name_length": null}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| (name.len() as f64) > mean).count();
    Json(serde_json::json!({"count_above_mean": count, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p75 — soma de docs dos segmentos abaixo do P75. Sprint #4637.
pub async fn segment_docs_sum_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p75": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).min(n - 1);
    let p75 = vals[p75_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p75).sum();
    let count = vals.iter().filter(|&&v| v < p75).count();
    Json(serde_json::json!({"sum_docs_below_p75": sum, "p75_docs": p75, "count_below_p75": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p25 — soma de docs dos segmentos abaixo do P25. Sprint #4638.
pub async fn segment_docs_sum_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p25).sum();
    let count = vals.iter().filter(|&&v| v < p25).count();
    Json(serde_json::json!({"sum_docs_below_p25": sum, "p25_docs": p25, "count_below_p25": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p10 — soma de docs dos segmentos abaixo do P10. Sprint #4639.
pub async fn segment_docs_sum_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p10).sum();
    let count = vals.iter().filter(|&&v| v < p10).count();
    Json(serde_json::json!({"sum_docs_below_p10": sum, "p10_docs": p10, "count_below_p10": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p05 — soma de docs dos segmentos abaixo do P05. Sprint #4640.
pub async fn segment_docs_sum_below_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p05).sum();
    let count = vals.iter().filter(|&&v| v < p05).count();
    Json(serde_json::json!({"sum_docs_below_p05": sum, "p05_docs": p05, "count_below_p05": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p01 — soma de docs dos segmentos abaixo do P01. Sprint #4657.
pub async fn segment_docs_sum_below_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p01).sum();
    let count = vals.iter().filter(|&&v| v < p01).count();
    Json(serde_json::json!({"sum_docs_below_p01": sum, "p01_docs": p01, "count_below_p01": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p90 — soma de docs dos segmentos abaixo do P90. Sprint #4658.
pub async fn segment_docs_sum_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p90": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p90_idx = ((n as f64 * 0.90).ceil() as usize).min(n - 1);
    let p90 = vals[p90_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p90).sum();
    let count = vals.iter().filter(|&&v| v < p90).count();
    Json(serde_json::json!({"sum_docs_below_p90": sum, "p90_docs": p90, "count_below_p90": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p95 — soma de docs dos segmentos abaixo do P95. Sprint #4659.
pub async fn segment_docs_sum_below_p95(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p95": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p95_idx = ((n as f64 * 0.95).ceil() as usize).min(n - 1);
    let p95 = vals[p95_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p95).sum();
    let count = vals.iter().filter(|&&v| v < p95).count();
    Json(serde_json::json!({"sum_docs_below_p95": sum, "p95_docs": p95, "count_below_p95": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-below-p99 — soma de docs dos segmentos abaixo do P99. Sprint #4660.
pub async fn segment_docs_sum_below_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_below_p99": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p99_idx = ((n as f64 * 0.99).ceil() as usize).min(n - 1);
    let p99 = vals[p99_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p99).sum();
    let count = vals.iter().filter(|&&v| v < p99).count();
    Json(serde_json::json!({"sum_docs_below_p99": sum, "p99_docs": p99, "count_below_p99": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p99 — soma de docs dos segmentos acima do P99. Sprint #4677.
pub async fn segment_docs_sum_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_above_p99": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p99_idx = ((n as f64 * 0.99).ceil() as usize).min(n - 1);
    let p99 = vals[p99_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p99).sum();
    let count = vals.iter().filter(|&&v| v > p99).count();
    Json(serde_json::json!({"sum_docs_above_p99": sum, "p99_docs": p99, "count_above_p99": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-p99 — soma de bytes dos segmentos acima do P99. Sprint #4678.
pub async fn segment_bytes_sum_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_above_p99": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p99_idx = ((n as f64 * 0.99).ceil() as usize).min(n - 1);
    let p99 = vals[p99_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p99).sum();
    let count = vals.iter().filter(|&&v| v > p99).count();
    Json(serde_json::json!({"sum_bytes_above_p99": sum, "p99_bytes": p99, "count_above_p99": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-below-p10 — soma de comprimento de nome dos segmentos abaixo do P10. Sprint #4679.
pub async fn segment_name_length_sum_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_name_length_below_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(name, _, _)| name.len() as u64).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p10).sum();
    let count = vals.iter().filter(|&&v| v < p10).count();
    Json(serde_json::json!({"sum_name_length_below_p10": sum, "p10_name_length": p10, "count_below_p10": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-below-p05 — soma de comprimento de nome dos segmentos abaixo do P05. Sprint #4680.
pub async fn segment_name_length_sum_below_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_name_length_below_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(name, _, _)| name.len() as u64).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p05).sum();
    let count = vals.iter().filter(|&&v| v < p05).count();
    Json(serde_json::json!({"sum_name_length_below_p05": sum, "p05_name_length": p05, "count_below_p05": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p25 — soma de docs dos segmentos acima do P25. Sprint #4617.
/// GET /api/v1/search/index/segments/name-length-sum-below-p01 — soma de comprimento de nome dos segmentos abaixo do P01. Sprint #4697.
pub async fn segment_name_length_sum_below_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_name_length_below_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(name, _, _)| name.len() as u64).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p01).sum();
    let count = vals.iter().filter(|&&v| v < p01).count();
    Json(serde_json::json!({"sum_name_length_below_p01": sum, "p01_name_length": p01, "count_below_p01": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-p90 — soma de comprimento de nome dos segmentos acima do P90. Sprint #4698.
pub async fn segment_name_length_sum_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_name_length_above_p90": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(name, _, _)| name.len() as u64).collect();
    vals.sort_unstable();
    let p90_idx = ((n as f64 * 0.90).ceil() as usize).min(n - 1);
    let p90 = vals[p90_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p90).sum();
    let count = vals.iter().filter(|&&v| v > p90).count();
    Json(serde_json::json!({"sum_name_length_above_p90": sum, "p90_name_length": p90, "count_above_p90": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-p99 — soma de comprimento de nome dos segmentos acima do P99. Sprint #4699.
pub async fn segment_name_length_sum_above_p99(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_name_length_above_p99": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(name, _, _)| name.len() as u64).collect();
    vals.sort_unstable();
    let p99_idx = ((n as f64 * 0.99).ceil() as usize).min(n - 1);
    let p99 = vals[p99_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p99).sum();
    let count = vals.iter().filter(|&&v| v > p99).count();
    Json(serde_json::json!({"sum_name_length_above_p99": sum, "p99_name_length": p99, "count_above_p99": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-above-p25 — soma de comprimento de nome dos segmentos acima do P25. Sprint #4700.
pub async fn segment_name_length_sum_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_name_length_above_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(name, _, _)| name.len() as u64).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p25).sum();
    let count = vals.iter().filter(|&&v| v > p25).count();
    Json(serde_json::json!({"sum_name_length_above_p25": sum, "p25_name_length": p25, "count_above_p25": count, "total_segments": n}))
}

pub async fn segment_docs_sum_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_above_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p25).sum();
    let count = vals.iter().filter(|&&v| v > p25).count();
    Json(serde_json::json!({"sum_docs_above_p25": sum, "p25_docs": p25, "count_above_p25": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p05 — soma de docs dos segmentos acima do P05. Sprint #4618.
pub async fn segment_docs_sum_above_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_above_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p05).sum();
    let count = vals.iter().filter(|&&v| v > p05).count();
    Json(serde_json::json!({"sum_docs_above_p05": sum, "p05_docs": p05, "count_above_p05": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p01 — soma de docs dos segmentos acima do P01. Sprint #4619.
pub async fn segment_docs_sum_above_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_above_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p01).sum();
    let count = vals.iter().filter(|&&v| v > p01).count();
    Json(serde_json::json!({"sum_docs_above_p01": sum, "p01_docs": p01, "count_above_p01": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-below-p01 — soma de bytes dos segmentos abaixo do P01. Sprint #4620.
pub async fn segment_bytes_sum_below_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_below_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p01).sum();
    let count = vals.iter().filter(|&&v| v < p01).count();
    Json(serde_json::json!({"sum_bytes_below_p01": sum, "p01_bytes": p01, "count_below_p01": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-below-p10 — soma de bytes dos segmentos abaixo do P10. Sprint #4597.
pub async fn segment_bytes_sum_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_below_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p10).sum();
    let count = vals.iter().filter(|&&v| v < p10).count();
    Json(serde_json::json!({"sum_bytes_below_p10": sum, "p10_bytes": p10, "count_below_p10": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-below-p05 — soma de bytes dos segmentos abaixo do P05. Sprint #4598.
pub async fn segment_bytes_sum_below_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_below_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p05).sum();
    let count = vals.iter().filter(|&&v| v < p05).count();
    Json(serde_json::json!({"sum_bytes_below_p05": sum, "p05_bytes": p05, "count_below_p05": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-p01 — soma de bytes dos segmentos acima do P01. Sprint #4599.
pub async fn segment_bytes_sum_above_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_above_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p01).sum();
    let count = vals.iter().filter(|&&v| v > p01).count();
    Json(serde_json::json!({"sum_bytes_above_p01": sum, "p01_bytes": p01, "count_above_p01": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-sum-above-p10 — soma de docs dos segmentos acima do P10. Sprint #4600.
pub async fn segment_docs_sum_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_docs_above_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p10).sum();
    let count = vals.iter().filter(|&&v| v > p10).count();
    Json(serde_json::json!({"sum_docs_above_p10": sum, "p10_docs": p10, "count_above_p10": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-p90 — número de segmentos com nome abaixo do P90. Sprint #4577.
pub async fn segment_name_length_count_below_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p90": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p90_idx = ((n as f64 * 0.90).ceil() as usize).min(n - 1);
    let p90 = vals[p90_idx];
    let count = vals.iter().filter(|&&v| v < p90).count();
    Json(serde_json::json!({"count_below_p90": count, "p90_name_length": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-p25 — soma de bytes dos segmentos acima do P25. Sprint #4578.
pub async fn segment_bytes_sum_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_above_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p25).sum();
    let count = vals.iter().filter(|&&v| v > p25).count();
    Json(serde_json::json!({"sum_bytes_above_p25": sum, "p25_bytes": p25, "count_above_p25": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-above-p10 — soma de bytes dos segmentos acima do P10. Sprint #4579.
pub async fn segment_bytes_sum_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_above_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let sum: u64 = vals.iter().filter(|&&v| v > p10).sum();
    let count = vals.iter().filter(|&&v| v > p10).count();
    Json(serde_json::json!({"sum_bytes_above_p10": sum, "p10_bytes": p10, "count_above_p10": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-sum-below-p25 — soma de bytes dos segmentos abaixo do P25. Sprint #4580.
pub async fn segment_bytes_sum_below_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_bytes_below_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx];
    let sum: u64 = vals.iter().filter(|&&v| v < p25).sum();
    let count = vals.iter().filter(|&&v| v < p25).count();
    Json(serde_json::json!({"sum_bytes_below_p25": sum, "p25_bytes": p25, "count_below_p25": count, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-p90 — número de segmentos com nome acima do P90. Sprint #4557.
pub async fn segment_name_length_count_above_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p90": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p90_idx = ((n as f64 * 0.90).ceil() as usize).min(n - 1);
    let p90 = vals[p90_idx];
    let count = vals.iter().filter(|&&v| v > p90).count();
    Json(serde_json::json!({"count_above_p90": count, "p90_name_length": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-p10 — número de segmentos com nome abaixo do P10. Sprint #4558.
pub async fn segment_name_length_count_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let count = vals.iter().filter(|&&v| v < p10).count();
    Json(serde_json::json!({"count_below_p10": count, "p10_name_length": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-p05 — número de segmentos com nome abaixo do P05. Sprint #4559.
pub async fn segment_name_length_count_below_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let count = vals.iter().filter(|&&v| v < p05).count();
    Json(serde_json::json!({"count_below_p05": count, "p05_name_length": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-p01 — número de segmentos com nome abaixo do P01. Sprint #4560.
pub async fn segment_name_length_count_below_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let count = vals.iter().filter(|&&v| v < p01).count();
    Json(serde_json::json!({"count_below_p01": count, "p01_name_length": p01, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-below-mean — número de segmentos com nome abaixo da média. Sprint #4298.
pub async fn segment_name_length_count_below_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_mean": 0, "total_segments": 0, "mean_name_length": null}));
    }
    let mean = segs.iter().map(|(name, _, _)| name.len()).sum::<usize>() as f64 / n as f64;
    let count = segs.iter().filter(|(name, _, _)| (name.len() as f64) < mean).count();
    Json(serde_json::json!({"count_below_mean": count, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-sum-below-p75 — soma dos comprimentos de nome abaixo do P75. Sprint #4299.
pub async fn segment_name_length_sum_below_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_below_p75": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p75_idx = ((n as f64 * 0.75).ceil() as usize).min(n - 1);
    let p75 = vals[p75_idx] as f64;
    let sum: usize = vals.iter().filter(|&&v| (v as f64) < p75).sum();
    Json(serde_json::json!({"sum_below_p75": sum, "p75_name_length": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-p50 — número de segmentos com nome acima da mediana. Sprint #4300.
pub async fn segment_name_length_count_above_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p50": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let p50 = if n % 2 == 0 { (vals[n / 2 - 1] + vals[n / 2]) as f64 / 2.0 } else { vals[n / 2] as f64 };
    let count = vals.iter().filter(|&&v| (v as f64) > p50).count();
    Json(serde_json::json!({"count_above_p50": count, "p50_name_length": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-p05 — P05 de comprimento de nome dos segmentos. Sprint #4417.
pub async fn segment_name_length_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p05_name_length": null, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    Json(serde_json::json!({"p05_name_length": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-unique-chars — número de caracteres únicos em todos os nomes de segmentos. Sprint #4418.
pub async fn segment_name_unique_chars(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let distinct: std::collections::HashSet<char> = segs.iter().flat_map(|(name, _, _)| name.chars()).collect();
    Json(serde_json::json!({"unique_chars": distinct.len(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-avg-word-length — comprimento médio de palavra nos nomes dos segmentos. Sprint #4419.
pub async fn segment_name_avg_word_length(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_word_length": null, "total_segments": 0}));
    }
    let words: Vec<usize> = segs.iter()
        .flat_map(|(name, _, _)| name.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).map(|w| w.len()).collect::<Vec<_>>())
        .collect();
    let avg = if words.is_empty() { None } else { Some(words.iter().sum::<usize>() as f64 / words.len() as f64) };
    Json(serde_json::json!({"avg_word_length": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-cv — coeficiente de variação do comprimento de nome dos segmentos. Sprint #4420.
pub async fn segment_name_length_cv(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"name_length_cv": null, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(name, _, _)| name.len() as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    if mean == 0.0 {
        return Json(serde_json::json!({"name_length_cv": null, "total_segments": n}));
    }
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let cv = variance.sqrt() / mean;
    Json(serde_json::json!({"name_length_cv": cv, "mean_name_length": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-p01 — número de segmentos com nome acima do P01. Sprint #4437.
pub async fn segment_name_length_count_above_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[idx] as f64;
    let count = vals.iter().filter(|&&v| (v as f64) > p01).count();
    Json(serde_json::json!({"count_above_p01": count, "p01_name_length": p01, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-p05 — número de segmentos com nome acima do P05. Sprint #4438.
pub async fn segment_name_length_count_above_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[idx] as f64;
    let count = vals.iter().filter(|&&v| (v as f64) > p05).count();
    Json(serde_json::json!({"count_above_p05": count, "p05_name_length": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/name-length-count-above-p10 — número de segmentos com nome acima do P10. Sprint #4439.
pub async fn segment_name_length_count_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<usize> = segs.iter().map(|(name, _, _)| name.len()).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[idx] as f64;
    let count = vals.iter().filter(|&&v| (v as f64) > p10).count();
    Json(serde_json::json!({"count_above_p10": count, "p10_name_length": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-mode — moda de bytes dos segmentos. Sprint #4440.
pub async fn segment_bytes_mode(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mode_bytes": null, "total_segments": 0}));
    }
    let mut freq: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for (_, bytes, _) in &segs { *freq.entry(*bytes).or_insert(0) += 1; }
    let (mode, mode_freq) = freq.into_iter().max_by_key(|&(_, c)| c).unwrap_or((0, 0));
    Json(serde_json::json!({"mode_bytes": mode, "mode_frequency": mode_freq, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-p01 — P01 de bytes dos segmentos. Sprint #4457.
pub async fn segment_bytes_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p01_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    Json(serde_json::json!({"p01_bytes": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p25 — número de segmentos com bytes acima do P25. Sprint #4458.
pub async fn segment_bytes_count_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx];
    let count = vals.iter().filter(|&&v| v > p25).count();
    Json(serde_json::json!({"count_above_p25": count, "p25_bytes": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p01 — número de segmentos com bytes acima do P01. Sprint #4459.
pub async fn segment_bytes_count_above_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let count = vals.iter().filter(|&&v| v > p01).count();
    Json(serde_json::json!({"count_above_p01": count, "p01_bytes": p01, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p05 — número de segmentos com bytes acima do P05. Sprint #4460.
pub async fn segment_bytes_count_above_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let count = vals.iter().filter(|&&v| v > p05).count();
    Json(serde_json::json!({"count_above_p05": count, "p05_bytes": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p10 — número de segmentos com bytes acima do P10. Sprint #4477.
pub async fn segment_bytes_count_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let count = vals.iter().filter(|&&v| v > p10).count();
    Json(serde_json::json!({"count_above_p10": count, "p10_bytes": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p10 — número de segmentos com bytes abaixo do P10. Sprint #4478.
pub async fn segment_bytes_count_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let count = vals.iter().filter(|&&v| v < p10).count();
    Json(serde_json::json!({"count_below_p10": count, "p10_bytes": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p05 — número de segmentos com bytes abaixo do P05. Sprint #4479.
pub async fn segment_bytes_count_below_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let count = vals.iter().filter(|&&v| v < p05).count();
    Json(serde_json::json!({"count_below_p05": count, "p05_bytes": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-below-p01 — número de segmentos com bytes abaixo do P01. Sprint #4537.
pub async fn segment_bytes_count_below_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let count = vals.iter().filter(|&&v| v < p01).count();
    Json(serde_json::json!({"count_below_p01": count, "p01_bytes": p01, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p01 — número de segmentos com bytes acima do P01. Sprint #4538.
pub async fn segment_bytes_count_above_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let count = vals.iter().filter(|&&v| v > p01).count();
    Json(serde_json::json!({"count_above_p01": count, "p01_bytes": p01, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p05 — número de segmentos com bytes acima do P05. Sprint #4539.
pub async fn segment_bytes_count_above_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let count = vals.iter().filter(|&&v| v > p05).count();
    Json(serde_json::json!({"count_above_p05": count, "p05_bytes": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-count-above-p10 — número de segmentos com bytes acima do P10. Sprint #4540.
pub async fn segment_bytes_count_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, bytes, _)| *bytes).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let count = vals.iter().filter(|&&v| v > p10).count();
    Json(serde_json::json!({"count_above_p10": count, "p10_bytes": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-mode — moda de docs dos segmentos. Sprint #4480.
pub async fn segment_docs_mode(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mode_docs": null, "total_segments": 0}));
    }
    let mut freq: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for (_, _, docs) in &segs { *freq.entry(*docs).or_insert(0) += 1; }
    let (mode, mode_freq) = freq.into_iter().max_by_key(|&(_, c)| c).unwrap_or((0, 0));
    Json(serde_json::json!({"mode_docs": mode, "mode_frequency": mode_freq, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p25 — número de segmentos com docs acima do P25. Sprint #4497.
pub async fn segment_docs_count_above_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p25": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p25_idx = ((n as f64 * 0.25).ceil() as usize).min(n - 1);
    let p25 = vals[p25_idx];
    let count = vals.iter().filter(|&&v| v > p25).count();
    Json(serde_json::json!({"count_above_p25": count, "p25_docs": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p10 — número de segmentos com docs acima do P10. Sprint #4498.
pub async fn segment_docs_count_above_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let count = vals.iter().filter(|&&v| v > p10).count();
    Json(serde_json::json!({"count_above_p10": count, "p10_docs": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p10 — número de segmentos com docs abaixo do P10. Sprint #4499.
pub async fn segment_docs_count_below_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p10": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p10_idx = ((n as f64 * 0.10).ceil() as usize).min(n - 1);
    let p10 = vals[p10_idx];
    let count = vals.iter().filter(|&&v| v < p10).count();
    Json(serde_json::json!({"count_below_p10": count, "p10_docs": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-p01 — P01 de docs dos segmentos. Sprint #4500.
pub async fn segment_docs_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p01_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    Json(serde_json::json!({"p01_docs": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p05 — número de segmentos com docs abaixo do P05. Sprint #4517.
pub async fn segment_docs_count_below_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let count = vals.iter().filter(|&&v| v < p05).count();
    Json(serde_json::json!({"count_below_p05": count, "p05_docs": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-below-p01 — número de segmentos com docs abaixo do P01. Sprint #4518.
pub async fn segment_docs_count_below_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let count = vals.iter().filter(|&&v| v < p01).count();
    Json(serde_json::json!({"count_below_p01": count, "p01_docs": p01, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p01 — número de segmentos com docs acima do P01. Sprint #4519.
pub async fn segment_docs_count_above_p01(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p01": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p01_idx = ((n as f64 * 0.01).ceil() as usize).min(n - 1);
    let p01 = vals[p01_idx];
    let count = vals.iter().filter(|&&v| v > p01).count();
    Json(serde_json::json!({"count_above_p01": count, "p01_docs": p01, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-count-above-p05 — número de segmentos com docs acima do P05. Sprint #4520.
pub async fn segment_docs_count_above_p05(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_p05": 0, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, docs)| *docs).collect();
    vals.sort_unstable();
    let p05_idx = ((n as f64 * 0.05).ceil() as usize).min(n - 1);
    let p05 = vals[p05_idx];
    let count = vals.iter().filter(|&&v| v > p05).count();
    Json(serde_json::json!({"count_above_p05": count, "p05_docs": p05, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p75-bytes — P75 de bytes entre segmentos. Sprint #2588.
pub async fn segment_p75_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = (n * 3 / 4).min(n - 1);
    Json(serde_json::json!({"p75_bytes": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p75-docs — P75 de docs entre segmentos. Sprint #2593.
pub async fn segment_p75_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = (n * 3 / 4).min(n - 1);
    Json(serde_json::json!({"p75_docs": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p90-bytes — P90 de bytes entre segmentos. Sprint #2598.
pub async fn segment_p90_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let idx = (n * 9 / 10).min(n - 1);
    Json(serde_json::json!({"p90_bytes": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p90-docs — P90 de docs entre segmentos. Sprint #2603.
pub async fn segment_p90_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let idx = (n * 9 / 10).min(n - 1);
    Json(serde_json::json!({"p90_docs": vals[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/range-bytes — range de bytes entre segmentos. Sprint #2568.
pub async fn segment_range_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"range_bytes": 0, "total_segments": 0}));
    }
    let max_b = segs.iter().map(|(_, _, b)| *b).max().unwrap_or(0);
    let min_b = segs.iter().map(|(_, _, b)| *b).min().unwrap_or(0);
    Json(serde_json::json!({"range_bytes": max_b.saturating_sub(min_b), "max_bytes": max_b, "min_bytes": min_b, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/range-docs — range de docs entre segmentos. Sprint #2573.
pub async fn segment_range_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"range_docs": 0, "total_segments": 0}));
    }
    let max_d = segs.iter().map(|(_, d, _)| *d).max().unwrap_or(0);
    let min_d = segs.iter().map(|(_, d, _)| *d).min().unwrap_or(0);
    Json(serde_json::json!({"range_docs": max_d.saturating_sub(min_d), "max_docs": max_d, "min_docs": min_d, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p50-bytes — mediana de bytes entre segmentos. Sprint #2578.
pub async fn segment_p50_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_bytes": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    vals.sort_unstable();
    let p50 = vals[n / 2];
    Json(serde_json::json!({"p50_bytes": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/p50-docs — mediana de docs entre segmentos. Sprint #2583.
pub async fn segment_p50_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_docs": null, "total_segments": 0}));
    }
    let mut vals: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    vals.sort_unstable();
    let p50 = vals[n / 2];
    Json(serde_json::json!({"p50_docs": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/cv-bytes — CV de bytes entre segmentos. Sprint #2548.
pub async fn segment_cv_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"cv_bytes": 0.0, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_bytes": cv, "stddev": stddev, "mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/cv-docs — CV de docs entre segmentos. Sprint #2553.
pub async fn segment_cv_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"cv_docs": 0.0, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_docs": cv, "stddev": stddev, "mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/iqr-bytes — IQR de bytes entre segmentos. Sprint #2558.
pub async fn segment_iqr_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"iqr_bytes": 0.0, "total_segments": n}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = vals[n / 4];
    let q3 = vals[3 * n / 4];
    Json(serde_json::json!({"iqr_bytes": q3 - q1, "q1": q1, "q3": q3, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/iqr-docs — IQR de docs entre segmentos. Sprint #2563.
pub async fn segment_iqr_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"iqr_docs": 0.0, "total_segments": n}));
    }
    let mut vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = vals[n / 4];
    let q3 = vals[3 * n / 4];
    Json(serde_json::json!({"iqr_docs": q3 - q1, "q1": q1, "q3": q3, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/avg-docs — média de docs por segmento. Sprint #2528.
pub async fn segment_avg_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_docs": 0.0, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, d, _)| d).sum();
    Json(serde_json::json!({"avg_docs": total as f64 / n as f64, "total_docs": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/stddev-bytes — stddev de bytes entre segmentos. Sprint #2533.
pub async fn segment_stddev_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"stddev_bytes": 0.0, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"stddev_bytes": variance.sqrt(), "mean_bytes": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/stddev-docs — stddev de docs entre segmentos. Sprint #2538.
pub async fn segment_stddev_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"stddev_docs": 0.0, "total_segments": n}));
    }
    let vals: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = vals.iter().sum::<f64>() / n as f64;
    let variance = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"stddev_docs": variance.sqrt(), "mean_docs": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-empty — número de segmentos sem documentos. Sprint #2543.
pub async fn segment_count_empty(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total = segs.len();
    let empty = segs.iter().filter(|(_, docs, _)| *docs == 0).count();
    Json(serde_json::json!({"empty_segments": empty, "total_segments": total, "pct_empty": if total > 0 { empty as f64 / total as f64 * 100.0 } else { 0.0 }}))
}

/// GET /api/v1/search/index/segments/docs-count-by-size-bucket — contagem de segmentos por bucket de tamanho. Sprint #2508.
pub async fn segment_docs_count_by_size_bucket(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut tiny = 0u64;
    let mut small = 0u64;
    let mut medium = 0u64;
    let mut large = 0u64;
    for (_, _, bytes) in &segs {
        match bytes {
            b if *b < 1_048_576 => tiny += 1,
            b if *b < 10_485_760 => small += 1,
            b if *b < 104_857_600 => medium += 1,
            _ => large += 1,
        }
    }
    Json(serde_json::json!({"buckets": {"tiny_lt1mb": tiny, "small_1mb_10mb": small, "medium_10mb_100mb": medium, "large_ge100mb": large}, "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/size-bucket-distribution — distribuição percentual por bucket de bytes. Sprint #2513.
pub async fn segment_size_bucket_distribution(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len() as f64;
    let (mut tiny, mut small, mut medium, mut large) = (0u64, 0u64, 0u64, 0u64);
    for (_, _, bytes) in &segs {
        match bytes {
            b if *b < 1_048_576 => tiny += 1,
            b if *b < 10_485_760 => small += 1,
            b if *b < 104_857_600 => medium += 1,
            _ => large += 1,
        }
    }
    let pct = |v: u64| if n > 0.0 { v as f64 / n * 100.0 } else { 0.0 };
    Json(serde_json::json!({"distribution_pct": {"tiny_lt1mb": pct(tiny), "small_1mb_10mb": pct(small), "medium_10mb_100mb": pct(medium), "large_ge100mb": pct(large)}, "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/doc-bucket-distribution — distribuição percentual por bucket de docs. Sprint #2518.
pub async fn segment_doc_bucket_distribution(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len() as f64;
    let (mut sparse, mut light, mut dense, mut heavy) = (0u64, 0u64, 0u64, 0u64);
    for (_, docs, _) in &segs {
        match docs {
            d if *d < 1_000 => sparse += 1,
            d if *d < 100_000 => light += 1,
            d if *d < 10_000_000 => dense += 1,
            _ => heavy += 1,
        }
    }
    let pct = |v: u64| if n > 0.0 { v as f64 / n * 100.0 } else { 0.0 };
    Json(serde_json::json!({"distribution_pct": {"sparse_lt1k": pct(sparse), "light_1k_100k": pct(light), "dense_100k_10m": pct(dense), "heavy_ge10m": pct(heavy)}, "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/avg-bytes — média de bytes por segmento. Sprint #2523.
pub async fn segment_avg_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_bytes": 0.0, "total_segments": 0}));
    }
    let total: u64 = segs.iter().map(|(_, _, b)| b).sum();
    Json(serde_json::json!({"avg_bytes": total as f64 / n as f64, "total_bytes": total, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-p75 — p75 da densidade docs/byte. Sprint #2488.
pub async fn segment_docs_density_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p75 = densities[3 * n / 4];
    Json(serde_json::json!({"p75_docs_per_byte": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-p90 — p90 da densidade docs/byte. Sprint #2493.
pub async fn segment_docs_density_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (9 * n / 10).min(n - 1);
    Json(serde_json::json!({"p90_docs_per_byte": densities[idx], "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-min — mínimo da densidade docs/byte. Sprint #2498.
pub async fn segment_docs_density_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let min = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).fold(f64::MAX, f64::min);
    Json(serde_json::json!({"min_docs_per_byte": min, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-max — máximo da densidade docs/byte. Sprint #2503.
pub async fn segment_docs_density_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let max = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).fold(f64::MIN, f64::max);
    Json(serde_json::json!({"max_docs_per_byte": max, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-range — range da densidade docs/byte. Sprint #2465.
pub async fn segment_docs_density_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"range_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let range = densities.last().copied().unwrap_or(0.0) - densities.first().copied().unwrap_or(0.0);
    Json(serde_json::json!({"range_docs_per_byte": range, "max": densities.last(), "min": densities.first(), "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-cv — CV da densidade docs/byte. Sprint #2470.
pub async fn segment_docs_density_cv(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"cv_docs_per_byte": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_docs_per_byte": cv, "stddev": stddev, "mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-iqr — IQR da densidade docs/byte. Sprint #2475.
pub async fn segment_docs_density_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"iqr_docs_per_byte": 0.0, "total_segments": n}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q1 = densities[n / 4];
    let q3 = densities[3 * n / 4];
    Json(serde_json::json!({"iqr_docs_per_byte": q3 - q1, "q1": q1, "q3": q3, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-p50 — mediana da densidade docs/byte. Sprint #2480.
pub async fn segment_docs_density_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = if n % 2 == 0 {
        (densities[n / 2 - 1] + densities[n / 2]) / 2.0
    } else {
        densities[n / 2]
    };
    Json(serde_json::json!({"p50_docs_per_byte": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-stddev — stddev de docs por byte. Sprint #2460.
pub async fn segment_docs_density_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"stddev_docs_per_byte": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"stddev_docs_per_byte": variance.sqrt(), "avg_docs_per_byte": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-skewness — skewness de docs por byte entre segmentos. Sprint #3048.
pub async fn segment_docs_density_skewness(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skewness_docs_per_byte": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let skewness = if stddev > 0.0 {
        densities.iter().map(|d| ((d - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skewness_docs_per_byte": skewness, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-kurtosis — kurtosis de docs por byte entre segmentos. Sprint #3053.
pub async fn segment_docs_density_kurtosis(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"kurtosis_docs_per_byte": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let kurtosis = if stddev > 0.0 {
        densities.iter().map(|d| ((d - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
    } else { 0.0 };
    Json(serde_json::json!({"kurtosis_docs_per_byte": kurtosis, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-p10 — P10 de docs por byte entre segmentos. Sprint #3058.
pub async fn segment_docs_density_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64) * 0.10).ceil() as usize;
    let p10 = densities[idx.saturating_sub(1).min(n - 1)];
    Json(serde_json::json!({"p10_docs_per_byte": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-density-p25 — P25 de docs por byte entre segmentos. Sprint #3063.
pub async fn segment_docs_density_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_docs_per_byte": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64) * 0.25).ceil() as usize;
    let p25 = densities[idx.saturating_sub(1).min(n - 1)];
    Json(serde_json::json!({"p25_docs_per_byte": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-cv — CV da densidade bytes/doc. Sprint #2425.
pub async fn segment_byte_density_cv(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"cv_byte_density": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_byte_density": cv, "stddev": stddev, "mean": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-iqr — IQR da densidade bytes/doc. Sprint #2430.
pub async fn segment_byte_density_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"iqr_byte_density": null, "total_segments": n}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p25 = densities[n / 4];
    let p75 = densities[3 * n / 4];
    Json(serde_json::json!({"iqr_byte_density": p75 - p25, "p75": p75, "p25": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-skewness — skewness da densidade bytes/doc. Sprint #3068.
pub async fn segment_byte_density_skewness(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skewness_byte_density": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let skewness = if stddev > 0.0 {
        densities.iter().map(|d| ((d - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skewness_byte_density": skewness, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-kurtosis — kurtosis da densidade bytes/doc. Sprint #3073.
pub async fn segment_byte_density_kurtosis(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"kurtosis_byte_density": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let kurtosis = if stddev > 0.0 {
        densities.iter().map(|d| ((d - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
    } else { 0.0 };
    Json(serde_json::json!({"kurtosis_byte_density": kurtosis, "mean": mean, "stddev": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-p10 — P10 da densidade bytes/doc. Sprint #3078.
pub async fn segment_byte_density_p10(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p10_byte_density": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64) * 0.10).ceil() as usize;
    let p10 = densities[idx.saturating_sub(1).min(n - 1)];
    Json(serde_json::json!({"p10_byte_density": p10, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-p25 — P25 da densidade bytes/doc. Sprint #3083.
pub async fn segment_byte_density_p25(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p25_byte_density": 0.0, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((n as f64) * 0.25).ceil() as usize;
    let p25 = densities[idx.saturating_sub(1).min(n - 1)];
    Json(serde_json::json!({"p25_byte_density": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-sum — soma total da densidade bytes/doc. Sprint #3088.
pub async fn segment_byte_density_sum(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"sum_byte_density": 0.0, "total_segments": 0}));
    }
    let sum: f64 = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).sum();
    Json(serde_json::json!({"sum_byte_density": sum, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-count — contagem de segmentos com density calculável. Sprint #3093.
pub async fn segment_byte_density_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total = segs.len();
    let with_docs = segs.iter().filter(|(_, docs, _)| *docs > 0).count();
    Json(serde_json::json!({"segments_with_density": with_docs, "total_segments": total}))
}

/// GET /api/v1/search/index/segments/byte-density-variance — variância da densidade bytes/doc. Sprint #3098.
pub async fn segment_byte_density_variance(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"variance_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"variance_byte_density": variance, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-mad — MAD da densidade bytes/doc. Sprint #3103.
pub async fn segment_byte_density_mad(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mad_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let mad = densities.iter().map(|d| (d - mean).abs()).sum::<f64>() / n as f64;
    Json(serde_json::json!({"mad_byte_density": mad, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-entropy — entropia da densidade bytes/doc. Sprint #3108.
pub async fn segment_byte_density_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"entropy_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let total: f64 = densities.iter().sum();
    let entropy = if total > 0.0 {
        densities.iter().map(|&d| {
            let p = d / total;
            if p > 0.0 { -p * p.ln() } else { 0.0 }
        }).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"entropy_byte_density": entropy, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-gini — coeficiente Gini da densidade bytes/doc. Sprint #3113.
pub async fn segment_byte_density_gini(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"gini_byte_density": null, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let sum: f64 = densities.iter().sum();
    let gini = if sum > 0.0 {
        let weighted: f64 = densities.iter().enumerate().map(|(i, &d)| (2 * (i + 1) - n - 1) as f64 * d).sum();
        weighted / (n as f64 * sum)
    } else { 0.0 };
    Json(serde_json::json!({"gini_byte_density": gini, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-hhi — índice HHI da densidade bytes/doc. Sprint #3118.
pub async fn segment_byte_density_hhi(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"hhi_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let total: f64 = densities.iter().sum();
    let hhi = if total > 0.0 {
        densities.iter().map(|&d| (d / total).powi(2)).sum::<f64>()
    } else { 0.0 };
    Json(serde_json::json!({"hhi_byte_density": hhi, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-theil — índice Theil da densidade bytes/doc. Sprint #3123.
pub async fn segment_byte_density_theil(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"theil_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let theil = if mean > 0.0 {
        densities.iter().map(|&d| {
            if d > 0.0 { (d / mean) * (d / mean).ln() } else { 0.0 }
        }).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"theil_byte_density": theil, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-atkinson — índice Atkinson da densidade bytes/doc. Sprint #3128.
pub async fn segment_byte_density_atkinson(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"atkinson_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let atkinson = if mean > 0.0 {
        let geo_mean = densities.iter().map(|&d| if d > 0.0 { d.ln() } else { f64::NEG_INFINITY }).sum::<f64>() / n as f64;
        let geo_mean = if geo_mean.is_finite() { geo_mean.exp() } else { 0.0 };
        1.0 - geo_mean / mean
    } else { 0.0 };
    Json(serde_json::json!({"atkinson_byte_density": atkinson, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-lorenz — curva de Lorenz da densidade bytes/doc. Sprint #3133.
pub async fn segment_byte_density_lorenz(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"lorenz_points": [], "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = densities.iter().sum();
    let mut cumulative = 0.0;
    let points: Vec<serde_json::Value> = densities.iter().enumerate().map(|(i, &d)| {
        cumulative += d;
        serde_json::json!({"population_share": (i + 1) as f64 / n as f64, "density_share": if total > 0.0 { cumulative / total } else { 0.0 }})
    }).collect();
    Json(serde_json::json!({"lorenz_points": points, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-trimmed-mean — média truncada da densidade bytes/doc. Sprint #3138.
pub async fn segment_byte_density_trimmed_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"trimmed_mean_byte_density": null, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let trim = (n as f64 * 0.1).floor() as usize;
    let trimmed = &densities[trim..n - trim];
    let mean = if trimmed.is_empty() { 0.0 } else { trimmed.iter().sum::<f64>() / trimmed.len() as f64 };
    Json(serde_json::json!({"trimmed_mean_byte_density": mean, "trim_pct": 0.1, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-winsorized-mean — média winsorizada da densidade bytes/doc. Sprint #3143.
pub async fn segment_byte_density_winsorized_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"winsorized_mean_byte_density": null, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let trim = (n as f64 * 0.1).floor() as usize;
    let lo = densities[trim];
    let hi = densities[n - 1 - trim];
    let mean = densities.iter().map(|&d| d.max(lo).min(hi)).sum::<f64>() / n as f64;
    Json(serde_json::json!({"winsorized_mean_byte_density": mean, "trim_pct": 0.1, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-harmonic-mean — média harmônica de bytes/doc. Sprint #3157.
pub async fn segment_byte_density_harmonic_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"harmonic_mean_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).filter(|&d| d > 0.0).collect();
    let m = densities.len();
    let hm = if m == 0 { 0.0 } else {
        let inv_sum: f64 = densities.iter().map(|&d| 1.0 / d).sum();
        if inv_sum > 0.0 { m as f64 / inv_sum } else { 0.0 }
    };
    Json(serde_json::json!({"harmonic_mean_byte_density": hm, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-geometric-mean — média geométrica de bytes/doc. Sprint #3158.
pub async fn segment_byte_density_geometric_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"geometric_mean_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).filter(|&d| d > 0.0).collect();
    let m = densities.len();
    let gm = if m == 0 { 0.0 } else {
        let ln_sum: f64 = densities.iter().map(|&d| d.ln()).sum();
        (ln_sum / m as f64).exp()
    };
    Json(serde_json::json!({"geometric_mean_byte_density": gm, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-normalized-entropy — entropia normalizada de bytes/doc. Sprint #3159.
pub async fn segment_byte_density_normalized_entropy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"normalized_entropy_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let total: f64 = densities.iter().sum();
    let entropy = if total <= 0.0 { 0.0 } else {
        densities.iter().fold(0.0f64, |acc, &d| {
            if d > 0.0 { let p = d / total; acc - p * p.ln() } else { acc }
        })
    };
    let max_entropy = if n > 1 { (n as f64).ln() } else { 1.0 };
    let normalized = if max_entropy > 0.0 { entropy / max_entropy } else { 0.0 };
    Json(serde_json::json!({"normalized_entropy_byte_density": normalized, "raw_entropy": entropy, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-avg-docs — segmentos abaixo da média de docs. Sprint #3177.
pub async fn segment_below_avg_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_avg_count": 0}));
    }
    let avg_docs = segs.iter().map(|(_, docs, _)| *docs as f64).sum::<f64>() / n as f64;
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, _)| (*docs as f64) < avg_docs)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "avg_docs": avg_docs, "below_avg_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-avg-bytes — segmentos abaixo da média de bytes. Sprint #3178.
pub async fn segment_below_avg_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_avg_count": 0}));
    }
    let avg_bytes = segs.iter().map(|(_, _, bytes)| *bytes as f64).sum::<f64>() / n as f64;
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, bytes)| (*bytes as f64) < avg_bytes)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "avg_bytes": avg_bytes, "below_avg_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p75-docs — segmentos acima do P75 de docs. Sprint #3179.
pub async fn segment_above_p75_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "above_p75_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, docs, _)| *docs).collect();
    docs_sorted.sort_unstable();
    let p75 = docs_sorted[((n as f64 * 0.75) as usize).min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, _)| *docs > p75)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = above.len();
    Json(serde_json::json!({"segments": above, "p75_docs": p75, "above_p75_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p75-bytes — segmentos acima do P75 de bytes. Sprint #3180.
pub async fn segment_above_p75_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "above_p75_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, bytes)| *bytes).collect();
    bytes_sorted.sort_unstable();
    let p75 = bytes_sorted[((n as f64 * 0.75) as usize).min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, bytes)| *bytes > p75)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = above.len();
    Json(serde_json::json!({"segments": above, "p75_bytes": p75, "above_p75_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p90-docs — segmentos acima do P90 de docs. Sprint #3197.
pub async fn segment_above_p90_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "above_p90_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, docs, _)| *docs).collect();
    docs_sorted.sort_unstable();
    let p90 = docs_sorted[((n as f64 * 0.90) as usize).min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, _)| *docs > p90)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = above.len();
    Json(serde_json::json!({"segments": above, "p90_docs": p90, "above_p90_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p90-bytes — segmentos acima do P90 de bytes. Sprint #3198.
pub async fn segment_above_p90_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "above_p90_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, bytes)| *bytes).collect();
    bytes_sorted.sort_unstable();
    let p90 = bytes_sorted[((n as f64 * 0.90) as usize).min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, bytes)| *bytes > p90)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = above.len();
    Json(serde_json::json!({"segments": above, "p90_bytes": p90, "above_p90_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p75-density — segmentos acima do P75 de densidade bytes/doc. Sprint #3199.
pub async fn segment_above_p75_density(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "above_p75_count": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p75 = densities[((n as f64 * 0.75) as usize).min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, bytes)| {
            let d = if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 };
            d > p75
        })
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = above.len();
    Json(serde_json::json!({"segments": above, "p75_density": p75, "above_p75_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p90-density — segmentos acima do P90 de densidade bytes/doc. Sprint #3200.
pub async fn segment_above_p90_density(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "above_p90_count": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p90 = densities[((n as f64 * 0.90) as usize).min(n - 1)];
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, bytes)| {
            let d = if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 };
            d > p90
        })
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = above.len();
    Json(serde_json::json!({"segments": above, "p90_density": p90, "above_p90_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p25-docs — segmentos abaixo do P25 de docs. Sprint #3217.
pub async fn segment_below_p25_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_p25_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, docs, _)| *docs).collect();
    docs_sorted.sort_unstable();
    let p25 = docs_sorted[((n as f64 * 0.25) as usize).min(n - 1)];
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, _)| *docs < p25)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "p25_docs": p25, "below_p25_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p25-bytes — segmentos abaixo do P25 de bytes. Sprint #3218.
pub async fn segment_below_p25_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_p25_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, bytes)| *bytes).collect();
    bytes_sorted.sort_unstable();
    let p25 = bytes_sorted[((n as f64 * 0.25) as usize).min(n - 1)];
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, bytes)| *bytes < p25)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "p25_bytes": p25, "below_p25_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p10-docs — segmentos abaixo do P10 de docs. Sprint #3219.
pub async fn segment_below_p10_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_p10_count": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, docs, _)| *docs).collect();
    docs_sorted.sort_unstable();
    let p10 = docs_sorted[((n as f64 * 0.10) as usize).min(n - 1)];
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, _)| *docs < p10)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "p10_docs": p10, "below_p10_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p10-bytes — segmentos abaixo do P10 de bytes. Sprint #3220.
pub async fn segment_below_p10_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_p10_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, bytes)| *bytes).collect();
    bytes_sorted.sort_unstable();
    let p10 = bytes_sorted[((n as f64 * 0.10) as usize).min(n - 1)];
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, bytes)| *bytes < p10)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "p10_bytes": p10, "below_p10_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p10-density — segmentos abaixo do P10 de densidade. Sprint #3237.
pub async fn segment_below_p10_density(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_p10_count": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p10 = densities[((n as f64 * 0.10) as usize).min(n - 1)];
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, bytes)| {
            let d = if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 };
            d < p10
        })
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "p10_density": p10, "below_p10_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p25-density — segmentos abaixo do P25 de densidade. Sprint #3238.
pub async fn segment_below_p25_density(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "below_p25_count": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p25 = densities[((n as f64 * 0.25) as usize).min(n - 1)];
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, docs, bytes)| {
            let d = if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 };
            d < p25
        })
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let cnt = below.len();
    Json(serde_json::json!({"segments": below, "p25_density": p25, "below_p25_count": cnt, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-above-avg-docs — contagem de segmentos acima da média de docs. Sprint #3239.
pub async fn segment_count_above_avg_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_avg": 0, "total_segments": 0, "avg_docs": null}));
    }
    let avg = segs.iter().map(|(_, docs, _)| *docs as f64).sum::<f64>() / n as f64;
    let count = segs.iter().filter(|(_, docs, _)| (*docs as f64) > avg).count();
    Json(serde_json::json!({"count_above_avg": count, "avg_docs": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-above-avg-bytes — contagem de segmentos acima da média de bytes. Sprint #3240.
pub async fn segment_count_above_avg_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_above_avg": 0, "total_segments": 0, "avg_bytes": null}));
    }
    let avg = segs.iter().map(|(_, _, bytes)| *bytes as f64).sum::<f64>() / n as f64;
    let count = segs.iter().filter(|(_, _, bytes)| (*bytes as f64) > avg).count();
    Json(serde_json::json!({"count_above_avg": count, "avg_bytes": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-below-avg-docs — contagem de segmentos abaixo da média de docs. Sprint #3257.
pub async fn segment_count_below_avg_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_avg": 0, "total_segments": 0, "avg_docs": null}));
    }
    let avg = segs.iter().map(|(_, docs, _)| *docs as f64).sum::<f64>() / n as f64;
    let count = segs.iter().filter(|(_, docs, _)| (*docs as f64) < avg).count();
    Json(serde_json::json!({"count_below_avg": count, "avg_docs": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-below-avg-bytes — contagem de segmentos abaixo da média de bytes. Sprint #3258.
pub async fn segment_count_below_avg_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_avg": 0, "total_segments": 0, "avg_bytes": null}));
    }
    let avg = segs.iter().map(|(_, _, bytes)| *bytes as f64).sum::<f64>() / n as f64;
    let count = segs.iter().filter(|(_, _, bytes)| (*bytes as f64) < avg).count();
    Json(serde_json::json!({"count_below_avg": count, "avg_bytes": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-avg-docs — fração de segmentos acima da média de docs. Sprint #3259.
pub async fn segment_ratio_above_avg_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_avg": null, "total_segments": 0}));
    }
    let avg = segs.iter().map(|(_, docs, _)| *docs as f64).sum::<f64>() / n as f64;
    let above = segs.iter().filter(|(_, docs, _)| (*docs as f64) > avg).count();
    let ratio = above as f64 / n as f64;
    Json(serde_json::json!({"ratio_above_avg": ratio, "count_above_avg": above, "avg_docs": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-avg-bytes — fração de segmentos acima da média de bytes. Sprint #3260.
pub async fn segment_ratio_above_avg_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_avg": null, "total_segments": 0}));
    }
    let avg = segs.iter().map(|(_, _, bytes)| *bytes as f64).sum::<f64>() / n as f64;
    let above = segs.iter().filter(|(_, _, bytes)| (*bytes as f64) > avg).count();
    let ratio = above as f64 / n as f64;
    Json(serde_json::json!({"ratio_above_avg": ratio, "count_above_avg": above, "avg_bytes": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-avg-docs — fração de segmentos abaixo da média de docs. Sprint #3277.
pub async fn segment_ratio_below_avg_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_avg": null, "total_segments": 0}));
    }
    let avg = segs.iter().map(|(_, docs, _)| *docs as f64).sum::<f64>() / n as f64;
    let below = segs.iter().filter(|(_, docs, _)| (*docs as f64) < avg).count();
    let ratio = below as f64 / n as f64;
    Json(serde_json::json!({"ratio_below_avg": ratio, "count_below_avg": below, "avg_docs": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-avg-bytes — fração de segmentos abaixo da média de bytes. Sprint #3278.
pub async fn segment_ratio_below_avg_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_avg": null, "total_segments": 0}));
    }
    let avg = segs.iter().map(|(_, _, bytes)| *bytes as f64).sum::<f64>() / n as f64;
    let below = segs.iter().filter(|(_, _, bytes)| (*bytes as f64) < avg).count();
    let ratio = below as f64 / n as f64;
    Json(serde_json::json!({"ratio_below_avg": ratio, "count_below_avg": below, "avg_bytes": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p25-docs — segmentos com docs acima do P25. Sprint #3279.
pub async fn segment_above_p25_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p25_docs": null}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p25_idx = n / 4;
    let p25 = docs_sorted[p25_idx] as f64;
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, d, _)| (*d as f64) > p25)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": above, "p25_docs": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p25-bytes — segmentos com bytes acima do P25. Sprint #3280.
pub async fn segment_above_p25_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p25_bytes": null}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p25_idx = n / 4;
    let p25 = bytes_sorted[p25_idx] as f64;
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, b)| (*b as f64) > p25)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": above, "p25_bytes": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p50-docs — segmentos com docs acima do P50. Sprint #3297.
pub async fn segment_above_p50_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p50_docs": null}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p50 = if n % 2 == 0 { (docs_sorted[n/2 - 1] + docs_sorted[n/2]) as f64 / 2.0 } else { docs_sorted[n/2] as f64 };
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, d, _)| (*d as f64) > p50)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": above, "p50_docs": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/above-p50-bytes — segmentos com bytes acima do P50. Sprint #3298.
pub async fn segment_above_p50_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p50_bytes": null}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p50 = if n % 2 == 0 { (bytes_sorted[n/2 - 1] + bytes_sorted[n/2]) as f64 / 2.0 } else { bytes_sorted[n/2] as f64 };
    let above: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, b)| (*b as f64) > p50)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": above, "p50_bytes": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p50-docs — segmentos com docs abaixo do P50. Sprint #3299.
pub async fn segment_below_p50_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p50_docs": null}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p50 = if n % 2 == 0 { (docs_sorted[n/2 - 1] + docs_sorted[n/2]) as f64 / 2.0 } else { docs_sorted[n/2] as f64 };
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, d, _)| (*d as f64) < p50)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": below, "p50_docs": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p50-bytes — segmentos com bytes abaixo do P50. Sprint #3300.
pub async fn segment_below_p50_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p50_bytes": null}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p50 = if n % 2 == 0 { (bytes_sorted[n/2 - 1] + bytes_sorted[n/2]) as f64 / 2.0 } else { bytes_sorted[n/2] as f64 };
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, b)| (*b as f64) < p50)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": below, "p50_bytes": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p75-docs — segmentos com docs abaixo do P75. Sprint #3317.
pub async fn segment_below_p75_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p75_docs": null}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p75_idx = (n as f64 * 0.75) as usize;
    let p75 = docs_sorted[p75_idx.min(n - 1)] as f64;
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, d, _)| (*d as f64) < p75)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": below, "p75_docs": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p75-bytes — segmentos com bytes abaixo do P75. Sprint #3318.
pub async fn segment_below_p75_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p75_bytes": null}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p75_idx = (n as f64 * 0.75) as usize;
    let p75 = bytes_sorted[p75_idx.min(n - 1)] as f64;
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, b)| (*b as f64) < p75)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": below, "p75_bytes": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p90-docs — segmentos com docs abaixo do P90. Sprint #3319.
pub async fn segment_below_p90_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p90_docs": null}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p90_idx = (n as f64 * 0.90) as usize;
    let p90 = docs_sorted[p90_idx.min(n - 1)] as f64;
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, d, _)| (*d as f64) < p90)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": below, "p90_docs": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/below-p90-bytes — segmentos com bytes abaixo do P90. Sprint #3320.
pub async fn segment_below_p90_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0, "p90_bytes": null}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p90_idx = (n as f64 * 0.90) as usize;
    let p90 = bytes_sorted[p90_idx.min(n - 1)] as f64;
    let below: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, b)| (*b as f64) < p90)
        .map(|(id, d, b)| serde_json::json!({"segment_id": id, "docs": d, "bytes": b}))
        .collect();
    Json(serde_json::json!({"segments": below, "p90_bytes": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p25-docs — fração de segmentos acima do P25 de docs. Sprint #3337.
pub async fn segment_ratio_above_p25_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p25": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p25 = docs_sorted[(n as f64 * 0.25) as usize] as f64;
    let above = segs.iter().filter(|(_, d, _)| (*d as f64) > p25).count();
    Json(serde_json::json!({"ratio_above_p25": above as f64 / n as f64, "count_above_p25": above, "p25_docs": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p50-docs — fração de segmentos acima do P50 de docs. Sprint #3338.
pub async fn segment_ratio_above_p50_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p50": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p50 = if n % 2 == 0 { (docs_sorted[n/2 - 1] + docs_sorted[n/2]) as f64 / 2.0 } else { docs_sorted[n/2] as f64 };
    let above = segs.iter().filter(|(_, d, _)| (*d as f64) > p50).count();
    Json(serde_json::json!({"ratio_above_p50": above as f64 / n as f64, "count_above_p50": above, "p50_docs": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p75-docs — fração de segmentos acima do P75 de docs. Sprint #3339.
pub async fn segment_ratio_above_p75_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p75": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p75 = docs_sorted[((n as f64 * 0.75) as usize).min(n - 1)] as f64;
    let above = segs.iter().filter(|(_, d, _)| (*d as f64) > p75).count();
    Json(serde_json::json!({"ratio_above_p75": above as f64 / n as f64, "count_above_p75": above, "p75_docs": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p90-docs — fração de segmentos acima do P90 de docs. Sprint #3340.
pub async fn segment_ratio_above_p90_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p90": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p90 = docs_sorted[((n as f64 * 0.90) as usize).min(n - 1)] as f64;
    let above = segs.iter().filter(|(_, d, _)| (*d as f64) > p90).count();
    Json(serde_json::json!({"ratio_above_p90": above as f64 / n as f64, "count_above_p90": above, "p90_docs": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p25-docs — fração de segmentos abaixo do P25 de docs. Sprint #3397.
pub async fn segment_ratio_below_p25_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p25_docs": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p25 = docs_sorted[((n as f64 * 0.25) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, d, _)| (*d as f64) < p25).count();
    Json(serde_json::json!({"ratio_below_p25_docs": below as f64 / n as f64, "count_below_p25_docs": below, "p25_docs": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p50-docs — fração de segmentos abaixo do P50 de docs. Sprint #3398.
pub async fn segment_ratio_below_p50_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p50_docs": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p50 = docs_sorted[((n as f64 * 0.50) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, d, _)| (*d as f64) < p50).count();
    Json(serde_json::json!({"ratio_below_p50_docs": below as f64 / n as f64, "count_below_p50_docs": below, "p50_docs": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p75-docs — fração de segmentos abaixo do P75 de docs. Sprint #3399.
pub async fn segment_ratio_below_p75_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p75_docs": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p75 = docs_sorted[((n as f64 * 0.75) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, d, _)| (*d as f64) < p75).count();
    Json(serde_json::json!({"ratio_below_p75_docs": below as f64 / n as f64, "count_below_p75_docs": below, "p75_docs": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p90-docs — fração de segmentos abaixo do P90 de docs. Sprint #3400.
pub async fn segment_ratio_below_p90_docs(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p90_docs": null, "total_segments": 0}));
    }
    let mut docs_sorted: Vec<u64> = segs.iter().map(|(_, d, _)| *d).collect();
    docs_sorted.sort_unstable();
    let p90 = docs_sorted[((n as f64 * 0.90) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, d, _)| (*d as f64) < p90).count();
    Json(serde_json::json!({"ratio_below_p90_docs": below as f64 / n as f64, "count_below_p90_docs": below, "p90_docs": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p25-bytes — fração de segmentos abaixo do P25 de bytes. Sprint #3377.
pub async fn segment_ratio_below_p25_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p25_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p25 = bytes_sorted[((n as f64 * 0.25) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, _, b)| (*b as f64) < p25).count();
    Json(serde_json::json!({"ratio_below_p25_bytes": below as f64 / n as f64, "count_below_p25_bytes": below, "p25_bytes": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p50-bytes — fração de segmentos abaixo do P50 de bytes. Sprint #3378.
pub async fn segment_ratio_below_p50_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p50_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p50 = bytes_sorted[((n as f64 * 0.50) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, _, b)| (*b as f64) < p50).count();
    Json(serde_json::json!({"ratio_below_p50_bytes": below as f64 / n as f64, "count_below_p50_bytes": below, "p50_bytes": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p75-bytes — fração de segmentos abaixo do P75 de bytes. Sprint #3379.
pub async fn segment_ratio_below_p75_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p75_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p75 = bytes_sorted[((n as f64 * 0.75) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, _, b)| (*b as f64) < p75).count();
    Json(serde_json::json!({"ratio_below_p75_bytes": below as f64 / n as f64, "count_below_p75_bytes": below, "p75_bytes": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-below-p90-bytes — fração de segmentos abaixo do P90 de bytes. Sprint #3380.
pub async fn segment_ratio_below_p90_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_below_p90_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p90 = bytes_sorted[((n as f64 * 0.90) as usize).min(n - 1)] as f64;
    let below = segs.iter().filter(|(_, _, b)| (*b as f64) < p90).count();
    Json(serde_json::json!({"ratio_below_p90_bytes": below as f64 / n as f64, "count_below_p90_bytes": below, "p90_bytes": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p25-bytes — fração de segmentos acima do P25 de bytes. Sprint #3357.
pub async fn segment_ratio_above_p25_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p25_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p25 = bytes_sorted[((n as f64 * 0.25) as usize).min(n - 1)] as f64;
    let above = segs.iter().filter(|(_, _, b)| (*b as f64) > p25).count();
    Json(serde_json::json!({"ratio_above_p25_bytes": above as f64 / n as f64, "count_above_p25_bytes": above, "p25_bytes": p25, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p50-bytes — fração de segmentos acima do P50 de bytes. Sprint #3358.
pub async fn segment_ratio_above_p50_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p50_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p50 = bytes_sorted[((n as f64 * 0.50) as usize).min(n - 1)] as f64;
    let above = segs.iter().filter(|(_, _, b)| (*b as f64) > p50).count();
    Json(serde_json::json!({"ratio_above_p50_bytes": above as f64 / n as f64, "count_above_p50_bytes": above, "p50_bytes": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p75-bytes — fração de segmentos acima do P75 de bytes. Sprint #3359.
pub async fn segment_ratio_above_p75_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p75_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p75 = bytes_sorted[((n as f64 * 0.75) as usize).min(n - 1)] as f64;
    let above = segs.iter().filter(|(_, _, b)| (*b as f64) > p75).count();
    Json(serde_json::json!({"ratio_above_p75_bytes": above as f64 / n as f64, "count_above_p75_bytes": above, "p75_bytes": p75, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/ratio-above-p90-bytes — fração de segmentos acima do P90 de bytes. Sprint #3360.
pub async fn segment_ratio_above_p90_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ratio_above_p90_bytes": null, "total_segments": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    bytes_sorted.sort_unstable();
    let p90 = bytes_sorted[((n as f64 * 0.90) as usize).min(n - 1)] as f64;
    let above = segs.iter().filter(|(_, _, b)| (*b as f64) > p90).count();
    Json(serde_json::json!({"ratio_above_p90_bytes": above as f64 / n as f64, "count_above_p90_bytes": above, "p90_bytes": p90, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-range — range da densidade bytes/doc. Sprint #2435.
pub async fn segment_byte_density_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"range_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let max = densities.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min = densities.iter().cloned().fold(f64::INFINITY, f64::min);
    Json(serde_json::json!({"range_byte_density": max - min, "max": max, "min": min, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-p50 — mediana da densidade bytes/doc. Sprint #2440.
pub async fn segment_byte_density_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_byte_density": null, "total_segments": 0}));
    }
    let mut densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    densities.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = if n % 2 == 1 {
        densities[n / 2]
    } else {
        (densities[n / 2 - 1] + densities[n / 2]) / 2.0
    };
    Json(serde_json::json!({"p50_byte_density": p50, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-avg — média da densidade bytes/doc. Sprint #2408.
pub async fn segment_byte_density_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_byte_density": null, "total_segments": 0}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let avg = densities.iter().sum::<f64>() / n as f64;
    Json(serde_json::json!({"avg_byte_density": avg, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/byte-density-max — máxima densidade bytes/doc. Sprint #2413.
pub async fn segment_byte_density_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_byte_density": null, "segment": null, "total_segments": 0}));
    }
    let densest = segs.iter().map(|(id, docs, bytes)| {
        let d = if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 };
        (id.clone(), *docs, *bytes, d)
    }).max_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
    match densest {
        Some((id, docs, bytes, density)) => Json(serde_json::json!({"max_byte_density": density, "segment_id": id, "num_docs": docs, "disk_bytes": bytes, "total_segments": n})),
        None => Json(serde_json::json!({"max_byte_density": null, "total_segments": n})),
    }
}

/// GET /api/v1/search/index/segments/byte-density-min — mínima densidade bytes/doc. Sprint #2418.
pub async fn segment_byte_density_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_byte_density": null, "segment": null, "total_segments": 0}));
    }
    let sparsest = segs.iter().map(|(id, docs, bytes)| {
        let d = if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 };
        (id.clone(), *docs, *bytes, d)
    }).min_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
    match sparsest {
        Some((id, docs, bytes, density)) => Json(serde_json::json!({"min_byte_density": density, "segment_id": id, "num_docs": docs, "disk_bytes": bytes, "total_segments": n})),
        None => Json(serde_json::json!({"min_byte_density": null, "total_segments": n})),
    }
}

/// GET /api/v1/search/index/segments/byte-density-stddev — desvio padrão da densidade bytes/doc. Sprint #2423.
pub async fn segment_byte_density_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"stddev_byte_density": 0.0, "total_segments": n}));
    }
    let densities: Vec<f64> = segs.iter().map(|(_, docs, bytes)| {
        if *docs > 0 { *bytes as f64 / *docs as f64 } else { 0.0 }
    }).collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let variance = densities.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    Json(serde_json::json!({"stddev_byte_density": stddev, "avg_byte_density": mean, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/bytes-above-avg — segmentos com bytes acima da média. Sprint #2385.
pub async fn segment_bytes_above_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| b).sum();
    let mean = total_bytes as f64 / n as f64;
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, b)| (*b as f64) > mean)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let count = above.len();
    Json(serde_json::json!({"segments": above, "count": count, "total_segments": n, "mean_bytes": mean}))
}

/// GET /api/v1/search/index/segments/docs-above-avg — segmentos com docs acima da média. Sprint #2390.
pub async fn segment_docs_above_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, d, _)| d).sum();
    let mean = total_docs as f64 / n as f64;
    let above: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, d, _)| (*d as f64) > mean)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    let count = above.len();
    Json(serde_json::json!({"segments": above, "count": count, "total_segments": n, "mean_docs": mean}))
}

/// GET /api/v1/search/index/segments/size-outliers — segmentos com bytes > média + 2*stddev. Sprint #2395.
pub async fn segment_size_outliers(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"outliers": [], "total_segments": n}));
    }
    let bytes: Vec<f64> = segs.iter().map(|(_, _, b)| *b as f64).collect();
    let mean = bytes.iter().sum::<f64>() / n as f64;
    let variance = bytes.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let threshold = mean + 2.0 * stddev;
    let outliers: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, _, b)| (*b as f64) > threshold)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    Json(serde_json::json!({"outliers": outliers, "threshold_bytes": threshold as u64, "mean_bytes": mean, "stddev_bytes": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/docs-outliers — segmentos com docs > média + 2*stddev. Sprint #2400.
pub async fn segment_docs_outliers(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"outliers": [], "total_segments": n}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let variance = docs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let threshold = mean + 2.0 * stddev;
    let outliers: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, d, _)| (*d as f64) > threshold)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    Json(serde_json::json!({"outliers": outliers, "threshold_docs": threshold as u64, "mean_docs": mean, "stddev_docs": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/merge-candidates — segmentos candidatos a merge (abaixo de 10% da média). Sprint #2365.
pub async fn segment_merge_candidates(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"merge_candidates": [], "total_segments": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, d, _)| d).sum();
    let mean = total_docs as f64 / n as f64;
    let threshold = mean * 0.10;
    let candidates: Vec<serde_json::Value> = segs
        .iter()
        .filter(|(_, docs, _)| (*docs as f64) < threshold)
        .map(|(id, docs, bytes)| serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}))
        .collect();
    Json(serde_json::json!({"merge_candidates": candidates, "total_segments": n, "threshold_docs": threshold as u64}))
}

/// GET /api/v1/search/index/segments/hot-ratio — fração de segmentos com docs acima da média. Sprint #2370.
pub async fn segment_hot_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"hot_ratio": 0.0, "total_segments": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, d, _)| d).sum();
    let mean = total_docs as f64 / n as f64;
    let hot = segs.iter().filter(|(_, d, _)| (*d as f64) > mean).count();
    let ratio = hot as f64 / n as f64;
    Json(serde_json::json!({"hot_ratio": ratio, "hot_count": hot, "total_segments": n, "mean_docs": mean}))
}

/// GET /api/v1/search/index/segments/cold-ratio — fração de segmentos com docs abaixo de 50% da média. Sprint #2375.
pub async fn segment_cold_ratio(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"cold_ratio": 0.0, "total_segments": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, d, _)| d).sum();
    let mean = total_docs as f64 / n as f64;
    let cold = segs.iter().filter(|(_, d, _)| (*d as f64) < mean * 0.5).count();
    let ratio = cold as f64 / n as f64;
    Json(serde_json::json!({"cold_ratio": ratio, "cold_count": cold, "total_segments": n, "mean_docs": mean}))
}

/// GET /api/v1/search/index/segments/anomaly-score — score de anomalia baseado em z-score máximo. Sprint #2380.
pub async fn segment_anomaly_score(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"anomaly_score": 0.0, "total_segments": n}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, d, _)| *d as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let variance = docs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let max_zscore = if stddev > 0.0 {
        docs.iter().map(|d| ((d - mean) / stddev).abs()).fold(0.0_f64, f64::max)
    } else {
        0.0
    };
    Json(serde_json::json!({"anomaly_score": max_zscore, "mean_docs": mean, "stddev_docs": stddev, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/count-below-avg — segmentos com docs abaixo da média. Sprint #2348.
pub async fn segment_count_below_avg(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"count_below_avg": 0, "total_segments": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, d, _)| d).sum();
    let mean = total_docs as f64 / n as f64;
    let below = segs.iter().filter(|(_, d, _)| (*d as f64) < mean).count();
    Json(serde_json::json!({"count_below_avg": below, "total_segments": n, "mean_docs": mean}))
}

/// GET /api/v1/search/index/segments/density-rank — ranking de segmentos por densidade (docs/byte). Sprint #2353.
pub async fn segment_density_rank(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"ranked": [], "total_segments": 0}));
    }
    let mut ranked: Vec<(String, f64)> = segs
        .iter()
        .map(|(id, docs, bytes)| {
            let density = if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 };
            (id.clone(), density)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let result: Vec<serde_json::Value> = ranked
        .into_iter()
        .enumerate()
        .map(|(i, (id, density))| serde_json::json!({"rank": i + 1, "segment_id": id, "docs_per_byte": density}))
        .collect();
    Json(serde_json::json!({"ranked": result, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/efficiency — razão docs/byte normalizada pelo máximo. Sprint #2358.
pub async fn segment_efficiency(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segments": [], "total_segments": 0}));
    }
    let densities: Vec<f64> = segs
        .iter()
        .map(|(_, docs, bytes)| if *bytes > 0 { *docs as f64 / *bytes as f64 } else { 0.0 })
        .collect();
    let max_density = densities.iter().cloned().fold(0.0_f64, f64::max);
    let result: Vec<serde_json::Value> = segs
        .iter()
        .zip(densities.iter())
        .map(|((id, docs, bytes), density)| {
            let efficiency = if max_density > 0.0 { density / max_density } else { 0.0 };
            serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes, "efficiency": efficiency})
        })
        .collect();
    Json(serde_json::json!({"segments": result, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/compaction-score — score de compactação: 1 - (n_segs / max_segs). Sprint #2363.
pub async fn segment_compaction_score(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    let max_segs: usize = 100;
    let score = if n >= max_segs { 0.0 } else { 1.0 - (n as f64 / max_segs as f64) };
    let total_docs: u64 = segs.iter().map(|(_, d, _)| d).sum();
    let total_bytes: u64 = segs.iter().map(|(_, _, b)| b).sum();
    Json(serde_json::json!({"compaction_score": score, "segment_count": n, "total_docs": total_docs, "total_bytes": total_bytes}))
}

/// GET /api/v1/search/index/segments/top-heavy — segmentos que concentram a maior parte dos docs. Sprint #2328.
pub async fn segment_top_heavy(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total_docs: u64 = segs.iter().map(|(_, d, _)| d).sum();
    if total_docs == 0 {
        return Json(serde_json::json!({"top_heavy_segments": [], "total_docs": 0}));
    }
    let threshold = total_docs / 2;
    let mut cumulative: u64 = 0;
    let mut top_heavy: Vec<serde_json::Value> = Vec::new();
    let mut sorted = segs.clone();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    for (id, docs, bytes) in sorted {
        if cumulative >= threshold { break; }
        cumulative += docs;
        top_heavy.push(serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes}));
    }
    Json(serde_json::json!({"top_heavy_segments": top_heavy, "total_docs": total_docs, "covers_docs": cumulative}))
}

/// GET /api/v1/search/index/segments/largest — segmento com maior tamanho em bytes. Sprint #2333.
pub async fn segment_largest(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segment": null, "total_segments": 0}));
    }
    let largest = segs.iter().max_by_key(|(_, _, b)| b);
    match largest {
        Some((id, docs, bytes)) => Json(serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes, "total_segments": n})),
        None => Json(serde_json::json!({"segment": null, "total_segments": n})),
    }
}

/// GET /api/v1/search/index/segments/smallest — segmento com menor tamanho em bytes. Sprint #2338.
pub async fn segment_smallest(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"segment": null, "total_segments": 0}));
    }
    let smallest = segs.iter().min_by_key(|(_, _, b)| b);
    match smallest {
        Some((id, docs, bytes)) => Json(serde_json::json!({"segment_id": id, "num_docs": docs, "disk_bytes": bytes, "total_segments": n})),
        None => Json(serde_json::json!({"segment": null, "total_segments": n})),
    }
}

/// GET /api/v1/search/index/segments/median-bytes — mediana do tamanho em bytes dos segmentos. Sprint #2343.
pub async fn segment_median_bytes(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"median_bytes": null, "total_segments": 0}));
    }
    let mut sizes: Vec<u64> = segs.iter().map(|(_, _, b)| *b).collect();
    sizes.sort_unstable();
    let median = if n % 2 == 1 {
        sizes[n / 2] as f64
    } else {
        (sizes[n / 2 - 1] + sizes[n / 2]) as f64 / 2.0
    };
    Json(serde_json::json!({"median_bytes": median, "total_segments": n}))
}

/// GET /api/v1/search/index/segments/age — segmento mais antigo pelo id lexicográfico (proxy de idade). Sprint #2308.
pub async fn segment_age(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"oldest_segment_id": null, "segment_count": 0}));
    }
    let oldest = segs.iter().min_by(|a, b| a.0.cmp(&b.0));
    let newest = segs.iter().max_by(|a, b| a.0.cmp(&b.0));
    Json(serde_json::json!({
        "oldest_segment_id": oldest.map(|(id, _, _)| id),
        "newest_segment_id": newest.map(|(id, _, _)| id),
        "segment_count": n
    }))
}

/// GET /api/v1/search/index/segments/churn — segmentos com 0 bytes (descartados/esvaziados). Sprint #2313.
pub async fn segment_churn(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let churned: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db == 0)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"churned_segments": churned, "count": churned.len(), "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/waste — bytes em segmentos vazios (desperdício de disco). Sprint #2318.
pub async fn segment_waste(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let waste_bytes: u64 = segs.iter()
        .filter(|(_, nd, _)| *nd == 0)
        .map(|(_, _, db)| *db)
        .sum();
    let total_bytes: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let pct = if total_bytes > 0 { waste_bytes as f64 / total_bytes as f64 * 100.0 } else { 0.0 };
    Json(serde_json::json!({"waste_bytes": waste_bytes, "waste_pct": pct, "total_bytes": total_bytes, "segment_count": segs.len()}))
}

/// GET /api/v1/search/index/segments/throughput — docs por byte em segmentos não-vazios (throughput médio de indexação). Sprint #2323.
pub async fn segment_throughput(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let active: Vec<_> = segs.iter().filter(|(_, nd, db)| *nd > 0 && *db > 0).collect();
    let n = active.len();
    if n == 0 {
        return Json(serde_json::json!({"avg_docs_per_byte": null, "active_segments": 0, "total_segments": segs.len()}));
    }
    let avg = active.iter().map(|(_, nd, db)| *nd as f64 / *db as f64).sum::<f64>() / n as f64;
    Json(serde_json::json!({"avg_docs_per_byte": avg, "active_segments": n, "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/fragility — segmentos de um único doc (frágeis, risco de perda total). Sprint #2288.
pub async fn segment_fragility(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let fragile: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd == 1)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"fragile_segments": fragile, "count": fragile.len(), "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/footprint — bytes totais do índice (footprint de disco). Sprint #2293.
pub async fn segment_footprint(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total_bytes: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let total_docs: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    Json(serde_json::json!({"total_bytes": total_bytes, "total_docs": total_docs, "segment_count": segs.len()}))
}

/// GET /api/v1/search/index/segments/load — docs totais do índice (carga de dados indexados). Sprint #2298.
pub async fn segment_load(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let total_docs: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let max_docs = segs.iter().map(|(_, nd, _)| *nd).max().unwrap_or(0);
    let n = segs.len();
    Json(serde_json::json!({"total_docs": total_docs, "max_segment_docs": max_docs, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/coverage — % de segmentos não-vazios (cobertura de indexação). Sprint #2303.
pub async fn segment_coverage(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"coverage_pct": null, "segment_count": 0}));
    }
    let nonempty = segs.iter().filter(|(_, nd, _)| *nd > 0).count();
    let pct = nonempty as f64 / n as f64 * 100.0;
    Json(serde_json::json!({"coverage_pct": pct, "nonempty_count": nonempty, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/imbalance — diferença entre max e min docs entre segmentos. Sprint #2268.
pub async fn segment_imbalance(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"imbalance": null, "segment_count": 0}));
    }
    let max_docs = segs.iter().map(|(_, nd, _)| *nd).max().unwrap_or(0);
    let min_docs = segs.iter().map(|(_, nd, _)| *nd).min().unwrap_or(0);
    Json(serde_json::json!({"imbalance": max_docs - min_docs, "max_docs": max_docs, "min_docs": min_docs, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/spread — range de bytes entre segmentos (max-min disk_bytes). Sprint #2273.
pub async fn segment_spread(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"spread_bytes": null, "segment_count": 0}));
    }
    let max_bytes = segs.iter().map(|(_, _, db)| *db).max().unwrap_or(0);
    let min_bytes = segs.iter().map(|(_, _, db)| *db).min().unwrap_or(0);
    Json(serde_json::json!({"spread_bytes": max_bytes - min_bytes, "max_bytes": max_bytes, "min_bytes": min_bytes, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/concentration — % de docs no maior segmento (concentração de índice). Sprint #2278.
pub async fn segment_concentration(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"concentration_pct": null, "segment_count": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let max_docs = segs.iter().map(|(_, nd, _)| *nd).max().unwrap_or(0);
    let pct = if total_docs > 0 { max_docs as f64 / total_docs as f64 * 100.0 } else { 0.0 };
    Json(serde_json::json!({"concentration_pct": pct, "max_segment_docs": max_docs, "total_docs": total_docs, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/overhead — bytes por doc em todo o índice (overhead de armazenamento). Sprint #2283.
pub async fn segment_overhead(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"bytes_per_doc": null, "segment_count": 0}));
    }
    let total_docs: u64 = segs.iter().map(|(_, nd, _)| *nd).sum();
    let total_bytes: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let bpd = if total_docs > 0 { total_bytes as f64 / total_docs as f64 } else { 0.0 };
    Json(serde_json::json!({"bytes_per_doc": bpd, "total_bytes": total_bytes, "total_docs": total_docs, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/pressure — razão bytes totais / número de segmentos (pressão por segmento). Sprint #2248.
pub async fn segment_pressure(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"pressure_bytes_per_segment": null, "segment_count": 0}));
    }
    let total_bytes: u64 = segs.iter().map(|(_, _, db)| *db).sum();
    let pressure = total_bytes as f64 / n as f64;
    Json(serde_json::json!({"pressure_bytes_per_segment": pressure, "total_bytes": total_bytes, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/bloat — bytes desperdiçados: segmentos com ratio < 0.1 (poucos docs por byte). Sprint #2253.
pub async fn segment_bloat(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let bloated: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, db)| *db > 0 && (*nd as f64 / *db as f64) < 0.1)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "ratio": *nd as f64 / *db as f64}))
        .collect();
    Json(serde_json::json!({"bloated_segments": bloated, "count": bloated.len(), "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/saturation — % de segmentos com mais de 10 000 docs (saturados). Sprint #2258.
pub async fn segment_saturation(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"saturation_pct": null, "saturated_count": 0, "segment_count": 0}));
    }
    let saturated = segs.iter().filter(|(_, nd, _)| *nd > 10_000).count();
    let pct = saturated as f64 / n as f64 * 100.0;
    Json(serde_json::json!({"saturation_pct": pct, "saturated_count": saturated, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/balance — coeficiente de variação dos docs entre segmentos (0=perfeito, >1=desequilíbrio). Sprint #2263.
pub async fn segment_balance(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"cv_docs": null, "segment_count": n}));
    }
    let docs: Vec<f64> = segs.iter().map(|(_, nd, _)| *nd as f64).collect();
    let mean = docs.iter().sum::<f64>() / n as f64;
    let variance = docs.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n as f64;
    let cv = if mean > 0.0 { variance.sqrt() / mean } else { 0.0 };
    Json(serde_json::json!({"cv_docs": cv, "mean_docs": mean, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/cold — segmentos com 0 docs (frios, sem conteúdo indexado). Sprint #2228.
pub async fn segment_cold(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let cold: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd == 0)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"cold_segments": cold, "count": cold.len(), "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/idle — segmentos com menos de 10 docs (idle, pouco uso). Sprint #2233.
pub async fn segment_idle(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let idle: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd < 10)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"idle_segments": idle, "count": idle.len(), "total_segments": segs.len()}))
}

/// GET /api/v1/search/index/segments/density — docs por byte médio de cada segmento (densidade de indexação). Sprint #2238.
pub async fn segment_density(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mean_density": null, "segment_count": 0}));
    }
    let densities: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 })
        .collect();
    let mean = densities.iter().sum::<f64>() / n as f64;
    let per_segment: Vec<serde_json::Value> = segs.iter().zip(densities.iter())
        .map(|((id, nd, db), d)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "density": d}))
        .collect();
    Json(serde_json::json!({"mean_density": mean, "segments": per_segment, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/anomaly — segmentos com ratio > mean+2σ (outliers de densidade). Sprint #2243.
pub async fn segment_anomaly(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"anomalies": [], "threshold": null, "segment_count": n}));
    }
    let ratios: Vec<f64> = segs.iter()
        .map(|(_, nd, db)| if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 })
        .collect();
    let mean = ratios.iter().sum::<f64>() / n as f64;
    let variance = ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n as f64;
    let stddev = variance.sqrt();
    let threshold = mean + 2.0 * stddev;
    let anomalies: Vec<serde_json::Value> = segs.iter().zip(ratios.iter())
        .filter(|(_, r)| **r > threshold)
        .map(|((id, nd, db), r)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "ratio": r}))
        .collect();
    Json(serde_json::json!({"anomalies": anomalies, "threshold": threshold, "mean_ratio": mean, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/lease — segmentos com ratio > 1 (mais docs que bytes: candidatos a lease/merge). Sprint #2208.
pub async fn segment_lease(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let lease: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, db)| *db > 0 && (*nd as f64 / *db as f64) > 1.0)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db, "ratio": *nd as f64 / *db as f64}))
        .collect();
    Json(serde_json::json!({"lease_candidates": lease, "count": lease.len()}))
}

/// GET /api/v1/search/index/segments/merge-candidate — segmentos abaixo de 1 000 docs (candidatos a merge). Sprint #2213.
pub async fn segment_merge_candidate(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let candidates: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, nd, _)| *nd < 1_000)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"merge_candidates": candidates, "count": candidates.len()}))
}

/// GET /api/v1/search/index/segments/defrag — segmentos com bytes acima de p90 (candidatos a defrag). Sprint #2218.
pub async fn segment_defrag(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"defrag_candidates": [], "p90_bytes": null, "segment_count": 0}));
    }
    let mut bytes_sorted: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    bytes_sorted.sort_unstable();
    let p90_idx = ((n as f64 * 0.90) as usize).min(n - 1);
    let p90 = bytes_sorted[p90_idx];
    let candidates: Vec<serde_json::Value> = segs.iter()
        .filter(|(_, _, db)| *db > p90)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"defrag_candidates": candidates, "p90_bytes": p90, "segment_count": n}))
}

/// GET /api/v1/search/index/segments/hotspot — top-3 segmentos por docs (hotspot de leitura). Sprint #2223.
pub async fn segment_hotspot(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let mut sorted = segs.clone();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    let top: Vec<serde_json::Value> = sorted.iter().take(3)
        .map(|(id, nd, db)| serde_json::json!({"id": id, "num_docs": nd, "disk_bytes": db}))
        .collect();
    Json(serde_json::json!({"hotspots": top, "segment_count": segs.len()}))
}

pub async fn segment_ratio_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_ratio": null, "segment_count": 0}));
    }
    let max = segs.iter().map(|(_, nd, db)| {
        if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 }
    }).fold(f64::MIN, f64::max);
    Json(serde_json::json!({"max_ratio": max, "segment_count": n}))
}

pub async fn segment_docs_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mean_docs": null, "segment_count": 0}));
    }
    let mean = segs.iter().map(|(_, nd, _)| *nd as f64).sum::<f64>() / n as f64;
    Json(serde_json::json!({"mean_docs": mean, "segment_count": n}))
}

pub async fn segment_bytes_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mean_bytes": null, "segment_count": 0}));
    }
    let mean = segs.iter().map(|(_, _, db)| *db as f64).sum::<f64>() / n as f64;
    Json(serde_json::json!({"mean_bytes": mean, "segment_count": n}))
}

pub async fn segment_size_count(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    Json(serde_json::json!({"segment_count": segs.len()}))
}

pub async fn segment_ratio_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mean_ratio": null, "segment_count": 0}));
    }
    let mean = segs.iter().map(|(_, nd, db)| {
        if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 }
    }).sum::<f64>() / n as f64;
    Json(serde_json::json!({"mean_ratio": mean, "segment_count": n}))
}

pub async fn segment_size_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_size": null, "segment_count": 0}));
    }
    let min = segs.iter().map(|(_, _, db)| *db).min().unwrap();
    Json(serde_json::json!({"min_size": min, "segment_count": n}))
}

pub async fn segment_ratio_min(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"min_ratio": null, "segment_count": 0}));
    }
    let min = segs.iter().map(|(_, nd, db)| {
        if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 }
    }).fold(f64::MAX, f64::min);
    Json(serde_json::json!({"min_ratio": min, "segment_count": n}))
}

pub async fn segment_size_max(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"max_size": null, "segment_count": 0}));
    }
    let max = segs.iter().map(|(_, _, db)| *db).max().unwrap();
    Json(serde_json::json!({"max_size": max, "segment_count": n}))
}

pub async fn segment_ratio_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_ratio": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| {
        if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 }
    }).collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p75 = ratios[3 * n / 4];
    Json(serde_json::json!({"p75_ratio": p75, "segment_count": n}))
}

pub async fn segment_ratio_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_ratio": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| {
        if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 }
    }).collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p90 = ratios[9 * n / 10];
    Json(serde_json::json!({"p90_ratio": p90, "segment_count": n}))
}

pub async fn segment_size_mean(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"mean_size": null, "segment_count": 0}));
    }
    let mean = segs.iter().map(|(_, _, db)| *db as f64).sum::<f64>() / n as f64;
    Json(serde_json::json!({"mean_size": mean, "segment_count": n}))
}

pub async fn segment_size_stddev(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"stddev_size": null, "segment_count": n}));
    }
    let sizes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = sizes.iter().sum::<f64>() / n as f64;
    let stddev = (sizes.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    Json(serde_json::json!({"stddev_size": stddev, "mean_size": mean, "segment_count": n}))
}

pub async fn segment_size_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_size": null, "segment_count": 0}));
    }
    let mut sizes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sizes.sort_unstable();
    let p50 = sizes[n / 2];
    Json(serde_json::json!({"p50_size": p50, "segment_count": n}))
}

pub async fn segment_size_p75(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p75_size": null, "segment_count": 0}));
    }
    let mut sizes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sizes.sort_unstable();
    let p75 = sizes[3 * n / 4];
    Json(serde_json::json!({"p75_size": p75, "segment_count": n}))
}

pub async fn segment_size_p90(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p90_size": null, "segment_count": 0}));
    }
    let mut sizes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    sizes.sort_unstable();
    let p90 = sizes[9 * n / 10];
    Json(serde_json::json!({"p90_size": p90, "segment_count": n}))
}

pub async fn segment_ratio_p50(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"p50_ratio": null, "segment_count": 0}));
    }
    let mut ratios: Vec<f64> = segs.iter().map(|(_, nd, db)| {
        if *db > 0 { *nd as f64 / *db as f64 } else { 0.0 }
    }).collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = ratios[n / 2];
    Json(serde_json::json!({"p50_ratio": p50, "segment_count": n}))
}

pub async fn segment_size_iqr(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 2 {
        return Json(serde_json::json!({"iqr_size": null, "segment_count": n}));
    }
    let mut sizes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q1 = sizes[n / 4];
    let q3 = sizes[3 * n / 4];
    Json(serde_json::json!({"iqr_size": q3 - q1, "q1_size": q1, "q3_size": q3, "segment_count": n}))
}

pub async fn segment_size_skew(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 3 {
        return Json(serde_json::json!({"skew_size": null, "segment_count": n}));
    }
    let sizes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = sizes.iter().sum::<f64>() / n as f64;
    let stddev = (sizes.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let skew = if stddev > 0.0 {
        sizes.iter().map(|s| ((s - mean) / stddev).powi(3)).sum::<f64>() / n as f64
    } else { 0.0 };
    Json(serde_json::json!({"skew_size": skew, "mean_size": mean, "stddev_size": stddev, "segment_count": n}))
}

pub async fn segment_size_kurtosis(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n < 4 {
        return Json(serde_json::json!({"kurtosis_size": null, "segment_count": n}));
    }
    let sizes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = sizes.iter().sum::<f64>() / n as f64;
    let stddev = (sizes.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let kurt = if stddev > 0.0 {
        sizes.iter().map(|s| ((s - mean) / stddev).powi(4)).sum::<f64>() / n as f64 - 3.0
    } else { 0.0 };
    Json(serde_json::json!({"kurtosis_size": kurt, "mean_size": mean, "stddev_size": stddev, "segment_count": n}))
}

pub async fn segment_size_range(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"size_range": null, "segment_count": 0}));
    }
    let sizes: Vec<u64> = segs.iter().map(|(_, _, db)| *db).collect();
    let min = *sizes.iter().min().unwrap();
    let max = *sizes.iter().max().unwrap();
    Json(serde_json::json!({"min_size": min, "max_size": max, "size_range": max.saturating_sub(min), "segment_count": n}))
}

pub async fn segment_size_cv(State(store): State<IndexStore>) -> Json<serde_json::Value> {
    let segs = store.list_segments().unwrap_or_default();
    let n = segs.len();
    if n == 0 {
        return Json(serde_json::json!({"cv_size": null, "mean_size": null, "segment_count": 0}));
    }
    let sizes: Vec<f64> = segs.iter().map(|(_, _, db)| *db as f64).collect();
    let mean = sizes.iter().sum::<f64>() / n as f64;
    let stddev = (sizes.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let cv = if mean > 0.0 { stddev / mean } else { 0.0 };
    Json(serde_json::json!({"cv_size": cv, "mean_size": mean, "stddev_size": stddev, "segment_count": n}))
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

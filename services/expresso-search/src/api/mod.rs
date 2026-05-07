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

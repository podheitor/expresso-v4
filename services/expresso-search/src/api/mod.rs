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

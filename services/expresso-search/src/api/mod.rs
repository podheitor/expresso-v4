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
    /// When set to "kind", response includes a `facets.kind` map of counts.
    pub facet: Option<String>,
}

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

/// GET /api/v1/search?q=...&tenant_id=...&limit=20&offset=0[&facet=kind]
pub async fn search(
    State(store): State<IndexStore>,
    Query(mut params): Query<SearchParams>,
) -> Result<Json<SearchResponse>, (StatusCode, String)> {
    if let Some(msg) = validate_search_params(&mut params) {
        return Err((StatusCode::BAD_REQUEST, msg));
    }
    let hits = store
        .search(&params.q, &params.tenant_id, params.limit, params.offset)
        .map_err(map_search_err)?;
    let count = hits.len();

    let facets = if params.facet.as_deref() == Some("kind") {
        let counts = store
            .facet_counts_by_kind(&params.q, &params.tenant_id)
            .map_err(map_search_err)?;
        let entries: Vec<FacetEntry> = counts.into_iter()
            .map(|(value, count)| FacetEntry { value, count })
            .collect();
        let mut map = std::collections::HashMap::new();
        map.insert("kind".to_string(), entries);
        Some(map)
    } else {
        None
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

#[cfg(test)]
mod tests {
    use super::*;

    fn p(q: &str, limit: usize) -> SearchParams {
        SearchParams {
            q:         q.to_string(),
            tenant_id: "00000000-0000-0000-0000-000000000000".into(),
            limit,
            offset:    0,
            facet:     None,
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

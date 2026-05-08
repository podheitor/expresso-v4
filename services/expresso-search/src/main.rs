//! expresso-search — full-text search service backed by Tantivy.
//!
//! Endpoints:
//!   GET  /health                     → service health (open)
//!   GET  /ready                      → readiness     (open)
//!   POST /api/v1/index               → index document       (auth)
//!   POST /api/v1/index/bulk          → bulk index (≤500)    (auth)
//!   GET  /api/v1/search?q=&tenant_id=  → search             (auth)
//!   DELETE /api/v1/index/:id         → remove document      (auth)
//!
//! Auth: quando `SEARCH_SERVICE_TOKEN` estiver no env, todos os endpoints
//! /api/v1/* exigem `Authorization: Bearer <token>` (compare em tempo
//! constante). Em dev a var pode ficar vazia — log de WARN no startup,
//! sem auth aplicada. Health/ready/metrics ficam abertos sempre (probes
//! de orquestrador + Prometheus).

mod api;
mod index_store;

use std::{env, net::SocketAddr, path::PathBuf};

use axum::{
    extract::Request,
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tracing::{info, warn};

use index_store::IndexStore;

const SERVICE: &str = "expresso-search";
const DEFAULT_PORT: u16 = 8007;

/// Constant-time byte compare. Length difference returns false sem compare
/// — tokens têm tamanho fixo conhecido pelo deployer, então length leak é
/// aceitável; o que importa é não vazar prefixo via timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

async fn require_bearer_token(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Token vem do State via extension — set abaixo no main().
    let expected = req
        .extensions()
        .get::<ServiceToken>()
        .map(|t| t.0.clone())
        .unwrap_or_default();

    let supplied = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");

    if ct_eq(expected.as_bytes(), supplied.as_bytes()) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

#[derive(Clone)]
struct ServiceToken(String);

async fn health() -> Json<Value> {
    Json(json!({"service": SERVICE, "status": "ok"}))
}

async fn ready() -> Json<Value> {
    Json(json!({"ready": true}))
}

fn resolve_addr() -> anyhow::Result<SocketAddr> {
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port = env::var("PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    Ok(format!("{}:{}", host, port).parse()?)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .json()
        .init();

    let data_dir = env::var("SEARCH_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/expresso/search"));

    let store = IndexStore::open(&data_dir)?;

    let token = env::var("SEARCH_SERVICE_TOKEN").unwrap_or_default();
    if token.is_empty() {
        warn!(
            service = SERVICE,
            "SEARCH_SERVICE_TOKEN not set — /api/v1/* endpoints exposed without auth (dev mode)"
        );
    } else {
        info!(service = SERVICE, "Bearer-token auth enabled on /api/v1/*");
    }

    let api_routes = Router::new()
        .route("/api/v1/index", post(api::index_doc).delete(api::delete_by_tenant))
        .route("/api/v1/index/bulk", post(api::bulk_index))
        .route("/api/v1/index/{id}", delete(api::delete_doc))
        .route("/api/v1/search", get(api::search))
        .route("/api/v1/search/stats", get(api::search_stats))
        .route("/api/v1/search/stats/by-tenant", get(api::search_stats_by_tenant))
        .route("/api/v1/search/index/segments",           get(api::list_segments))
        .route("/api/v1/search/index/segments/count",    get(api::segment_count))
        .route("/api/v1/search/index/segments/largest",   get(api::largest_segment))
        .route("/api/v1/search/index/segments/smallest",  get(api::smallest_segment))
        .route("/api/v1/search/index/segments/stats",     get(api::segment_stats))
        .route("/api/v1/search/index/segments/age-stats",       get(api::segment_age_stats))
        .route("/api/v1/search/index/segments/doc-distribution", get(api::segment_doc_distribution))
        .route("/api/v1/search/index/segments/top-n",    get(api::segments_top_n))
        .route("/api/v1/search/index/segments/bottom-n", get(api::segments_bottom_n))
        .route("/api/v1/search/index/segments/merge-candidates", get(api::segments_merge_candidates))
        .route("/api/v1/search/index/segments/size-stats",       get(api::segment_size_stats))
        .route("/api/v1/search/index/segments/doc-ratio",        get(api::segment_doc_ratio))
        .route("/api/v1/search/index/segments/overlap",          get(api::segments_overlap))
        .route("/api/v1/search/index/segments/cumulative",       get(api::segments_cumulative))
        .route("/api/v1/search/index/segments/percentile",       get(api::segment_percentile))
        .route("/api/v1/search/index/segments/stdev",            get(api::segment_stdev))
        .route("/api/v1/search/index/segments/entropy",          get(api::segment_entropy))
        .route("/api/v1/search/index/segments/gini",             get(api::segment_gini))
        .route("/api/v1/search/index/segments/iqr",              get(api::segment_iqr))
        .route("/api/v1/search/index/segments/range",            get(api::segment_range))
        .route("/api/v1/search/index/segments/cv",               get(api::segment_cv))
        .route("/api/v1/search/index/segments/skewness",         get(api::segment_skewness))
        .route("/api/v1/search/index/segments/mad",              get(api::segment_mad))
        .route("/api/v1/search/index/segments/kurtosis",         get(api::segment_kurtosis))
        .route("/api/v1/search/index/segments/trimmed-mean",     get(api::segment_trimmed_mean))
        .route("/api/v1/search/index/segments/harmonic-mean",    get(api::segment_harmonic_mean))
        .route("/api/v1/search/index/segments/geometric-mean",   get(api::segment_geometric_mean))
        .route("/api/v1/search/index/segments/z-scores",             get(api::segment_z_scores))
        .route("/api/v1/search/index/segments/doc-density",          get(api::segment_doc_density))
        .route("/api/v1/search/index/segments/coefficient-dispersion", get(api::segment_coefficient_dispersion))
        .route("/api/v1/search/index/segments/percentile-rank",       get(api::segment_percentile_rank))
        .route("/api/v1/search/index/segments/outliers",          get(api::segment_outliers))
        .route("/api/v1/search/index/segments/size-bands",        get(api::segment_size_bands))
        .route("/api/v1/search/index/segments/top-docs-ratio",    get(api::segment_top_docs_ratio))
        .route("/api/v1/search/index/segments/decay",             get(api::segment_decay))
        .route("/api/v1/search/index/segments/bytes-per-doc-by-segment", get(api::segment_bytes_per_doc_by_segment))
        .route("/api/v1/search/index/segments/health-score",        get(api::segment_health_score))
        .route("/api/v1/search/index/segments/write-amplification", get(api::segment_write_amplification))
        .route("/api/v1/search/index/segments/utilization",         get(api::segment_utilization))
        .route("/api/v1/search/index/segments/bottom-by-docs",          get(api::segment_bottom_by_docs))
        .route("/api/v1/search/index/segments/docs-above-median",      get(api::segment_docs_above_median))
        .route("/api/v1/search/index/segments/median-docs",            get(api::segment_median_docs))
        .route("/api/v1/search/index/segments/segment-age-rank",       get(api::segment_age_rank))
        .route("/api/v1/search/index/segments/top-by-docs",            get(api::segment_top_by_docs))
        .route("/api/v1/search/index/segments/size-percentile",       get(api::segment_size_percentile))
        .route("/api/v1/search/index/segments/docs-size-correlation", get(api::segment_docs_size_correlation))
        .route("/api/v1/search/index/segments/docs-percentile-band", get(api::segment_docs_percentile_band))
        .route("/api/v1/search/index/segments/size-spread",          get(api::segment_size_spread))
        .route("/api/v1/search/index/segments/compaction-ratio",     get(api::segment_compaction_ratio))
        .route("/api/v1/search/index/segments/docs-bytes-ratio-stdev",     get(api::segment_docs_bytes_ratio_stdev))
        .route("/api/v1/search/index/segments/bytes-per-doc-mean",        get(api::segment_bytes_per_doc_mean))
        .route("/api/v1/search/index/segments/bytes-per-doc-stdev",       get(api::segment_bytes_per_doc_stdev))
        .route("/api/v1/search/index/segments/bytes-per-doc-cv",          get(api::segment_bytes_per_doc_cv))
        .route("/api/v1/search/index/segments/bytes-above-p75",           get(api::segment_bytes_above_p75))
        .route("/api/v1/search/index/segments/bytes-median",              get(api::segment_bytes_median))
        .route("/api/v1/search/index/segments/docs-median",               get(api::segment_docs_median))
        .route("/api/v1/search/index/segments/bytes-per-doc-p75",         get(api::segment_bytes_per_doc_p75))
        .route("/api/v1/search/index/segments/total-docs",                  get(api::segment_total_docs))
        .route("/api/v1/search/index/segments/bytes-per-doc-median",       get(api::segment_bytes_per_doc_median))
        .route("/api/v1/search/index/segments/bytes-per-doc-p90",         get(api::segment_bytes_per_doc_p90))
        .route("/api/v1/search/index/segments/bytes-per-doc-p10",         get(api::segment_bytes_per_doc_p10))
        .route("/api/v1/search/index/segments/bytes-per-doc-p25",         get(api::segment_bytes_per_doc_p25))
        .route("/api/v1/search/index/segments/docs-p25",                  get(api::segment_docs_p25))
        .route("/api/v1/search/index/segments/bytes-p25",                 get(api::segment_bytes_p25))
        .route("/api/v1/search/index/segments/docs-p75",                  get(api::segment_docs_p75))
        .route("/api/v1/search/index/segments/bytes-p75",                 get(api::segment_bytes_p75))
        .route("/api/v1/search/index/segments/docs-p90",                  get(api::segment_docs_p90))
        .route("/api/v1/search/index/segments/bytes-p90",                 get(api::segment_bytes_p90))
        .route("/api/v1/search/index/segments/docs-p10",                  get(api::segment_docs_p10))
        .route("/api/v1/search/index/segments/bytes-p10",                 get(api::segment_bytes_p10))
        .route("/api/v1/search/index/segments/docs-bytes-ratio-min",      get(api::segment_docs_bytes_ratio_min))
        .route("/api/v1/search/index/segments/docs-bytes-ratio-mean",     get(api::segment_docs_bytes_ratio_mean))
        .route("/api/v1/search/index/segments/large-docs-ratio",         get(api::segment_large_docs_ratio))
        .route("/api/v1/search/index/segments/docs-bytes-ratio-max",    get(api::segment_docs_bytes_ratio_max))
        .route("/api/v1/search/index/segments/large-bytes-ratio",       get(api::segment_large_bytes_ratio))
        .route("/api/v1/search/index/segments/bottom-n-by-docs",       get(api::segment_bottom_n_by_docs))
        .route("/api/v1/search/index/segments/docs-above-p75",        get(api::segment_docs_above_p75))
        .route("/api/v1/search/index/segments/above-p75",            get(api::segment_above_p75))
        .route("/api/v1/search/index/segments/variance",             get(api::segment_variance))
        .route("/api/v1/search/index/segments/docs-floor",           get(api::segment_docs_floor))
        .route("/api/v1/search/index/segments/id-length-stats",      get(api::segment_id_length_stats))
        .route("/api/v1/search/index/segments/docs-sum",             get(api::segment_docs_sum))
        .route("/api/v1/search/index/segments/docs-density-rank",    get(api::segment_docs_density_rank))
        .route("/api/v1/search/index/segments/size-above-mean",    get(api::segment_size_above_mean))
        .route("/api/v1/search/index/segments/bytes-above-mean",   get(api::segment_bytes_above_mean))
        .route("/api/v1/search/index/segments/docs-above-mean",    get(api::segment_docs_above_mean))
        .route("/api/v1/search/index/segments/bytes-ceiling",      get(api::segment_bytes_ceiling))
        .route("/api/v1/search/index/segments/top-n-by-bytes",     get(api::segment_top_n_by_bytes))
        .route("/api/v1/search/index/segments/size-median",        get(api::segment_size_median))
        .route("/api/v1/search/index/segments/docs-median-deviation", get(api::segment_docs_median_deviation))
        .route("/api/v1/search/index/segments/bytes-floor",        get(api::segment_bytes_floor))
        .route("/api/v1/search/index/segments/docs-range",          get(api::segment_docs_range))
        .route("/api/v1/search/index/segments/count-by-size-band",  get(api::segment_count_by_size_band))
        .route("/api/v1/search/index/segments/bytes-per-doc-range", get(api::segment_bytes_per_doc_range))
        .route("/api/v1/search/index/segments/docs-stdev",          get(api::segment_docs_stdev))
        .route("/api/v1/search/index/segments/bytes-stdev",              get(api::segment_bytes_stdev))
        .route("/api/v1/search/index/segments/docs-cv",                  get(api::segment_docs_cv))
        .route("/api/v1/search/index/segments/bytes-cv",                 get(api::segment_bytes_cv))
        .route("/api/v1/search/index/segments/docs-bytes-ratio-stats",   get(api::segment_docs_bytes_ratio_stats))
        .route("/api/v1/search/index/segments/bytes-iqr",        get(api::segment_bytes_iqr))
        .route("/api/v1/search/index/segments/docs-iqr",         get(api::segment_docs_iqr))
        .route("/api/v1/search/index/segments/top-n-by-docs",    get(api::segment_top_n_by_docs))
        .route("/api/v1/search/index/segments/bottom-n-by-bytes", get(api::segment_bottom_n_by_bytes))
        .route("/api/v1/search/index/segments/total-size",      get(api::segment_total_size))
        .route("/api/v1/search/index/segments/total-bytes",             get(api::segment_total_bytes))
        .route("/api/v1/search/index/segments/avg-docs-per-segment", get(api::segment_avg_docs_per_segment))
        .route("/api/v1/search/index/segments/bytes-sum",             get(api::segment_bytes_sum))
        .route("/api/v1/search/index/segments/docs-bytes-product",    get(api::segment_docs_bytes_product))
        .route("/api/v1/search/index/segments/bytes-p99",             get(api::segment_bytes_p99))
        .route("/api/v1/search/index/segments/docs-p99",              get(api::segment_docs_p99))
        .route("/api/v1/search/index/segments/bytes-per-doc-p99",     get(api::segment_bytes_per_doc_p99))
        .route("/api/v1/search/index/segments/docs-variance",         get(api::segment_docs_variance))
        .route("/api/v1/search/index/segments/bytes-variance",         get(api::segment_bytes_variance))
        .route("/api/v1/search/index/segments/size-variance",          get(api::segment_size_variance))
        .route("/api/v1/search/index/segments/bytes-per-doc-variance", get(api::segment_bytes_per_doc_variance))
        .route("/api/v1/search/index/segments/docs-entropy",           get(api::segment_docs_entropy))
        .route("/api/v1/search/index/segments/bytes-entropy",          get(api::segment_bytes_entropy))
        .route("/api/v1/search/index/segments/docs-above-p99",          get(api::segment_docs_above_p99))
        .route("/api/v1/search/index/segments/bytes-above-p99",         get(api::segment_bytes_above_p99))
        .route("/api/v1/search/index/segments/bytes-p99-count",         get(api::segment_bytes_p99_count))
        .route("/api/v1/search/index/segments/docs-p99-count",          get(api::segment_docs_p99_count))
        .route("/api/v1/search/index/segments/docs-above-p95",          get(api::segment_docs_above_p95))
        .route("/api/v1/search/index/segments/bytes-above-p95",         get(api::segment_bytes_above_p95))
        .route("/api/v1/search/index/segments/bytes-p95-count",         get(api::segment_bytes_p95_count))
        .route("/api/v1/search/index/segments/docs-p95-count",          get(api::segment_docs_p95_count))
        .route("/api/v1/search/index/segments/count-p95-count",         get(api::segment_count_p95_count))
        .route("/api/v1/search/index/segments/bytes-per-doc-above-p95", get(api::segment_bytes_per_doc_above_p95))
        .route("/api/v1/search/index/segments/count-above-p99",         get(api::segment_count_above_p99))
        .route("/api/v1/search/index/segments/bytes-per-doc-p95-count", get(api::segment_bytes_per_doc_p95_count))
        .route("/api/v1/search/index/segments/docs-above-p99-count",    get(api::segment_docs_above_p99_count))
        .route("/api/v1/search/index/segments/bytes-above-p99-count",   get(api::segment_bytes_above_p99_count))
        .route("/api/v1/search/index/segments/bytes-per-doc-above-p99-count", get(api::segment_bytes_per_doc_above_p99_count))
        .route("/api/v1/search/index/segments/count-kurtosis",          get(api::segment_count_kurtosis))
        .route("/api/v1/search/index/segments/count-skewness",          get(api::segment_count_skewness))
        .route("/api/v1/search/index/segments/count-herfindahl",        get(api::segment_count_herfindahl))
        .route("/api/v1/search/index/segments/count-theil",             get(api::segment_count_theil))
        .route("/api/v1/search/index/segments/bytes-per-doc-theil",     get(api::segment_bytes_per_doc_theil))
        .route("/api/v1/search/index/segments/bytes-theil",             get(api::segment_bytes_theil))
        .route("/api/v1/search/index/segments/docs-theil",              get(api::segment_docs_theil))
        .route("/api/v1/search/index/segments/bytes-herfindahl",        get(api::segment_bytes_herfindahl))
        .route("/api/v1/search/index/segments/docs-herfindahl",         get(api::segment_docs_herfindahl))
        .route("/api/v1/search/index/segments/count-gini",              get(api::segment_count_gini))
        .route("/api/v1/search/index/segments/bytes-per-doc-gini",      get(api::segment_bytes_per_doc_gini))
        .route("/api/v1/search/index/segments/bytes-gini",              get(api::segment_bytes_gini))
        .route("/api/v1/search/index/segments/docs-gini",               get(api::segment_docs_gini))
        .route("/api/v1/search/index/segments/bytes-per-doc-kurtosis",  get(api::segment_bytes_per_doc_kurtosis))
        .route("/api/v1/search/index/segments/bytes-per-doc-skewness",  get(api::segment_bytes_per_doc_skewness))
        .route("/api/v1/search/index/segments/bytes-kurtosis",          get(api::segment_bytes_kurtosis))
        .route("/api/v1/search/index/segments/docs-kurtosis",           get(api::segment_docs_kurtosis))
        .route("/api/v1/search/index/segments/bytes-skewness",          get(api::segment_bytes_skewness))
        .route("/api/v1/search/index/segments/docs-skewness",           get(api::segment_docs_skewness))
        .route("/api/v1/search/index/segments/bytes-per-doc-entropy",  get(api::segment_bytes_per_doc_entropy))
        .route("/api/v1/search/index/segments/bytes-above-p90", get(api::segment_bytes_above_p90))
        .route("/api/v1/search/index/segments/docs-above-p90",       get(api::segment_docs_above_p90))
        .route("/api/v1/search/index/segments/docs-above-p90-count", get(api::segment_docs_above_p90_count))
        .route("/api/v1/search/index/segments/large-ratio",     get(api::segment_large_ratio))
        .route("/api/v1/search/index/segments/bytes-range",     get(api::segment_bytes_range))
        .route("/api/v1/search/index/segments/bytes-max",       get(api::segment_bytes_max))
        .route("/api/v1/search/index/segments/docs-max",        get(api::segment_docs_max))
        .route("/api/v1/search/index/segments/docs-min",        get(api::segment_docs_min))
        .route("/api/v1/search/index/segments/bytes-min",       get(api::segment_bytes_min))
        .route("/api/v1/search/index/segments/balance-score",      get(api::segment_balance_score))
        .route("/api/v1/search/index/segments/age-index-ratio",   get(api::segment_age_index_ratio))
        .route("/api/v1/search/index/segments/doc-index-ratio",   get(api::segment_doc_index_ratio))
        .route("/api/v1/search/index/segments/fragmentation",     get(api::segment_fragmentation))
        .route("/api/v1/search/index/segments/winsorized-mean",   get(api::segment_winsorized_mean))
        .route("/api/v1/search/index/segments/normalized-entropy", get(api::segment_normalized_entropy))
        .route("/api/v1/search/index/segments/relative-sizes",    get(api::segment_relative_sizes))
        .route("/api/v1/search/index/segments/size-ratio",        get(api::segment_size_ratio))
        .route("/api/v1/search/index/segments/count-p90-count",         get(api::segment_count_p90_count))
        .route("/api/v1/search/index/segments/docs-p75-count",          get(api::segment_docs_p75_count))
        .route("/api/v1/search/index/segments/docs-p90-count",          get(api::segment_docs_p90_count))
        .route("/api/v1/search/index/segments/bytes-p75-count",         get(api::segment_bytes_p75_count))
        .route("/api/v1/search/index/segments/bytes-above-p75-count",    get(api::segment_bytes_above_p75_count))
        .route("/api/v1/search/index/segments/count-above-p75",         get(api::segment_count_above_p75))
        .route("/api/v1/search/index/segments/count-above-p90",         get(api::segment_count_above_p90))
        .route("/api/v1/search/index/segments/count-p75-count",         get(api::segment_count_p75_count))
        .route("/api/v1/search/index/segments/bytes-above-p90-count",    get(api::segment_bytes_above_p90_count))
        .route("/api/v1/search/index/segments/docs-above-p75-count",    get(api::segment_docs_above_p75_count))
        .route("/api/v1/search/index/segments/bytes-per-doc-p90-count", get(api::segment_bytes_per_doc_p90_count))
        .route("/api/v1/search/index/segments/bytes-per-doc-p75-count", get(api::segment_bytes_per_doc_p75_count))
        .route("/api/v1/search/index/segments/count-p99-count",        get(api::segment_count_p99_count))
        .route("/api/v1/search/index/segments/bytes-per-doc-p99-count", get(api::segment_bytes_per_doc_p99_count))
        .route("/api/v1/search/index/segments/bytes-above-p95-count",  get(api::segment_bytes_above_p95_count))
        .route("/api/v1/search/index/segments/docs-above-p95-count",   get(api::segment_docs_above_p95_count))
        .route("/api/v1/search/index/segments/bytes-p90-count",        get(api::segment_bytes_p90_count))
        .route("/api/v1/search/index/segments/max-docs",               get(api::segment_max_docs))
        .route("/api/v1/search/index/segments/max-bytes",              get(api::segment_max_bytes))
        .route("/api/v1/search/index/segments/avg-bytes-per-segment",  get(api::segment_avg_bytes_per_segment))
        .route("/api/v1/search/index/segments/bytes-per-doc-min",     get(api::segment_bytes_per_doc_min))
        .route("/api/v1/search/index/segments/min-docs",              get(api::segment_min_docs))
        .route("/api/v1/search/index/segments/min-bytes",             get(api::segment_min_bytes))
        .route("/api/v1/search/index/segments/total-segment-count",   get(api::segment_total_count))
        .route("/api/v1/search/index/segments/bytes-per-doc-p50",    get(api::segment_bytes_per_doc_p50))
        .route("/api/v1/search/index/segments/docs-p50",             get(api::segment_docs_p50))
        .route("/api/v1/search/index/segments/bytes-p50",            get(api::segment_bytes_p50))
        .route("/api/v1/search/index/segments/top-segments-by-docs",    get(api::segment_top_by_docs))
        .route("/api/v1/search/index/segments/top-segments-by-bytes",   get(api::segment_top_by_bytes))
        .route("/api/v1/search/index/segments/bottom-segments-by-docs", get(api::segment_bottom_by_docs))
        .route("/api/v1/search/index/segments/bottom-segments-by-bytes",get(api::segment_bottom_by_bytes))
        .route("/api/v1/search/index/segments/ratio-above-p50",         get(api::segment_ratio_above_p50))
        .route("/api/v1/search/index/segments/ratio-above-p75",         get(api::segment_ratio_above_p75))
        .route("/api/v1/search/index/segments/ratio-above-p90",         get(api::segment_ratio_above_p90))
        .route("/api/v1/search/index/segments/ratio-above-p95",         get(api::segment_ratio_above_p95))
        .route("/api/v1/search/index/segments/above-avg-docs",          get(api::segment_above_avg_docs))
        .route("/api/v1/search/index/segments/above-avg-bytes",         get(api::segment_above_avg_bytes))
        .route("/api/v1/search/index/segments/above-avg-ratio",         get(api::segment_above_avg_ratio))
        .route("/api/v1/search/index/segments/count-above-avg",         get(api::segment_count_above_avg))
        .route("/api/v1/search/index/segments/docs-below-avg",          get(api::segment_docs_below_avg))
        .route("/api/v1/search/index/segments/bytes-below-avg",         get(api::segment_bytes_below_avg))
        .route("/api/v1/search/index/segments/ratio-below-avg",         get(api::segment_ratio_below_avg))
        .route("/api/v1/search/index/segments/docs-below-p50",          get(api::segment_docs_below_p50))
        .route("/api/v1/search/index/segments/bytes-below-p50",         get(api::segment_bytes_below_p50))
        .route("/api/v1/search/index/segments/ratio-below-p50",         get(api::segment_ratio_below_p50))
        .route("/api/v1/search/index/segments/docs-below-p75",          get(api::segment_docs_below_p75))
        .route("/api/v1/search/index/segments/bytes-below-p75",         get(api::segment_bytes_below_p75))
        .route("/api/v1/search/index/segments/ratio-below-p75",         get(api::segment_ratio_below_p75))
        .route("/api/v1/search/index/segments/docs-below-p90",          get(api::segment_docs_below_p90))
        .route("/api/v1/search/index/segments/bytes-below-p90",         get(api::segment_bytes_below_p90))
        .route("/api/v1/search/index/segments/ratio-below-p90",         get(api::segment_ratio_below_p90))
        .route("/api/v1/search/index/segments/docs-below-p95",          get(api::segment_docs_below_p95))
        .route("/api/v1/search/index/segments/bytes-below-p95",         get(api::segment_bytes_below_p95))
        .route("/api/v1/search/index/segments/ratio-below-p95",         get(api::segment_ratio_below_p95))
        .route("/api/v1/search/index/segments/docs-below-p99",          get(api::segment_docs_below_p99))
        .route("/api/v1/search/index/segments/bytes-below-p99",         get(api::segment_bytes_below_p99))
        .route("/api/v1/search/index/segments/ratio-below-p99",         get(api::segment_ratio_below_p99))
        .route("/api/v1/search/index/segments/size-below-p10",          get(api::segment_size_below_p10))
        .route("/api/v1/search/index/segments/docs-above-p10",          get(api::segment_docs_above_p10))
        .route("/api/v1/search/index/segments/bytes-above-p10",         get(api::segment_bytes_above_p10))
        .route("/api/v1/search/index/segments/ratio-below-p10",         get(api::segment_ratio_below_p10))
        .route("/api/v1/search/index/segments/ratio-below-p25",         get(api::segment_ratio_below_p25))
        .route("/api/v1/search/index/segments/docs-below-p10",          get(api::segment_docs_below_p10))
        .route("/api/v1/search/index/segments/bytes-below-p10",         get(api::segment_bytes_below_p10))
        .route("/api/v1/search/index/segments/size-above-p10",          get(api::segment_size_above_p10))
        .route("/api/v1/search/index/segments/size-above-p25",          get(api::segment_size_above_p25))
        .route("/api/v1/search/index/segments/size-above-p50",          get(api::segment_size_above_p50))
        .route("/api/v1/search/index/segments/docs-above-p25",          get(api::segment_docs_above_p25))
        .route("/api/v1/search/index/segments/size-above-p75",          get(api::segment_size_above_p75))
        .route("/api/v1/search/index/segments/size-above-p90",          get(api::segment_size_above_p90))
        .route("/api/v1/search/index/segments/docs-above-p50",          get(api::segment_docs_above_p50))
        .route("/api/v1/search/index/segments/size-above-p95",          get(api::segment_size_above_p95))
        .route("/api/v1/search/index/segments/size-above-p99",          get(api::segment_size_above_p99))
        .route("/api/v1/search/index/segments/ratio-above-p10",         get(api::segment_ratio_above_p10))
        .route("/api/v1/search/index/segments/ratio-above-p25",         get(api::segment_ratio_above_p25))
        .route("/api/v1/search/index/segments/docs-above-p99-ratio",    get(api::segment_docs_above_p99_ratio))
        .route("/api/v1/search/index/segments/ratio-above-p99",         get(api::segment_ratio_above_p99))
        .route("/api/v1/search/index/segments/docs-above-p50-ratio",    get(api::segment_docs_above_p50_ratio))
        .route("/api/v1/search/index/segments/docs-above-p25-ratio",    get(api::segment_docs_above_p25_ratio))
        .route("/api/v1/search/index/segments/size-below-p25",          get(api::segment_size_below_p25))
        .route("/api/v1/search/index/segments/size-below-p50",          get(api::segment_size_below_p50))
        .route("/api/v1/search/index/segments/size-below-p75",          get(api::segment_size_below_p75))
        .route("/api/v1/search/index/segments/size-below-p90",          get(api::segment_size_below_p90))
        .route("/api/v1/search/index/segments/size-below-p95",          get(api::segment_size_below_p95))
        .route("/api/v1/search/index/segments/bytes-above-p25",         get(api::segment_bytes_above_p25))
        .route("/api/v1/search/index/segments/bytes-above-p50",         get(api::segment_bytes_above_p50))
        .route("/api/v1/search/index/segments/docs-below-p25",          get(api::segment_docs_below_p25))
        .route("/api/v1/search/index/segments/bytes-below-p25",         get(api::segment_bytes_below_p25))
        .route("/api/v1/search/index/segments/count-below-p25",         get(api::segment_count_below_p25))
        .route("/api/v1/search/index/segments/count-below-p50",         get(api::segment_count_below_p50))
        .route("/api/v1/search/index/segments/count-below-p75",         get(api::segment_count_below_p75))
        .route("/api/v1/search/index/segments/count-below-p90",         get(api::segment_count_below_p90))
        .route("/api/v1/search/index/segments/count-below-p95",         get(api::segment_count_below_p95))
        .route("/api/v1/search/index/segments/count-below-p99",         get(api::segment_count_below_p99))
        .route("/api/v1/search/index/segments/count-above-p25",         get(api::segment_count_above_p25))
        .route("/api/v1/search/index/segments/count-above-p50",         get(api::segment_count_above_p50))
        .route("/api/v1/search/index/segments/count-above-p95",         get(api::segment_count_above_p95))
        .route("/api/v1/search/index/segments/size-below-p99",          get(api::segment_size_below_p99))
        .route("/api/v1/search/index/segments/bytes-per-doc-below-p25", get(api::segment_bytes_per_doc_below_p25))
        .route("/api/v1/search/index/segments/bytes-per-doc-below-p50", get(api::segment_bytes_per_doc_below_p50))
        .route("/api/v1/search/index/segments/bytes-per-doc-below-p75", get(api::segment_bytes_per_doc_below_p75))
        .route("/api/v1/search/index/segments/bytes-per-doc-below-p90", get(api::segment_bytes_per_doc_below_p90))
        .route("/api/v1/search/index/segments/bytes-per-doc-below-p95", get(api::segment_bytes_per_doc_below_p95))
        .route("/api/v1/search/index/segments/bytes-per-doc-below-p99", get(api::segment_bytes_per_doc_below_p99))
        .route("/api/v1/search/index/segments/bytes-per-doc-above-p75", get(api::segment_bytes_per_doc_above_p75))
        .route("/api/v1/search/index/segments/bytes-per-doc-above-p90", get(api::segment_bytes_per_doc_above_p90))
        .route("/api/v1/search/index/segments/bytes-per-doc-above-p25", get(api::segment_bytes_per_doc_above_p25))
        .route("/api/v1/search/index/segments/bytes-per-doc-above-p50", get(api::segment_bytes_per_doc_above_p50))
        .route("/api/v1/search/index/segments/empty-count",             get(api::segment_empty_count))
        .route("/api/v1/search/index/segments/singleton-count",         get(api::segment_singleton_count))
        .route("/api/v1/search/index/segments/large-count",             get(api::segment_large_count))
        .route("/api/v1/search/index/segments/small-count",             get(api::segment_small_count))
        .route("/api/v1/search/index/segments/medium-count",            get(api::segment_medium_count))
        .route("/api/v1/search/index/segments/docs-stddev",             get(api::segment_docs_stddev))
        .route("/api/v1/search/index/segments/bytes-stddev",            get(api::segment_bytes_stddev))
        .route("/api/v1/search/index/segments/ratio-stddev",            get(api::segment_ratio_stddev))
        .route("/api/v1/search/index/segments/ratio-cv",                get(api::segment_ratio_cv))
        .route("/api/v1/search/index/segments/ratio-iqr",               get(api::segment_ratio_iqr))
        .route("/api/v1/search/index/segments/ratio-range",             get(api::segment_ratio_range))
        .route("/api/v1/search/index/segments/ratio-skew",              get(api::segment_ratio_skew))
        .route("/api/v1/search/index/segments/docs-skew",               get(api::segment_docs_skew))
        .route("/api/v1/search/index/segments/bytes-skew",              get(api::segment_bytes_skew))
        .route("/api/v1/search/index/segments/ratio-kurtosis",          get(api::segment_ratio_kurtosis))
        .route("/api/v1/search/index/segments/count-below-avg",         get(api::segment_count_below_avg))
        .route("/api/v1/search/index/segments/density-rank",            get(api::segment_density_rank))
        .route("/api/v1/search/index/segments/efficiency",              get(api::segment_efficiency))
        .route("/api/v1/search/index/segments/compaction-score",        get(api::segment_compaction_score))
        .route("/api/v1/search/index/segments/top-heavy",               get(api::segment_top_heavy))
        .route("/api/v1/search/index/segments/largest",                  get(api::segment_largest))
        .route("/api/v1/search/index/segments/smallest",                 get(api::segment_smallest))
        .route("/api/v1/search/index/segments/median-bytes",             get(api::segment_median_bytes))
        .route("/api/v1/search/index/segments/age",                      get(api::segment_age))
        .route("/api/v1/search/index/segments/churn",                    get(api::segment_churn))
        .route("/api/v1/search/index/segments/waste",                    get(api::segment_waste))
        .route("/api/v1/search/index/segments/throughput",               get(api::segment_throughput))
        .route("/api/v1/search/index/segments/fragility",                get(api::segment_fragility))
        .route("/api/v1/search/index/segments/footprint",                get(api::segment_footprint))
        .route("/api/v1/search/index/segments/load",                     get(api::segment_load))
        .route("/api/v1/search/index/segments/coverage",                 get(api::segment_coverage))
        .route("/api/v1/search/index/segments/imbalance",                get(api::segment_imbalance))
        .route("/api/v1/search/index/segments/spread",                   get(api::segment_spread))
        .route("/api/v1/search/index/segments/concentration",            get(api::segment_concentration))
        .route("/api/v1/search/index/segments/overhead",                 get(api::segment_overhead))
        .route("/api/v1/search/index/segments/pressure",                 get(api::segment_pressure))
        .route("/api/v1/search/index/segments/bloat",                    get(api::segment_bloat))
        .route("/api/v1/search/index/segments/saturation",               get(api::segment_saturation))
        .route("/api/v1/search/index/segments/balance",                  get(api::segment_balance))
        .route("/api/v1/search/index/segments/cold",                     get(api::segment_cold))
        .route("/api/v1/search/index/segments/idle",                     get(api::segment_idle))
        .route("/api/v1/search/index/segments/density",                  get(api::segment_density))
        .route("/api/v1/search/index/segments/anomaly",                  get(api::segment_anomaly))
        .route("/api/v1/search/index/segments/lease",                    get(api::segment_lease))
        .route("/api/v1/search/index/segments/merge-candidate",          get(api::segment_merge_candidate))
        .route("/api/v1/search/index/segments/defrag",                   get(api::segment_defrag))
        .route("/api/v1/search/index/segments/hotspot",                  get(api::segment_hotspot))
        .route("/api/v1/search/index/segments/ratio-max",                get(api::segment_ratio_max))
        .route("/api/v1/search/index/segments/docs-mean",                get(api::segment_docs_mean))
        .route("/api/v1/search/index/segments/bytes-mean",               get(api::segment_bytes_mean))
        .route("/api/v1/search/index/segments/count",                    get(api::segment_size_count))
        .route("/api/v1/search/index/segments/ratio-mean",               get(api::segment_ratio_mean))
        .route("/api/v1/search/index/segments/size-min",                 get(api::segment_size_min))
        .route("/api/v1/search/index/segments/ratio-min",                get(api::segment_ratio_min))
        .route("/api/v1/search/index/segments/size-max",                 get(api::segment_size_max))
        .route("/api/v1/search/index/segments/ratio-p75",                get(api::segment_ratio_p75))
        .route("/api/v1/search/index/segments/ratio-p90",                get(api::segment_ratio_p90))
        .route("/api/v1/search/index/segments/size-mean",                get(api::segment_size_mean))
        .route("/api/v1/search/index/segments/size-stddev",              get(api::segment_size_stddev))
        .route("/api/v1/search/index/segments/size-p50",                 get(api::segment_size_p50))
        .route("/api/v1/search/index/segments/size-p75",                 get(api::segment_size_p75))
        .route("/api/v1/search/index/segments/size-p90",                 get(api::segment_size_p90))
        .route("/api/v1/search/index/segments/ratio-p50",                get(api::segment_ratio_p50))
        .route("/api/v1/search/index/segments/size-iqr",                 get(api::segment_size_iqr))
        .route("/api/v1/search/index/segments/size-skew",                get(api::segment_size_skew))
        .route("/api/v1/search/index/segments/size-kurtosis",            get(api::segment_size_kurtosis))
        .route("/api/v1/search/index/segments/size-range",               get(api::segment_size_range))
        .route("/api/v1/search/index/segments/size-cv",                 get(api::segment_size_cv))
        .route("/api/v1/search/index/segments/reload",   post(api::reload_index))
        .route("/api/v1/search/index/segments/merge",    post(api::merge_segments))
        .route("/api/v1/search/index/segments/{id}",     get(api::get_segment))
        .route("/api/v1/search/index/vacuum",             post(api::vacuum_index))
        .route("/api/v1/search/index/tenant/{tenant_id}", delete(api::purge_tenant_index))
        .route("/api/v1/search/health/index",             get(api::index_health))
        .route("/api/v1/search/health/index/detailed",    get(api::index_health_detailed))
        .route("/api/v1/search/index/disk-usage",          get(api::index_disk_usage))
        .route("/api/v1/search/index/writer/stats",       get(api::writer_stats))
        .with_state(store);

    let api_routes = if !token.is_empty() {
        let tok = ServiceToken(token);
        api_routes.layer(axum::Extension(tok))
            .layer(middleware::from_fn(require_bearer_token))
    } else {
        api_routes
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .merge(api_routes)
        .merge(expresso_observability::metrics_router());

    let addr = resolve_addr()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!(service = SERVICE, %addr, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}

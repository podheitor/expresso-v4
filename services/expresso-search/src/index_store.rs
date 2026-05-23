//! Tantivy index management — shared state for the search service.
//! Schema: document_id (stored), tenant_id (indexed), subject (full-text+stored),
//! from_addr (stored+indexed), body (full-text+stored), kind (string+stored),
//! received_at (u64 fast-field, unix timestamp in seconds, STORED).
//!
//! NOTE: body was changed from TEXT to TEXT|STORED. Existing indexes built
//! before this change need re-indexing to populate stored body values; until
//! then snippet will be None for old documents.

use std::path::Path;
use tantivy::schema::Value as TantivyValue;
use std::sync::Arc;

use tantivy::{
    collector::TopDocs,
    directory::{Directory, MmapDirectory},
    doc,
    query::{BooleanQuery, Occur, Query, QueryParser, RangeQuery, TermQuery},
    schema::{
        Field, IndexRecordOption, NumericOptions, Schema, STORED, STRING, TEXT,
    },
    snippet::SnippetGenerator,
    HasLen, Index, IndexReader, IndexWriter, ReloadPolicy, SegmentId,
};
use tokio::sync::Mutex;
use tracing::info;
use uuid::Uuid;

#[derive(Clone)]
pub struct IndexStore {
    inner: Arc<Inner>,
}

struct Inner {
    index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    // Schema fields
    pub f_doc_id: Field,
    pub f_tenant_id: Field,
    pub f_subject: Field,
    pub f_from_addr: Field,
    pub f_body: Field,
    pub f_kind: Field,
    pub f_received_at: Field,
}

/// Document to be indexed
#[derive(Debug, serde::Deserialize)]
pub struct IndexDoc {
    pub document_id: String,
    pub tenant_id: String,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    pub body: Option<String>,
    /// Categorization (e.g. "mail", "drive", "contact"). Used for faceted search.
    pub kind: Option<String>,
    /// Unix timestamp in seconds (UTC). Used for temporal facets and range queries.
    /// Absent/null → stored as 0 (epoch), excluded from temporal range filters.
    pub received_at: Option<u64>,
}

/// Search result item
#[derive(Debug, serde::Serialize)]
pub struct SearchHit {
    pub document_id: String,
    pub score: f32,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    /// Excerpt (~200 chars) from the body around the matched terms. None when
    /// the document was indexed before body became STORED (needs re-index).
    pub snippet: Option<String>,
    pub kind: Option<String>,
    /// Unix timestamp in seconds. None when document has no received_at (stored as 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_at: Option<u64>,
}

impl IndexStore {
    /// Open or create index at given directory.
    pub fn open(data_dir: &Path) -> anyhow::Result<Self> {
        let mut schema_builder = Schema::builder();
        let f_doc_id = schema_builder.add_text_field("document_id", STRING | STORED);
        let f_tenant_id = schema_builder.add_text_field("tenant_id", STRING | STORED);
        let f_subject = schema_builder.add_text_field("subject", TEXT | STORED);
        let f_from_addr = schema_builder.add_text_field("from_addr", TEXT | STORED);
        let f_body = schema_builder.add_text_field("body", TEXT | STORED);
        let f_kind = schema_builder.add_text_field("kind", STRING | STORED);
        // received_at: fast-field (column store) + stored so it can be returned in hits.
        // u64 unix timestamp in seconds. 0 = absent/unknown (excluded from range filters).
        let f_received_at = schema_builder.add_u64_field(
            "received_at",
            NumericOptions::default().set_fast().set_stored(),
        );
        let schema = schema_builder.build();

        std::fs::create_dir_all(data_dir)?;
        let dir = MmapDirectory::open(data_dir)?;
        let index = Index::open_or_create(dir, schema)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        let writer = index.writer(50_000_000)?; // 50 MB heap

        info!(path = %data_dir.display(), "Tantivy index opened");

        Ok(Self {
            inner: Arc::new(Inner {
                index,
                reader,
                writer: Mutex::new(writer),
                f_doc_id,
                f_tenant_id,
                f_subject,
                f_from_addr,
                f_body,
                f_kind,
                f_received_at,
            }),
        })
    }

    /// Add or update a document in the index.
    ///
    /// `tenant_id` precisa ser UUID — sem validação, callers podiam indexar
    /// com tenant_id vazio ou wildcard (e.g. `*`) que casaria buscas alheias
    /// se um leitor não escapasse igualmente. Normalizamos para a forma
    /// canônica do UUID antes de gravar.
    pub async fn index_document(&self, doc_data: &IndexDoc) -> anyhow::Result<()> {
        let tenant_uuid = Uuid::parse_str(doc_data.tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        if doc_data.document_id.trim().is_empty() {
            anyhow::bail!("document_id must not be empty");
        }

        let i = &self.inner;
        let mut writer = i.writer.lock().await;

        // Delete existing doc with same id (upsert)
        let term = tantivy::Term::from_field_text(i.f_doc_id, &doc_data.document_id);
        writer.delete_term(term);

        writer.add_document(doc!(
            i.f_doc_id      => doc_data.document_id.as_str(),
            i.f_tenant_id   => tenant_canonical.as_str(),
            i.f_subject     => doc_data.subject.as_deref().unwrap_or(""),
            i.f_from_addr   => doc_data.from_addr.as_deref().unwrap_or(""),
            i.f_body        => doc_data.body.as_deref().unwrap_or(""),
            i.f_kind        => doc_data.kind.as_deref().unwrap_or(""),
            i.f_received_at => doc_data.received_at.unwrap_or(0u64),
        ))?;

        writer.commit()?;
        Ok(())
    }

    /// Add or update multiple documents in a single writer lock + commit.
    ///
    /// Validation failures on individual docs are collected and returned as an
    /// error listing which document_ids were rejected; successfully validated
    /// docs are still indexed and committed.
    pub async fn index_documents(&self, docs: &[IndexDoc]) -> anyhow::Result<Vec<String>> {
        let i = &self.inner;
        let mut writer = i.writer.lock().await;
        let mut rejected: Vec<String> = Vec::new();

        for doc_data in docs {
            let tenant_uuid = match Uuid::parse_str(doc_data.tenant_id.trim()) {
                Ok(u) => u,
                Err(_) => { rejected.push(doc_data.document_id.clone()); continue; }
            };
            if doc_data.document_id.trim().is_empty() {
                rejected.push(doc_data.document_id.clone());
                continue;
            }
            let tenant_canonical = tenant_uuid.to_string();
            let term = tantivy::Term::from_field_text(i.f_doc_id, &doc_data.document_id);
            writer.delete_term(term);
            writer.add_document(doc!(
                i.f_doc_id      => doc_data.document_id.as_str(),
                i.f_tenant_id   => tenant_canonical.as_str(),
                i.f_subject     => doc_data.subject.as_deref().unwrap_or(""),
                i.f_from_addr   => doc_data.from_addr.as_deref().unwrap_or(""),
                i.f_body        => doc_data.body.as_deref().unwrap_or(""),
                i.f_kind        => doc_data.kind.as_deref().unwrap_or(""),
                i.f_received_at => doc_data.received_at.unwrap_or(0u64),
            ))?;
        }

        writer.commit()?;
        Ok(rejected)
    }

    /// Remove a document by id.
    pub async fn delete_document(&self, document_id: &str) -> anyhow::Result<()> {
        let i = &self.inner;
        let mut writer = i.writer.lock().await;
        let term = tantivy::Term::from_field_text(i.f_doc_id, document_id);
        writer.delete_term(term);
        writer.commit()?;
        Ok(())
    }

    /// Remove all documents belonging to a tenant.
    pub async fn delete_tenant_documents(&self, tenant_id: &str) -> anyhow::Result<()> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let i = &self.inner;
        let mut writer = i.writer.lock().await;
        let term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        writer.delete_term(term);
        writer.commit()?;
        Ok(())
    }

    /// Force reader reload — exposed as POST /segments/reload.
    pub fn reload(&self) -> anyhow::Result<()> {
        self.inner.reader.reload()?;
        Ok(())
    }

    /// Returns writer-level stats observable without blocking on the writer lock.
    ///
    /// - `heap_budget_bytes`: static allocation given to the writer at open (50 MB).
    /// - `writer_busy`: true if another task currently holds the writer mutex (try_lock).
    /// - `num_docs_committed`: total live docs visible to the current reader snapshot.
    /// - `num_segments_committed`: segment count in the current reader snapshot.
    pub fn writer_stats(&self) -> (u64, bool, u64, usize) {
        let i = &self.inner;
        let writer_busy = i.writer.try_lock().is_err();
        let searcher = i.reader.searcher();
        let num_docs: u64 = searcher.segment_readers().iter()
            .map(|sr| sr.num_docs() as u64).sum();
        let num_segments = searcher.segment_readers().len();
        (50_000_000u64, writer_busy, num_docs, num_segments)
    }

    /// Full-text search filtered by tenant.
    pub fn search(&self, query_str: &str, tenant_id: &str, limit: usize, offset: usize) -> anyhow::Result<Vec<SearchHit>> {
        // Validação rígida: tenant_id precisa ser UUID. A versão antiga
        // injetava o valor cru no QueryParser via `format!`, escapando só
        // aspas — um tenant_id como `*` ou vazio puxaria docs de outros
        // tenants. Forçamos parse pra UUID e usamos termo direto.
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        // Defesa em profundidade: bloqueia tentativa do usuário sobrescrever
        // o filtro de tenant via sintaxe `tenant_id:...` no query string.
        // Mesmo que a BooleanQuery abaixo mantenha o Must do tenant correto,
        // queries com `tenant_id:` confundem o parser e podem expor termos
        // armazenados; melhor rejeitar de cara.
        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                // Tag como bad_query para que o handler retorne 400 sem vazar detalhes.
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        // Keep user_query separate so we can build a SnippetGenerator from it.
        let (final_query, user_query_for_snippet): (Box<dyn Query>, Option<Box<dyn Query>>) = if trimmed.is_empty() {
            (tenant_query, None)
        } else {
            let parser = QueryParser::for_index(
                &i.index,
                vec![i.f_subject, i.f_body, i.f_from_addr],
            );
            // Tag QueryParserError como bad_query — erros de sintaxe são input
            // do usuário (400), não falha interna (500); sem tag, o handler
            // vaza nomes de campos do schema via e.to_string().
            let user_query = parser
                .parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            // Clone via Box<dyn Query> is not available; re-parse for snippet generator.
            let user_query2 = parser
                .parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            let combined = Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]));
            (combined, Some(user_query2))
        };

        // Build snippet generator once (expensive) outside the per-doc loop.
        let snippet_gen = user_query_for_snippet.as_ref().and_then(|q| {
            SnippetGenerator::create(&searcher, q.as_ref(), i.f_body).ok()
        }).map(|mut gen| { gen.set_max_num_chars(200); gen });

        let top_docs = searcher.search(&*final_query, &TopDocs::with_limit(limit).and_offset(offset))?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_addr) in top_docs {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let doc_id = match doc.get_first(i.f_doc_id).and_then(|v| TantivyValue::as_str(&v)) {
                Some(id) => id.to_owned(),
                None => continue,
            };
            let subject   = doc.get_first(i.f_subject).and_then(|v| TantivyValue::as_str(&v)).map(str::to_owned);
            let from_addr = doc.get_first(i.f_from_addr).and_then(|v| TantivyValue::as_str(&v)).map(str::to_owned);
            let kind      = doc.get_first(i.f_kind).and_then(|v| TantivyValue::as_str(&v))
                               .filter(|s| !s.is_empty()).map(str::to_owned);
            let received_at = doc.get_first(i.f_received_at)
                .and_then(|v| TantivyValue::as_u64(&v))
                .filter(|&ts| ts > 0);
            let snippet = snippet_gen.as_ref().map(|gen| {
                let s = gen.snippet_from_doc(&doc);
                let text = s.to_html();
                // Strip tantivy's <b>…</b> highlight tags — return plain text.
                text.replace("<b>", "").replace("</b>", "")
            }).filter(|s| !s.is_empty());
            results.push(SearchHit { document_id: doc_id, score, subject, from_addr, snippet, kind, received_at });
        }

        Ok(results)
    }

    /// Aggregate hit counts by `kind` field for a tenant+query combination.
    /// Returns a sorted map of kind → count.
    pub fn facet_counts_by_kind(
        &self,
        query_str: &str,
        tenant_id: &str,
    ) -> anyhow::Result<Vec<(String, u64)>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let final_query: Box<dyn Query> = if trimmed.is_empty() {
            tenant_query
        } else {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]))
        };

        // Collect all matching doc addresses.
        use tantivy::collector::DocSetCollector;
        let doc_set = searcher.search(&*final_query, &DocSetCollector)?;

        let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let kind = doc.get_first(i.f_kind)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown")
                .to_owned();
            *counts.entry(kind).or_insert(0) += 1;
        }

        let mut sorted: Vec<(String, u64)> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(sorted)
    }

    /// Aggregate top tokens from `subject` field across the hit set
    /// (sprint #431). Tokeniza por whitespace + lowercase, filtra tokens curtos
    /// (< 3 chars) e stop-words PT/EN comuns. Retorna até `top_n` tokens
    /// ordenados por count desc, then alfabético. Útil pra "tag cloud" de
    /// palavras-chave do conjunto filtrado.
    pub fn facet_top_subject_terms(
        &self,
        query_str: &str,
        tenant_id: &str,
        top_n: usize,
    ) -> anyhow::Result<Vec<(String, u64)>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let final_query: Box<dyn Query> = if trimmed.is_empty() {
            tenant_query
        } else {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]))
        };

        use tantivy::collector::DocSetCollector;
        let doc_set = searcher.search(&*final_query, &DocSetCollector)?;

        // Stop-words mínimas PT+EN. Ampliar se viramos múltiplos idiomas reais.
        const STOP: &[&str] = &[
            "the", "and", "for", "you", "from", "with", "this", "that", "are",
            "was", "but", "not", "your", "have", "has", "all",
            "que", "com", "para", "sem", "por", "uma", "dos", "das",
            "como", "mas", "seu", "sua", "está", "ser", "fwd", "re",
        ];

        let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let subject = match doc.get_first(i.f_subject).and_then(|v| TantivyValue::as_str(&v)) {
                Some(s) => s,
                None    => continue,
            };
            for raw in subject.split(|c: char| !c.is_alphanumeric()) {
                let t = raw.to_lowercase();
                if t.chars().count() < 3 { continue; }
                if STOP.contains(&t.as_str()) { continue; }
                *counts.entry(t).or_insert(0) += 1;
            }
        }

        let mut sorted: Vec<(String, u64)> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        sorted.truncate(top_n);
        Ok(sorted)
    }

    /// Aggregate hit counts by `from_addr` field for a tenant+query combination
    /// (sprint #426). Top remetentes para o conjunto de hits. Retorna até `top_n`
    /// entradas ordenadas por count desc, then alfabético.
    /// Cross-facet kind×from_addr: pra cada (kind, from_addr) presente no hit-set,
    /// conta ocorrências e retorna top-N pares ordenados por count desc + alfabético.
    /// Útil pra "top remetentes por categoria" sem rodar dois facets separados
    /// e cruzar no cliente. Chave externa = "kind|from_addr"; cliente faz split.
    pub fn facet_kind_by_from(
        &self,
        query_str: &str,
        tenant_id: &str,
        top_n: usize,
    ) -> anyhow::Result<Vec<(String, String, u64)>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let final_query: Box<dyn Query> = if trimmed.is_empty() {
            tenant_query
        } else {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]))
        };

        use tantivy::collector::DocSetCollector;
        let doc_set = searcher.search(&*final_query, &DocSetCollector)?;

        let mut counts: std::collections::HashMap<(String, String), u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let kind = doc.get_first(i.f_kind)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .unwrap_or("(unknown)")
                .to_owned();
            let from = doc.get_first(i.f_from_addr)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .unwrap_or("(unknown)")
                .to_owned();
            *counts.entry((kind, from)).or_insert(0) += 1;
        }

        let mut sorted: Vec<(String, String, u64)> = counts.into_iter()
            .map(|((k, f), c)| (k, f, c))
            .collect();
        sorted.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then(a.0.cmp(&b.0))
                .then(a.1.cmp(&b.1))
        });
        sorted.truncate(top_n);
        Ok(sorted)
    }

    /// Cross-facet domain×kind: pra cada (domain, kind) presente no hit-set,
    /// conta ocorrências e retorna top-N pares. Domínio = parte after-@ do
    /// from_addr (lowercase). Útil pra "quais categorias cada domínio manda".
    /// Chave externa = "domain|kind"; cliente faz split.
    pub fn facet_domain_by_kind(
        &self,
        query_str: &str,
        tenant_id: &str,
        top_n: usize,
    ) -> anyhow::Result<Vec<(String, String, u64)>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let final_query: Box<dyn Query> = if trimmed.is_empty() {
            tenant_query
        } else {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]))
        };

        use tantivy::collector::DocSetCollector;
        let doc_set = searcher.search(&*final_query, &DocSetCollector)?;

        let mut counts: std::collections::HashMap<(String, String), u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let domain = doc.get_first(i.f_from_addr)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .and_then(|s| s.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()))
                .unwrap_or_else(|| "(unknown)".to_owned());
            let kind = doc.get_first(i.f_kind)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .unwrap_or("(unknown)")
                .to_owned();
            *counts.entry((domain, kind)).or_insert(0) += 1;
        }

        let mut sorted: Vec<(String, String, u64)> = counts.into_iter()
            .map(|((d, k), c)| (d, k, c))
            .collect();
        sorted.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then(a.0.cmp(&b.0))
                .then(a.1.cmp(&b.1))
        });
        sorted.truncate(top_n);
        Ok(sorted)
    }

    /// Aggrega hits por domínio do from_addr (parte after-@). Útil pra dashboards
    /// "top domínios remetentes" quando o universo de remetentes individuais é
    /// grande demais pra listar. Endereços sem '@' caem em "(unknown)".
    pub fn facet_counts_by_domain(
        &self,
        query_str: &str,
        tenant_id: &str,
        top_n: usize,
    ) -> anyhow::Result<Vec<(String, u64)>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let final_query: Box<dyn Query> = if trimmed.is_empty() {
            tenant_query
        } else {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]))
        };

        use tantivy::collector::DocSetCollector;
        let doc_set = searcher.search(&*final_query, &DocSetCollector)?;

        let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let domain = doc.get_first(i.f_from_addr)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .and_then(|s| s.rsplit_once('@').map(|(_, d)| d.to_ascii_lowercase()))
                .unwrap_or_else(|| "(unknown)".to_owned());
            *counts.entry(domain).or_insert(0) += 1;
        }

        let mut sorted: Vec<(String, u64)> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        sorted.truncate(top_n);
        Ok(sorted)
    }

    /// Temporal facet: counts hits bucketed by `received_at` using the requested
    /// granularity ("day" → YYYY-MM-DD, "week" → YYYY-Www, "month" → YYYY-MM).
    /// Documents with `received_at == 0` (absent) are excluded from buckets.
    /// Returns Vec<(bucket_label, count)> sorted by label ascending (chronological).
    pub fn facet_received_at_buckets(
        &self,
        query_str: &str,
        tenant_id: &str,
        granularity: &str,
    ) -> anyhow::Result<Vec<(String, u64)>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let final_query: Box<dyn Query> = if trimmed.is_empty() {
            tenant_query
        } else {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]))
        };

        use tantivy::collector::DocSetCollector;
        let doc_set = searcher.search(&*final_query, &DocSetCollector)?;

        let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let ts = match doc.get_first(i.f_received_at).and_then(|v| TantivyValue::as_u64(&v)) {
                Some(t) if t > 0 => t,
                _ => continue,
            };
            // Convert unix seconds to NaiveDate via integer arithmetic (no external dep).
            // Using Gregorian proleptic calendar: days since 1970-01-01.
            let bucket = unix_secs_to_bucket(ts, granularity);
            *counts.entry(bucket).or_insert(0) += 1;
        }

        let mut sorted: Vec<(String, u64)> = counts.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0)); // chronological by label
        Ok(sorted)
    }

    /// Range filter on `received_at`: returns only documents where
    /// `after_secs <= received_at < before_secs`. Either bound optional.
    /// Documents with `received_at == 0` are always excluded.
    pub fn search_with_received_at_filter(
        &self,
        query_str: &str,
        tenant_id: &str,
        limit: usize,
        offset: usize,
        after_secs: Option<u64>,
        before_secs: Option<u64>,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Must, tenant_query)];

        if !trimmed.is_empty() {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            clauses.push((Occur::Must, user_query));
        }

        // received_at range: exclude 0 (absent) by setting lower bound to max(1, after).
        let lo = after_secs.map(|v| v.max(1)).unwrap_or(1u64);
        let hi = before_secs;
        let range_query: Box<dyn Query> = Box::new(RangeQuery::new_u64_bounds(
            "received_at".to_owned(),
            std::ops::Bound::Included(lo),
            hi.map(std::ops::Bound::Excluded).unwrap_or(std::ops::Bound::Unbounded),
        ));
        clauses.push((Occur::Must, range_query));

        let final_query = Box::new(BooleanQuery::new(clauses));

        let user_query_for_snippet = if !trimmed.is_empty() {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            parser.parse_query(trimmed).ok()
        } else {
            None
        };

        let snippet_gen = user_query_for_snippet.as_ref().and_then(|q| {
            SnippetGenerator::create(&searcher, q.as_ref(), i.f_body).ok()
        }).map(|mut gen| { gen.set_max_num_chars(200); gen });

        let top_docs = searcher.search(&*final_query, &TopDocs::with_limit(limit).and_offset(offset))?;

        let mut results = Vec::with_capacity(top_docs.len());
        for (score, doc_addr) in top_docs {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let doc_id = match doc.get_first(i.f_doc_id).and_then(|v| TantivyValue::as_str(&v)) {
                Some(id) => id.to_owned(),
                None => continue,
            };
            let subject   = doc.get_first(i.f_subject).and_then(|v| TantivyValue::as_str(&v)).map(str::to_owned);
            let from_addr = doc.get_first(i.f_from_addr).and_then(|v| TantivyValue::as_str(&v)).map(str::to_owned);
            let kind      = doc.get_first(i.f_kind).and_then(|v| TantivyValue::as_str(&v))
                               .filter(|s| !s.is_empty()).map(str::to_owned);
            let received_at = doc.get_first(i.f_received_at)
                .and_then(|v| TantivyValue::as_u64(&v))
                .filter(|&ts| ts > 0);
            let snippet = snippet_gen.as_ref().map(|gen| {
                let s = gen.snippet_from_doc(&doc);
                let text = s.to_html();
                text.replace("<b>", "").replace("</b>", "")
            }).filter(|s| !s.is_empty());
            results.push(SearchHit { document_id: doc_id, score, subject, from_addr, snippet, kind, received_at });
        }

        Ok(results)
    }

    pub fn facet_counts_by_from(
        &self,
        query_str: &str,
        tenant_id: &str,
        top_n: usize,
    ) -> anyhow::Result<Vec<(String, u64)>> {
        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let trimmed = query_str.trim();
        if !trimmed.is_empty() {
            let lowered = trimmed.to_ascii_lowercase();
            if lowered.contains("tenant_id:") || lowered.contains("document_id:") {
                anyhow::bail!("bad_query: query must not reference internal fields");
            }
        }

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let tenant_query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let final_query: Box<dyn Query> = if trimmed.is_empty() {
            tenant_query
        } else {
            let parser = QueryParser::for_index(&i.index, vec![i.f_subject, i.f_body, i.f_from_addr]);
            let user_query = parser.parse_query(trimmed)
                .map_err(|e| anyhow::anyhow!("bad_query: {e}"))?;
            Box::new(BooleanQuery::new(vec![
                (Occur::Must, tenant_query),
                (Occur::Must, user_query),
            ]))
        };

        use tantivy::collector::DocSetCollector;
        let doc_set = searcher.search(&*final_query, &DocSetCollector)?;

        let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let from = doc.get_first(i.f_from_addr)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .unwrap_or("(unknown)")
                .to_owned();
            *counts.entry(from).or_insert(0) += 1;
        }

        let mut sorted: Vec<(String, u64)> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        sorted.truncate(top_n);
        Ok(sorted)
    }

    /// Force-merge all segments into one. Returns (merged_from, status).
    /// No-op when already 0–1 segments. Sprint #595.
    pub async fn force_merge_segments(&self) -> anyhow::Result<(usize, &'static str)> {
        let segment_ids: Vec<SegmentId> = self.inner.index.searchable_segment_ids()
            .map_err(|e| anyhow::anyhow!("segment list failed: {:?}", e))?;
        let merged_from = segment_ids.len();
        if merged_from <= 1 {
            return Ok((merged_from, "already_merged"));
        }
        let future = {
            let mut writer = self.inner.writer.lock().await;
            writer.merge(&segment_ids)
        };
        future.await.map_err(|e| anyhow::anyhow!("merge failed: {:?}", e))?;
        Ok((merged_from, "merged"))
    }

    /// Garbage-collect unreferenced segment files (tombstones, old merges).
    /// Returns the number of files deleted. Sprint #603.
    pub async fn vacuum(&self) -> anyhow::Result<usize> {
        let future = {
            let mut writer = self.inner.writer.lock().await;
            writer.garbage_collect_files()
        };
        let result = future.await
            .map_err(|e| anyhow::anyhow!("vacuum failed: {:?}", e))?;
        Ok(result.deleted_files.len())
    }

    /// Returns index-level health info: total docs across all tenants, number of
    /// segments, and estimated disk size in bytes. Tenant-agnostic (ops endpoint).
    /// Sprint #591.
    pub fn index_health(&self) -> anyhow::Result<(u64, usize, u64)> {
        let i = &self.inner;
        let searcher = i.reader.searcher();

        let num_docs: u64 = searcher.segment_readers().iter()
            .map(|sr| sr.num_docs() as u64)
            .sum();
        let num_segments = searcher.segment_readers().len();

        // Disk size: sum sizes of all files in the index directory.
        let meta = i.index.directory();
        let disk_bytes: u64 = meta.list_managed_files()
            .iter()
            .filter_map(|path| meta.get_file_handle(path).ok())
            .map(|fh| fh.len() as u64)
            .sum();

        Ok((num_docs, num_segments, disk_bytes))
    }

    /// Returns per-segment metadata: id, num_docs, and disk bytes consumed by
    /// that segment's files. Tenant-agnostic (ops endpoint). Sprint #613.
    pub fn list_segments(&self) -> anyhow::Result<Vec<(String, u64, u64)>> {
        let i = &self.inner;
        let searcher = i.reader.searcher();
        let dir      = i.index.directory();

        let segments: Vec<(String, u64, u64)> = searcher
            .segment_readers()
            .iter()
            .map(|sr| {
                let seg_id    = sr.segment_id().uuid_string();
                let num_docs  = sr.num_docs() as u64;
                // Sum file sizes for files belonging to this segment (prefix match).
                let seg_bytes: u64 = dir.list_managed_files()
                    .iter()
                    .filter(|p| {
                        p.to_string_lossy().starts_with(&seg_id)
                    })
                    .filter_map(|p| dir.get_file_handle(p).ok())
                    .map(|fh| fh.len() as u64)
                    .sum();
                (seg_id, num_docs, seg_bytes)
            })
            .collect();

        Ok(segments)
    }

    /// Returns metadata for a single segment by its UUID string. Returns `None`
    /// if no segment with that ID is visible to the current reader. Sprint #618.
    pub fn get_segment(&self, id: &str) -> anyhow::Result<Option<(String, u64, u64)>> {
        let i   = &self.inner;
        let dir = i.index.directory();
        let searcher = i.reader.searcher();

        let result = searcher
            .segment_readers()
            .iter()
            .find(|sr| sr.segment_id().uuid_string() == id)
            .map(|sr| {
                let seg_id   = sr.segment_id().uuid_string();
                let num_docs = sr.num_docs() as u64;
                let seg_bytes: u64 = dir.list_managed_files()
                    .iter()
                    .filter(|p| p.to_string_lossy().starts_with(&seg_id))
                    .filter_map(|p| dir.get_file_handle(p).ok())
                    .map(|fh| fh.len() as u64)
                    .sum();
                (seg_id, num_docs, seg_bytes)
            });
        Ok(result)
    }

    /// Returns aggregate document counts for a given `tenant_id`:
    /// total docs + a breakdown by `kind`. Empty `kind` field is reported as
    /// `"unknown"`. Used by `GET /api/v1/search/stats`. Sprint #584.
    pub fn doc_stats(&self, tenant_id: &str) -> anyhow::Result<(u64, Vec<(String, u64)>)> {
        use tantivy::collector::DocSetCollector;

        let tenant_uuid = Uuid::parse_str(tenant_id.trim())
            .map_err(|_| anyhow::anyhow!("tenant_id must be a valid UUID"))?;
        let tenant_canonical = tenant_uuid.to_string();

        let i = &self.inner;
        let searcher = i.reader.searcher();

        let tenant_term = tantivy::Term::from_field_text(i.f_tenant_id, &tenant_canonical);
        let query: Box<dyn Query> =
            Box::new(TermQuery::new(tenant_term, IndexRecordOption::Basic));

        let doc_set = searcher.search(&*query, &DocSetCollector)?;

        let mut kind_counts: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let kind = doc.get_first(i.f_kind)
                .and_then(|v| TantivyValue::as_str(&v))
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown")
                .to_owned();
            *kind_counts.entry(kind).or_insert(0) += 1;
        }

        let total: u64 = kind_counts.values().sum();
        let mut by_kind: Vec<(String, u64)> = kind_counts.into_iter().collect();
        by_kind.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok((total, by_kind))
    }

    /// Cross-tenant doc count grouped by tenant_id, ordered by count DESC, limited to `limit`.
    ///
    /// Scans all docs (MatchAllDocs) and accumulates counts by the `tenant_id` stored field.
    /// Sprint #684.
    pub fn docs_count_by_tenant(&self, limit: usize) -> anyhow::Result<Vec<(String, u64)>> {
        use tantivy::query::AllQuery;
        use tantivy::collector::DocSetCollector;

        let i = &self.inner;
        let searcher = i.reader.searcher();
        let doc_set = searcher.search(&AllQuery, &DocSetCollector)?;

        let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for doc_addr in doc_set {
            let doc: tantivy::TantivyDocument = searcher.doc(doc_addr)?;
            let tid = doc.get_first(i.f_tenant_id)
                .and_then(|v| TantivyValue::as_str(&v))
                .unwrap_or("unknown")
                .to_owned();
            *counts.entry(tid).or_insert(0) += 1;
        }

        let mut rows: Vec<(String, u64)> = counts.into_iter().collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        rows.truncate(limit);
        Ok(rows)
    }
}

/// Convert unix timestamp (seconds) to a bucket label for temporal facets.
/// granularity: "day" → "YYYY-MM-DD", "week" → "YYYY-Www", "month" → "YYYY-MM".
/// Any unrecognised granularity falls back to "day".
fn unix_secs_to_bucket(ts: u64, granularity: &str) -> String {
    // Integer-only Gregorian calendar computation (no chrono/time dep in index_store).
    let days = (ts / 86400) as i64; // days since 1970-01-01
    let (y, m, d) = days_to_ymd(days);
    match granularity {
        "month" => format!("{:04}-{:02}", y, m),
        "week" => {
            // ISO week: use the Thursday-anchor rule. Compute day-of-week (Mon=0).
            let dow = ((days + 3) % 7) as i32; // Mon=0 … Sun=6 (1970-01-01 was Thu=3)
            // Thursday of the same ISO week
            let thursday_days = days + (3 - dow) as i64;
            let (ty, _, _) = days_to_ymd(thursday_days);
            // Week number = ceil((thursday_days - first_thursday_of_year) / 7) + 1
            let jan1_days = ymd_to_days(ty, 1, 1);
            let jan1_dow = ((jan1_days + 3) % 7) as i32;
            let first_thursday = jan1_days + (3 - jan1_dow) as i64;
            let week_num = ((thursday_days - first_thursday) / 7 + 1) as u32;
            format!("{:04}-W{:02}", ty, week_num)
        }
        _ => format!("{:04}-{:02}-{:02}", y, m, d),
    }
}

fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    // Proleptic Gregorian; algorithm from Henry F. Fliegel & Thomas C. Van Flandern (1968).
    let jd = days + 2440588; // Julian Day Number for 1970-01-01 is 2440588
    let l = jd + 68569;
    let n = (4 * l) / 146097;
    let l = l - (146097 * n + 3) / 4;
    let i = (4000 * (l + 1)) / 1461001;
    let l = l - (1461 * i) / 4 + 31;
    let j = (80 * l) / 2447;
    let d = l - (2447 * j) / 80;
    let l = j / 11;
    let m = j + 2 - 12 * l;
    let y = 100 * (n - 49) + i + l;
    (y as i32, m as u32, d as u32)
}

fn ymd_to_days(y: i32, m: u32, d: u32) -> i64 {
    let jd = (1461 * (y as i64 + 4800 + (m as i64 - 14) / 12)) / 4
        + (367 * (m as i64 - 2 - 12 * ((m as i64 - 14) / 12))) / 12
        - (3 * ((y as i64 + 4900 + (m as i64 - 14) / 12) / 100)) / 4
        + d as i64 - 32075;
    jd - 2440588
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const TENANT_A: &str = "11111111-1111-1111-1111-111111111111";
    const TENANT_B: &str = "22222222-2222-2222-2222-222222222222";

    #[tokio::test]
    async fn index_and_search() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();

        let doc = IndexDoc {
            document_id: "msg-001".to_owned(),
            tenant_id: TENANT_A.to_owned(),
            subject: Some("Meeting tomorrow".to_owned()),
            from_addr: Some("alice@example.com".to_owned()),
            body: Some("Please join the meeting at 10am in the main hall".to_owned()),
            kind: None,
            received_at: None,
        };

        store.index_document(&doc).await.unwrap();
        store.reload().unwrap();

        let hits = store.search("meeting", TENANT_A, 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].document_id, "msg-001");
        assert_eq!(hits[0].subject.as_deref(), Some("Meeting tomorrow"));
        assert_eq!(hits[0].from_addr.as_deref(), Some("alice@example.com"));
        // snippet should contain the body text around the match term
        let snip = hits[0].snippet.as_deref().unwrap_or("");
        assert!(snip.contains("meeting"), "snippet missing match term, got: {snip:?}");

        // Different tenant → no results
        let hits2 = store.search("meeting", TENANT_B, 10, 0).unwrap();
        assert!(hits2.is_empty());

        // Delete and verify gone
        store.delete_document("msg-001").await.unwrap();
        store.reload().unwrap();
        let hits3 = store.search("meeting", TENANT_A, 10, 0).unwrap();
        assert!(hits3.is_empty());
    }

    #[tokio::test]
    async fn rejects_non_uuid_tenant_in_search() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();
        assert!(store.search("hello", "", 10, 0).is_err());
        assert!(store.search("hello", "*", 10, 0).is_err());
        assert!(store.search("hello", "tenant-abc", 10, 0).is_err());
    }

    #[tokio::test]
    async fn rejects_non_uuid_tenant_on_index() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();
        let bad = IndexDoc {
            document_id: "x".into(),
            tenant_id: "not-a-uuid".into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert!(store.index_document(&bad).await.is_err());
    }

    #[tokio::test]
    async fn rejects_tenant_field_in_user_query() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();

        let doc = IndexDoc {
            document_id: "msg-x".into(),
            tenant_id: TENANT_A.into(),
            subject: Some("hello".into()),
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        store.index_document(&doc).await.unwrap();
        store.reload().unwrap();

        // Tentativa de pivot cross-tenant via query string deve falhar.
        let res = store.search(&format!("hello OR tenant_id:{TENANT_B}"), TENANT_A, 10, 0);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().starts_with("bad_query:"));
    }

    #[tokio::test]
    async fn rejects_unknown_field_as_bad_query() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();
        // Campo inexistente → QueryParserError → deve ser tagged bad_query (→ 400).
        let res = store.search("nonexistent_field:hello", TENANT_A, 10, 0);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().starts_with("bad_query:"));
    }

    #[tokio::test]
    async fn rejects_malformed_query_syntax_as_bad_query() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();
        // Parêntese sem fechar → QueryParserError::SyntaxError → bad_query.
        let res = store.search("(subject:hello AND", TENANT_A, 10, 0);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().starts_with("bad_query:"));
    }

    // ─── unix_secs_to_bucket pure-logic tests ────────────────────────────────

    #[test]
    fn bucket_day_epoch() {
        // 1970-01-01T00:00:00Z → "1970-01-01"
        assert_eq!(unix_secs_to_bucket(0, "day"), "1970-01-01");
    }

    #[test]
    fn bucket_day_known_date() {
        // 2026-01-15 = 20469 days since epoch  (2026*365 + leap-years - ...)
        // Use a well-known timestamp: 2026-05-22T00:00:00Z = 1748304000
        let ts: u64 = 1748304000;
        assert_eq!(unix_secs_to_bucket(ts, "day"), "2026-05-22");
    }

    #[test]
    fn bucket_month_known_date() {
        let ts: u64 = 1748304000; // 2026-05-22
        assert_eq!(unix_secs_to_bucket(ts, "month"), "2026-05");
    }

    #[test]
    fn bucket_week_known_date() {
        // 2026-05-22 is a Friday in ISO week 21 of 2026
        let ts: u64 = 1748304000;
        assert_eq!(unix_secs_to_bucket(ts, "week"), "2026-W21");
    }

    #[test]
    fn bucket_unknown_granularity_falls_back_to_day() {
        let ts: u64 = 1748304000;
        assert_eq!(unix_secs_to_bucket(ts, "quarter"), unix_secs_to_bucket(ts, "day"));
    }

    #[test]
    fn bucket_day_end_of_year() {
        // 2025-12-31T00:00:00Z = 1735603200
        let ts: u64 = 1735603200;
        assert_eq!(unix_secs_to_bucket(ts, "day"), "2025-12-31");
        assert_eq!(unix_secs_to_bucket(ts, "month"), "2025-12");
    }

    // ─── days_to_ymd / ymd_to_days round-trip ────────────────────────────────

    #[test]
    fn ymd_roundtrip_epoch() {
        let days = ymd_to_days(1970, 1, 1);
        assert_eq!(days, 0);
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn ymd_roundtrip_leap_year() {
        // 2000-02-29 exists (leap year)
        let days = ymd_to_days(2000, 2, 29);
        let (y, m, d) = days_to_ymd(days);
        assert_eq!((y, m, d), (2000, 2, 29));
    }

    #[test]
    fn ymd_roundtrip_multiple_dates() {
        let cases = [(2026, 5, 22), (2000, 1, 1), (1999, 12, 31), (2038, 1, 19)];
        for (ey, em, ed) in cases {
            let days = ymd_to_days(ey, em, ed);
            let (y, m, d) = days_to_ymd(days);
            assert_eq!((y, m, d), (ey as i32, em, ed), "roundtrip failed for {ey}-{em:02}-{ed:02}");
        }
    }

    // ─── IndexStore facet isolation ──────────────────────────────────────────

    #[tokio::test]
    async fn facet_counts_by_kind_tenant_isolation() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();

        store.index_document(&IndexDoc {
            document_id: "d1".into(), tenant_id: TENANT_A.into(),
            subject: None, from_addr: None, body: None,
            kind: Some("mail".into()), received_at: None,
        }).await.unwrap();
        store.index_document(&IndexDoc {
            document_id: "d2".into(), tenant_id: TENANT_B.into(),
            subject: None, from_addr: None, body: None,
            kind: Some("drive".into()), received_at: None,
        }).await.unwrap();
        store.reload().unwrap();

        let fa = store.facet_counts_by_kind("", TENANT_A).unwrap();
        assert_eq!(fa, vec![("mail".to_string(), 1)]);

        let fb = store.facet_counts_by_kind("", TENANT_B).unwrap();
        assert_eq!(fb, vec![("drive".to_string(), 1)]);
    }

    #[tokio::test]
    async fn doc_stats_empty_index() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();
        let (total, by_kind) = store.doc_stats(TENANT_A).unwrap();
        assert_eq!(total, 0);
        assert!(by_kind.is_empty());
    }

    #[tokio::test]
    async fn doc_stats_counts_by_kind() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();
        for (id, kind) in [("a", "mail"), ("b", "mail"), ("c", "drive")] {
            store.index_document(&IndexDoc {
                document_id: id.into(), tenant_id: TENANT_A.into(),
                subject: None, from_addr: None, body: None,
                kind: Some(kind.into()), received_at: None,
            }).await.unwrap();
        }
        store.reload().unwrap();
        let (total, by_kind) = store.doc_stats(TENANT_A).unwrap();
        assert_eq!(total, 3);
        assert!(by_kind.contains(&("mail".to_string(), 2)));
        assert!(by_kind.contains(&("drive".to_string(), 1)));
    }

    #[tokio::test]
    async fn index_documents_batch_rejects_invalid() {
        let dir = tempdir().unwrap();
        let store = IndexStore::open(dir.path()).unwrap();
        let docs = vec![
            IndexDoc { document_id: "ok1".into(), tenant_id: TENANT_A.into(),
                       subject: None, from_addr: None, body: None, kind: None, received_at: None },
            IndexDoc { document_id: "bad".into(), tenant_id: "not-a-uuid".into(),
                       subject: None, from_addr: None, body: None, kind: None, received_at: None },
        ];
        let rejected = store.index_documents(&docs).await.unwrap();
        assert_eq!(rejected, vec!["bad".to_string()]);
        store.reload().unwrap();
        let hits = store.search("", TENANT_A, 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn index_doc_all_optional_fields_are_none_by_default() {
        let doc = IndexDoc {
            document_id: "x".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert!(doc.subject.is_none());
        assert!(doc.received_at.is_none());
    }

    #[test]
    fn index_doc_document_id_preserved() {
        let doc = IndexDoc {
            document_id: "msg-abc".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert_eq!(doc.document_id, "msg-abc");
    }

    #[test]
    fn index_doc_tenant_id_preserved() {
        let doc = IndexDoc {
            document_id: "d1".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert_eq!(doc.tenant_id, TENANT_A);
    }

    #[test]
    fn index_doc_kind_none_by_default() {
        let doc = IndexDoc {
            document_id: "d2".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert!(doc.kind.is_none());
    }

    #[test]
    fn index_doc_tenant_b_id_preserved() {
        let doc = IndexDoc {
            document_id: "d3".into(),
            tenant_id: TENANT_B.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert_eq!(doc.tenant_id, TENANT_B);
    }

    #[test]
    fn index_doc_body_none_by_default() {
        let doc = IndexDoc {
            document_id: "d4".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert!(doc.body.is_none());
    }

    #[test]
    fn index_doc_received_at_some_preserved() {
        let doc = IndexDoc {
            document_id: "d5".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: Some(1_700_000_000),
        };
        assert_eq!(doc.received_at, Some(1_700_000_000));
    }

    #[test]
    fn index_doc_unique_document_id_roundtrips() {
        let doc = IndexDoc {
            document_id: "unique-doc-id".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert_eq!(doc.document_id, "unique-doc-id");
    }

    #[test]
    fn index_doc_from_addr_none_by_default() {
        let doc = IndexDoc {
            document_id: "d6".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert!(doc.from_addr.is_none());
    }

    #[test]
    fn index_doc_kind_some_preserved() {
        let doc = IndexDoc {
            document_id: "d7".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: Some("email".into()),
            received_at: None,
        };
        assert_eq!(doc.kind.as_deref(), Some("email"));
    }

    #[test]
    fn index_doc_subject_some_preserved() {
        let doc = IndexDoc {
            document_id: "d8".into(),
            tenant_id: TENANT_A.into(),
            subject: Some("Hello World".into()),
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert_eq!(doc.subject.as_deref(), Some("Hello World"));
    }

    #[test]
    fn index_doc_document_id_nonempty_after_construction() {
        let doc = IndexDoc {
            document_id: "doc-99".into(),
            tenant_id: TENANT_A.into(),
            subject: None,
            from_addr: None,
            body: None,
            kind: None,
            received_at: None,
        };
        assert!(!doc.document_id.is_empty());
    }
}

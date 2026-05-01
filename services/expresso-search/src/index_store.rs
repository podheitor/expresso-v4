//! Tantivy index management — shared state for the search service.
//! Schema: document_id (stored), tenant_id (indexed), subject (full-text+stored),
//! from_addr (stored+indexed), body (full-text+stored), received_at (fast-field).
//!
//! NOTE: body was changed from TEXT to TEXT|STORED. Existing indexes built
//! before this change need re-indexing to populate stored body values; until
//! then snippet will be None for old documents.

use std::path::Path;
use tantivy::schema::Value as TantivyValue;
use std::sync::Arc;

use tantivy::{
    collector::TopDocs,
    directory::MmapDirectory,
    doc,
    query::{BooleanQuery, Occur, Query, QueryParser, TermQuery},
    schema::{
        Field, IndexRecordOption, Schema, STORED, STRING, TEXT,
    },
    snippet::SnippetGenerator,
    Index, IndexReader, IndexWriter, ReloadPolicy,
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
            i.f_doc_id    => doc_data.document_id.as_str(),
            i.f_tenant_id => tenant_canonical.as_str(),
            i.f_subject   => doc_data.subject.as_deref().unwrap_or(""),
            i.f_from_addr => doc_data.from_addr.as_deref().unwrap_or(""),
            i.f_body      => doc_data.body.as_deref().unwrap_or(""),
            i.f_kind      => doc_data.kind.as_deref().unwrap_or(""),
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
                i.f_doc_id    => doc_data.document_id.as_str(),
                i.f_tenant_id => tenant_canonical.as_str(),
                i.f_subject   => doc_data.subject.as_deref().unwrap_or(""),
                i.f_from_addr => doc_data.from_addr.as_deref().unwrap_or(""),
                i.f_body      => doc_data.body.as_deref().unwrap_or(""),
                i.f_kind      => doc_data.kind.as_deref().unwrap_or(""),
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

    /// Force reader reload — primarily for tests.
    #[cfg(test)]
    pub fn reload(&self) -> anyhow::Result<()> {
        self.inner.reader.reload()?;
        Ok(())
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
            let snippet = snippet_gen.as_ref().map(|gen| {
                let s = gen.snippet_from_doc(&doc);
                let text = s.to_html();
                // Strip tantivy's <b>…</b> highlight tags — return plain text.
                text.replace("<b>", "").replace("</b>", "")
            }).filter(|s| !s.is_empty());
            results.push(SearchHit { document_id: doc_id, score, subject, from_addr, snippet, kind });
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
}

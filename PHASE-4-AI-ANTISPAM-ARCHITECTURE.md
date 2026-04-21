# Phase 4: AI + Antispam Architecture Planning
**Expresso v4 — Rust/Axum Ecosystem**

## Executive Summary
Expresso V4 **ALREADY USES** Rust + Axum + Tokio across all 13 services. This document plans integration of:
- **Antispam Module**: Hybrid (rule-based + ML + LLM-powered)
- **AI Email Interface**: Summarization, smart compose, search enhancement

---

## 1. ANTISPAM ARCHITECTURE

### 1.1 Core Components

#### A. **Local Classifier (Rust-native)**
- **Tech**: `ort` (ONNX Runtime) + pre-trained SpamAssassin ONNX model
- **Location**: New lib `expresso-antispam`
- **Features**:
  - Header analysis (SPF/DKIM/DMARC verification)
  - Content heuristics (phishing patterns, suspicious URLs, etc.)
  - Bayesian filtering (token frequency analysis)
  - Feedback loop (user marking spam retrains model)

#### B. **External Service Adapters**
- **SpamAssassin** (optional, via HTTP API on dedicated container)
- **SURBL** (URL reputation lists)
- **Barracuda** / **Cloudmark** (optional cloud service)
- **Custom**: User-defined webhook for external scanning

#### C. **LLM-Powered Analysis**
- **Fallback for edge cases**: Use LLM (OpenAI/Anthropic/Ollama) for semantic content analysis
- **Detection Types**: 
  - Credential harvesting patterns
  - Business Email Compromise (BEC) detection
  - Targeted phishing (spear phishing)
  - Spoofed sender detection
- **Async job queue**: Long-running LLM calls via NATS background jobs

#### D. **Scoring & Quarantine**
- **Probabilistic scoring** (0-100):
  - 0-30: Likely clean
  - 31-70: Suspicious (flag for review or manual action)
  - 71-100: High spam probability (auto-quarantine or move to spam)
- **Quarantine folder**: Separate system folder + Recovery UI
- **Allow/Block lists**: Per-user + global

---

### 1.2 Implementation Plan

#### Phase 4.1: Core Antispam Lib (2-3 weeks)
```
libs/expresso-antispam/
├── src/
│   ├── classifier/
│   │   ├── bayesian.rs        # Token-based scoring
│   │   ├── heuristics.rs      # Header/URL/content rules
│   │   └── model.rs           # ONNX inference (ort crate)
│   ├── integrations/
│   │   ├── spamassassin.rs    # External adapter
│   │   ├── surbl.rs           # URL reputation
│   │   └── llm.rs             # OpenAI/Anthropic/Ollama
│   ├── scoring.rs             # Scoring logic & thresholds
│   ├── feedback.rs            # User feedback training
│   └── lib.rs
└── Cargo.toml
```

**Dependencies**:
```toml
ort = "2.0"                    # ONNX Runtime inference
reqwest = { workspace = true } # HTTP calls to external services
tokio = { workspace = true }   # Async
sqlx = { workspace = true }    # Store feedback data
redis = { workspace = true }   # Cache spam scores
serde_json = { workspace = true }
```

**Key APIs**:
```rust
pub async fn classify_message(
    headers: &EmailHeaders,
    body: &str,
    sender: &str,
) -> Result<SpamScore, AntiSpamError>;

pub async fn register_feedback(message_id: Uuid, is_spam: bool) -> Result<()>;

pub struct SpamScore {
    pub total: f32,              // 0-100
    pub local_score: f32,
    pub external_score: Option<f32>,
    pub llm_analysis: Option<LLMAnalysis>,
    pub details: HashMap<String, f32>, // breakdown by detector
}
```

#### Phase 4.2: Integrate into expresso-mail (1-2 weeks)
- Hook into `SMTP ingest` + `API receive message`
- Background scoring job (NATS)
- Add `score` + `spam_status` fields to `Message` table
- UI: Show spam badge + quarantine recovery

#### Phase 4.3: External Integrations (1 week)
- Implement SpamAssassin HTTP adapter
- Add user-configurable webhooks
- Rate-limiting + circuit-breaker pattern

#### Phase 4.4: LLM Integration (1 week)
- Setup OpenAI/Anthropic client (optional)
- Or: Use local Ollama for privacy
- Background async scoring for high-value targets

---

## 2. AI EMAIL INTERFACE

### 2.1 Use Cases

#### A. **Thread Summarization**
- **Trigger**: User clicks "Summarize" button on conversation
- **Tech**: LLM (OpenAI API / Ollama local)
- **Output**: 
  - Key points (bullets)
  - Sentiment (positive/neutral/negative)
  - Action items
  - Timeline (chronological events)

#### B. **Smart Compose**
- **Features**:
  - Auto-complete (next phrase prediction via small local model)
  - Tone adjustment (formal ↔ casual)
  - Spell/grammar check + style suggestions
  - Detect recipients by intention ("copy John on the budget discussion")
- **Tech**: 
  - Local: `candle` (Hugging Face) for embeddings + lightweight models
  - Cloud: OpenAI GPT-4 for polish + tone adjustment

#### C. **Search Enhancement**
- **Semantic search**: "Find emails about our Q4 roadmap"
- **Tech**: Embeddings stored in `pgvector` (PostgreSQL), HNSW indexing
- **Retriever**: BM25 (keyword) + semantic (vector) hybrid search

#### D. **Folder Organization AI**
- **Auto-categorization**: Suggest folders based on content + sender
- **Priority inbox**: ML scoring for importance
- **Auto-archive**: Old read emails

#### E. **Email Analytics**
- **Dashboards**: 
  - Response time trends
  - Sender relationship strength
  - Topic frequency over time
- **Predictions**: Send time optimization, best time to respond

---

### 2.2 Implementation Plan

#### Phase 5.1: Embeddings Infrastructure (1 week)
```sql
-- New PostgreSQL extension
CREATE EXTENSION IF NOT EXISTS vector;

-- Messages embeddings table
CREATE TABLE message_embeddings (
  id UUID PRIMARY KEY,
  message_id UUID NOT NULL,
  user_id UUID NOT NULL,
  
  -- Content embedding (1536 dims for OpenAI / vary for Ollama)
  embedding vector(1536),
  
  -- For quick filtering
  scope TEXT NOT NULL, -- 'subject', 'body', 'thread'
  
  created_at TIMESTAMP DEFAULT NOW(),
  FOREIGN KEY(message_id) REFERENCES messages(id),
  FOREIGN KEY(user_id) REFERENCES users(id)
);

CREATE INDEX idx_msg_emb_user_id ON message_embeddings(user_id);
CREATE INDEX idx_msg_emb_vector ON message_embeddings USING ivfflat(embedding vector_cosine_ops);
```

**Lib**: `expresso-ai` (new)
```
libs/expresso-ai/
├── src/
│   ├── embeddings/
│   │   ├── openai.rs         # OpenAI API client
│   │   ├── ollama.rs         # Local Ollama client
│   │   └── cached.rs         # Embedding cache (Redis)
│   ├── summarization.rs      # Thread summarization
│   ├── compose.rs            # Smart compose helpers
│   ├── search.rs             # Semantic + hybrid search
│   └── lib.rs
└── Cargo.toml
```

**Dependencies**:
```toml
openai-api-rs = "5.0"          # OpenAI client
reqwest = { workspace = true } # HTTP (Ollama, etc)
tokenizers = "0.15"            # Tokenization for embeddings
pgvector = "0.3"               # Vector SQL support
redis = { workspace = true }   # Cache embeddings
```

#### Phase 5.2: Summarization Service (1 week)
- **Endpoint**: `POST /api/threads/{id}/summarize`
- **Flow**:
  1. Load thread messages
  2. Tokenize + truncate (fit LLM context window)
  3. Call LLM with system prompt
  4. Cache result (Redis, 7 days)
  5. Return structured summary

#### Phase 5.3: Smart Compose (1 week)
- **Endpoint**: `POST /api/compose/suggest`
- **Input**: Current draft + context
- **Output**: 
  - Next token suggestions
  - Grammar/style corrections
  - Recipient predictions

#### Phase 5.4: Semantic Search (1 week)
- **Enhance** `GET /api/messages/search`
- **Add**: Vector search param + hybrid ranking
- **Indexing**: Background job (NATS) on new/edited messages

#### Phase 5.5: Analytics Dashboard (1 week)
- **Frontend**: React components (dashboard view)
- **Backend**: Aggregation queries (PostgreSQL window functions)
- **Data**: Message metadata + user interactions

---

## 3. WHERE ELSE TO USE AI

### 3.1 In Other Services

#### **expresso-calendar**
- Event name/description suggestions
- Auto-scheduling (find optimal time across attendees via ML)
- Detect meeting duration from context

#### **expresso-contacts**
- Profile enrichment (LinkedIn API integration)
- Company info auto-fetch
- Duplicate detection (fuzzy matching)

#### **expresso-drive**
- File naming suggestions
- Auto-tagging (via OCR + classification)
- Sensitive data detection (PII redaction)

#### **expresso-compliance**
- Legal document classification
- Retention policy automation
- Regulatory requirement detection

#### **expresso-search**
- Cross-service semantic search
- Query expansion (synonyms, related concepts)
- Result ranking by relevance

#### **expresso-chat**
- Conversational AI assistance (copilot mode)
- Sentiment analysis + escalation detection
- Suggested responses

---

## 4. ARCHITECTURE DIAGRAM

```
┌─────────────────────────────────────────────────────────────┐
│              User Interface (React/Vue)                     │
├─────────────────────────────────────────────────────────────┤
│  - Email compose (w/ smart suggestions)                     │
│  - Thread view (w/ summarization button)                    │
│  - Search bar (semantic)                                    │
│  - Analytics dashboard                                      │
└───────────────────┬─────────────────────────────────────────┘
                    │
        ┌───────────┼───────────┐
        │           │           │
    ┌───▼──────┐ ┌──▼────────┐ ┌▼──────────────┐
    │Expresso- │ │Expresso-  │ │Expresso-      │
    │Mail      │ │Search     │ │Drive + others │
    │(Axum)    │ │(Axum)     │ │(Axum)         │
    └───┬──────┘ └───┬──────┘ └┬──────────────┘
        │            │        │
        │     ┌──────┼────────┘
        │     │      │
    ┌───▼─────▼──────▼───────────────────────┐
    │     Core Libraries (Workspace)         │
    ├────────────────────────────────────────┤
    │ ┌──────────────────────────────────┐  │
    │ │ expresso-antispam                │  │
    │ │ - Bayesian classifier (local)    │  │
    │ │ - Header heuristics              │  │
    │ │ - External adapters              │  │
    │ │ - LLM-powered analysis           │  │
    │ └──────────────────────────────────┘  │
    │ ┌──────────────────────────────────┐  │
    │ │ expresso-ai                      │  │
    │ │ - Embeddings (OpenAI/Ollama)     │  │
    │ │ - Summarization                  │  │
    │ │ - Smart compose                  │  │
    │ │ - Search enhancement             │  │
    │ │ - Analytics                      │  │
    │ └──────────────────────────────────┘  │
    │ expresso-storage, expresso-config... │
    └─────────────────┬─────────────────────┘
                      │
        ┌─────────────┼─────────────┐
        │             │             │
    ┌───▼──────┐  ┌──▼───────┐ ┌──▼───────┐
    │PostgreSQL│  │ Redis    │ │ S3/MinIO │
    │(pgvector)│  │(cache)   │ │(storage) │
    └──────────┘  └──────────┘ └──────────┘
    
    ┌─────────────────────────────────────────┐
    │ External Services (Optional)            │
    ├─────────────────────────────────────────┤
    │ - OpenAI / Anthropic / Ollama (LLM)    │
    │ - SpamAssassin (antispam API)          │
    │ - SURBL (URL reputation)               │
    │ - LinkedIn API (contacts enrichment)   │
    └─────────────────────────────────────────┘
    
    ┌─────────────────────────────────────────┐
    │ Background Jobs (NATS)                  │
    ├─────────────────────────────────────────┤
    │ - Spam classification                  │
    │ - Embedding generation                 │
    │ - LLM-based analysis                   │
    │ - Analytics data aggregation           │
    └─────────────────────────────────────────┘
```

---

## 5. IMPLEMENTATION TIMELINE

| Phase | Task | Duration | Start | End |
|-------|------|----------|-------|-----|
| 4.0   | Current: Deployment + stabilization | 1-2 wk | Week 1-2 | - |
| 4.1   | Antispam lib (Bayesian + heuristics) | 2-3 wk | Week 3 | Week 5 |
| 4.2   | Integrate into expresso-mail | 1-2 wk | Week 6 | Week 7 |
| 4.3   | External service adapters | 1 wk | Week 8 | Week 8 |
| 4.4   | LLM integration (OpenAI/Ollama) | 1 wk | Week 9 | Week 9 |
| 5.1   | Embeddings infrastructure + pgvector | 1 wk | Week 10 | Week 10 |
| 5.2   | Summarization service | 1 wk | Week 11 | Week 11 |
| 5.3   | Smart compose | 1 wk | Week 12 | Week 12 |
| 5.4   | Semantic search enhancement | 1 wk | Week 13 | Week 13 |
| 5.5   | Analytics dashboard | 1 wk | Week 14 | Week 14 |
| **Total Estimated Build Time** | | | | **14-15 weeks** |

---

## 6. CONFIGURATION

### Environment Variables (New)

```env
# Antispam
ANTISPAM__ENABLED=true
ANTISPAM__LOCAL_THRESHOLD=0.7
ANTISPAM__EXTERNAL__ENABLED=true
ANTISPAM__EXTERNAL__PROVIDER=spamassassin  # or 'custom'
ANTISPAM__EXTERNAL__URL=http://spamassassin:783
ANTISPAM__LLM__ENABLED=true
ANTISPAM__LLM__PROVIDER=openai  # or 'anthropic', 'ollama'
ANTISPAM__LLM__API_KEY=<secret>

# AI Features
AI__ENABLED=true
AI__EMBEDDINGS_PROVIDER=openai  # or 'ollama'
AI__EMBEDDINGS_MODEL=text-embedding-3-small
AI__EMBEDDINGS_CACHE_TTL_SECS=604800  # 7 days
AI__LLM_PROVIDER=openai
AI__LLM_MODEL=gpt-4-turbo
AI__SUMMARIZATION_ENABLED=true
AI__SMART_COMPOSE_ENABLED=true
AI__SEMANTIC_SEARCH_ENABLED=true

# Ollama (for local, privacy-preserving option)
OLLAMA__ENABLED=false
OLLAMA__BASE_URL=http://ollama:11434
OLLAMA__EMBEDDING_MODEL=nomic-embed-text
OLLAMA__LLM_MODEL=llama2:7b-chat
```

---

## 7. SECURITY & PRIVACY CONSIDERATIONS

1. **LLM Data Handling**:
   - Option 1: Cloud API (OpenAI/Anthropic) - data sent externally
   - Option 2: Local Ollama - all processing on-premise
   - **Default**: Opt-in toggle per user + privacy mode

2. **Email Content**:
   - Never log raw email bodies to external services
   - Truncate long emails before sending to LLM
   - Hash sensitive patterns before analysis

3. **Model Storage**:
   - ONNX models stored in encrypted S3 bucket
   - Model versioning + audit trail

4. **Rate Limiting**:
   - Per-user quota on LLM calls (to avoid abuse)
   - Exponential backoff for external APIs

---

## 8. TESTING STRATEGY

- **Unit tests**: Classifier heuristics, scoring logic
- **Integration tests**: Full antispam pipeline + external mock
- **E2E tests**: User journey (compose → send → classify)
- **Benchmark tests**: Latency of classification (target <100ms local)

---

## 9. RESOURCE REQUIREMENTS

### Hardware (for production)
- **Local models** (Ollama): 
  - 6GB VRAM (for embedding model)
  - 8GB VRAM (for LLM model)
  - Or: GPU if available (CUDA/Metal acceleration)

- **ML Infrastructure**:
  - Redis cache (2-4GB for embeddings)
  - PostgreSQL with pgvector extension
  - NATS queue for async jobs

### Budget (if using cloud LLMs)
- OpenAI: ~$0.02-0.10 per classification (1000s/mo)
- Anthropic: Similar
- Self-hosted Ollama: $0 (infrastructure cost only)

---

## 10. RECOMMENDED START POINT

**Immediate Next Steps**:
1. ✅ Stabilize Phase 3 (finish 13 service deployments)
2. 🔄 Phase 4.1: Start antispam lib development (2 weeks)
3. 🔄 Parallel: Set up PostgreSQL pgvector extension
4. 🔄 Research: Choose LLM provider (OpenAI vs local Ollama)

**Decision Tree**:
```
Do you want Cloud LLM (OpenAI/Anthropic)?
├─ YES → More powerful, easier, but ongoing API costs + privacy
└─ NO → Use local Ollama, privacy-first, but requires GPU/CPU resources
```

---

**Document Version**: 1.0 (2026-04-19)
**Status**: Architecture Planning
**Next Review**: After Phase 3 completion

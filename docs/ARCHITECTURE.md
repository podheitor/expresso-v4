# Arquitetura Técnica — Expresso V4

> Stack tecnológico definitivo para a suite colaborativa governamental brasileira

## Sistema Operacional

**Debian 13 "Trixie"** — escolha definitiva para todos os ambientes  
- Kernel 6.11+ LTS  
- Ciclo de suporte 5 anos (LTS)  
- Alinhado com e-PING (distribuição livre baseada em Debian)  
- ≠ Oracle Linux / RHEL (custo + lock-in)  

## Stack por Camada

```
┌─────────────────────────────────────────────────────┐
│                 CLIENTES                             │
│  Browser (SvelteKit + WASM)  │  PWA Mobile          │
│  Tauri Desktop (Rust)        │  IMAP/CalDAV/CardDAV │
└─────────────────────────────────────────────────────┘
                    │ HTTPS / WSS
┌─────────────────────────────────────────────────────┐
│              GATEWAY (Pingora — Rust)                │
│  TLS termination │ Rate limiting │ Auth validation   │
└─────────────────────────────────────────────────────┘
                    │ HTTP/2 gRPC
┌────────────────────────────────────────────────────────────────┐
│                      MICROSERVIÇOS (Rust)                       │
│  expresso-mail    │ expresso-calendar │ expresso-contacts       │
│  expresso-drive   │ expresso-chat     │ expresso-meet           │
│  expresso-admin   │ expresso-auth     │ expresso-compliance     │
│  expresso-search  │ expresso-wopi     │ expresso-flows          │
└────────────────────────────────────────────────────────────────┘
                    │
┌───────────────────────────────────────────────────────────────┐
│                    PLATAFORMA                                  │
│  PostgreSQL 16   │ Redis 7    │ MinIO     │ NATS JetStream     │
│  OpenSearch      │ Keycloak   │ LOOL      │ Prometheus         │
└───────────────────────────────────────────────────────────────┘
```

## Microserviços — Definição

| Serviço | Função | Porta | Dependências |
|---------|--------|-------|-------------|
| `expresso-mail` | SMTP/IMAP server + webmail API | 25, 587, 993, 8001 | PostgreSQL, Redis, MinIO, NATS |
| `expresso-calendar` | CalDAV server + meeting mgmt | 8002 | PostgreSQL, Redis |
| `expresso-contacts` | CardDAV + LDAP gateway | 8003, 389 | PostgreSQL |
| `expresso-drive` | WebDAV + file sync API | 8004 | PostgreSQL, MinIO |
| `expresso-wopi` | WOPI bridge para LibreOffice Online | 8005 | expresso-drive |
| `expresso-chat` | Matrix homeserver (Conduit/Conduwuit) | 8006, 8448 | PostgreSQL, Redis |
| `expresso-meet` | WebRTC SFU (mediasoup) | 8007, 10000-20000 UDP | Redis, NATS |
| `expresso-auth` | Keycloak companion + gov.br adapter | 8080 | PostgreSQL, Redis |
| `expresso-admin` | Admin API + tenant management | 8008 | PostgreSQL, all services |
| `expresso-compliance` | Audit, eDiscovery, DLP, Labels | 8009 | PostgreSQL, MinIO |
| `expresso-search` | Full-text search (Tantivy) | 8010 | OpenSearch, NATS |
| `expresso-flows` | Workflow engine (webhooks + triggers) | 8011 | PostgreSQL, NATS |
| `expresso-notifications` | Push notifications (Web Push, email) | 8012 | Redis, NATS |

## Bibliotecas Rust Principais

```toml
# Cargo.toml workspace dependencies

[dependencies]
# Web framework
axum = "0.7"
tokio = { version = "1", features = ["full"] }
tower = "0.5"
tower-http = "0.6"

# Database
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio-native-tls", "uuid", "time"] }
deadpool-postgres = "0.14"
deadpool-redis = "0.16"

# Mail
lettre = { version = "0.11", features = ["tokio1", "native-tls"] }
mail-parser = "0.9"
imap-codec = "2"

# gRPC
tonic = "0.12"
prost = "0.13"

# Crypto
ring = "0.17"
age = "0.10"
ed25519-dalek = "2"
x509-parser = "0.16"

# Search
tantivy = "0.22"

# Object storage
aws-sdk-s3 = "1"  # MinIO compat

# Auth/OIDC
openidconnect = "4"
jsonwebtoken = "9"

# Observability
opentelemetry = "0.24"
tracing = "0.1"
tracing-subscriber = "0.3"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# WebSocket
tokio-tungstenite = "0.24"
```

## Banco de Dados — Schema Core

```sql
-- Multi-tenancy via Row Level Security (PostgreSQL 16)

-- Tenants (organizations/órgãos)
CREATE TABLE tenants (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    slug        TEXT UNIQUE NOT NULL,  -- ex: "mda.gov.br"
    name        TEXT NOT NULL,
    cnpj        TEXT UNIQUE,
    plan        TEXT NOT NULL DEFAULT 'standard',
    created_at  TIMESTAMPTZ DEFAULT now(),
    config      JSONB DEFAULT '{}'
);

-- Users
CREATE TABLE users (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID NOT NULL REFERENCES tenants(id),
    email       TEXT NOT NULL,
    cpf         TEXT,                  -- gov.br sync
    sub_govbr   TEXT,                  -- gov.br subject
    display_name TEXT NOT NULL,
    role        TEXT NOT NULL DEFAULT 'user',
    quota_bytes BIGINT DEFAULT 53687091200,  -- 50 GB
    created_at  TIMESTAMPTZ DEFAULT now(),
    UNIQUE(tenant_id, email)
);

-- RLS policies
ALTER TABLE users ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON users
    USING (tenant_id = current_setting('app.tenant_id')::UUID);

-- Mailboxes
CREATE TABLE mailboxes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id),
    tenant_id   UUID NOT NULL REFERENCES tenants(id),
    folder_name TEXT NOT NULL,         -- INBOX, Sent, Drafts...
    uid_validity BIGINT NOT NULL,
    next_uid    BIGINT NOT NULL DEFAULT 1,
    subscribed  BOOL DEFAULT true,
    created_at  TIMESTAMPTZ DEFAULT now(),
    UNIQUE(user_id, folder_name)
);

-- Messages
CREATE TABLE messages (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mailbox_id  UUID NOT NULL REFERENCES mailboxes(id),
    tenant_id   UUID NOT NULL REFERENCES tenants(id),
    uid         BIGINT NOT NULL,
    flags       TEXT[] DEFAULT '{}',
    size_bytes  INT NOT NULL,
    subject     TEXT,
    from_addr   TEXT,
    to_addrs    TEXT[],
    date        TIMESTAMPTZ,
    message_id  TEXT,
    references_ TEXT[],
    in_reply_to TEXT,
    body_path   TEXT NOT NULL,         -- S3 path in MinIO
    created_at  TIMESTAMPTZ DEFAULT now(),
    UNIQUE(mailbox_id, uid)
);

-- Audit log (append-only)
CREATE TABLE audit_log (
    id          BIGSERIAL PRIMARY KEY,
    tenant_id   UUID NOT NULL,
    user_id     UUID,
    action      TEXT NOT NULL,
    resource    TEXT NOT NULL,
    metadata    JSONB DEFAULT '{}',
    ip_addr     INET,
    user_agent  TEXT,
    created_at  TIMESTAMPTZ DEFAULT now()
);
-- Sem UPDATE/DELETE via policy
REVOKE UPDATE, DELETE ON audit_log FROM PUBLIC;
```

## Observabilidade

```yaml
# Stack de observabilidade
OpenTelemetry Collector:
  traces:  → Tempo (Grafana)
  metrics: → Prometheus → Grafana
  logs:    → Loki → Grafana

Alertas:
  Prometheus Alertmanager → Slack/e-mail/PagerDuty

Dashboards Grafana:
  - Expresso Mail (SMTP/IMAP metrics)
  - Expresso Drive (upload/download rates, quota)
  - Expresso Chat (active rooms, messages/sec)
  - Security (login failures, DLP alerts)
  - Tenant Health (por órgão)
```

## Segurança — Camadas

```
Nível 1: Rede
  - TLS 1.3 obrigatório (TLS 1.2 permitido com justificativa)
  - HSTS + HSTS preloading
  - CSP, X-Frame-Options, X-Content-Type-Options
  - Rate limiting por IP + por user

Nível 2: Autenticação
  - OAuth2.1 + PKCE mandatory
  - OIDC com Keycloak
  - gov.br OIDC federation
  - WebAuthn/FIDO2 passkeys
  - Sessões com rotação de tokens

Nível 3: Autorização
  - RBAC com PostgreSQL RLS
  - OPA (Open Policy Agent) para Conditional Access
  - Zero-trust: cada request autenticado + autorizado

Nível 4: Dados
  - Criptografia em repouso: AES-256-GCM (MinIO SSE-S3)
  - Criptografia em trânsito: TLS 1.3
  - E2EE opcional (age encryption, client-side)
  - Chaves por tenant (BYOK via HashiCorp Vault)

Nível 5: Governo
  - ICP-Brasil para assinaturas digitais
  - WORM storage para audit log (S3 Object Lock)
  - HSM para chaves críticas (futuro)
```

## Performance Targets

| Métrica | Target | Medição |
|---------|--------|---------|
| Latência IMAP FETCH | < 50ms p99 | Prometheus histogram |
| Latência WebMail (SvelteKit) | < 200ms TTFB | Lighthouse |
| Throughput SMTP entrega | > 10.000 msgs/min | Benchmark |
| Busca full-text | < 100ms p99 (1M msgs) | Tantivy benchmark |
| Upload Drive (100 MB) | < 10s LAN | tus.io benchmark |
| WebRTC meeting (100 participantes) | < 150ms latência média | mediasoup stats |
| Disponibilidade | 99.95% (≈ 26min downtime/mês) | Prometheus uptime |

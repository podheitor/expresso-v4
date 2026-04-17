# Roadmap — Expresso V4

> Roadmap incremental de desenvolvimento por fases e sprints

## Visão de Fases

| Fase | Módulos | Duração | Entregável |
|------|---------|---------|-----------|
| **Fase 1** | Mail (MVP) | 14 semanas | E-mail funcional, IMAP, WebMail |
| **Fase 2** | Calendar + Contacts | 8 semanas | CalDAV, CardDAV, Booking |
| **Fase 3** | Drive + Office Online | 12 semanas | WebDAV, LOOL co-edição |
| **Fase 4** | Chat (Matrix) | 10 semanas | Canal, DM, presença |
| **Fase 5** | Meet + Admin | 12 semanas | WebRTC, tenant admin |
| **Fase 6** | Compliance + AI | 16 semanas | eDiscovery, DLP, LLM |
| **Fase 7** | Enterprise | TBD | BI, Flows, Extensions |

---

## Fase 1 — Expresso Mail MVP (14 semanas)

### Sprint 1–2 (Semanas 1–4): Infraestrutura Base
- [ ] Monorepo scaffold (Cargo workspace + pnpm workspace)
- [ ] Dockerfile para Debian 13 base image
- [ ] Docker Compose: PostgreSQL 16, Redis 7, MinIO
- [ ] Migration engine (sqlx migrations)
- [ ] Schema inicial: tenants, users, mailboxes, messages
- [ ] CI/CD pipeline (GitHub Actions ou Gitea CI)
- [ ] Observabilidade inicial: tracing + Prometheus

### Sprint 3–4 (Semanas 5–8): SMTP + IMAP Server
- [ ] SMTP server (porta 25 + 587 STARTTLS) em Rust
- [ ] IMAP4rev2 server (porta 993 IMAPS) em Rust
- [ ] Sieve filters básicos (entrega, pasta, rejeitar)
- [ ] Anti-spam: Rspamd integration
- [ ] Anti-malware: ClamAV integration
- [ ] DKIM signing automático por domínio
- [ ] SPF + DMARC validation na entrada
- [ ] Armazenamento de mensagens no MinIO

### Sprint 5–6 (Semanas 9–12): WebMail (SvelteKit)
- [ ] UI SvelteKit: lista de e-mails, leitura, composição
- [ ] Thread view (conversas agrupadas)
- [ ] Inbox rules UI (Sieve)
- [ ] Pesquisa de e-mail (Tantivy)
- [ ] Anexos (upload MinIO, download, preview)
- [ ] Out-of-office (Sieve Vacation)
- [ ] Catálogo de endereços (GAL) com autocomplete

### Sprint 7 (Semanas 13–14): Auth + gov.br
- [ ] Keycloak setup + realm config
- [ ] gov.br OIDC adapter (sso.acesso.gov.br)
- [ ] Login WebMail via OIDC
- [ ] MFA: TOTP + WebAuthn
- [ ] RBAC: SuperAdmin, TenantAdmin, User, Readonly
- [ ] Audit log de autenticação

---

## Fase 2 — Calendar + Contacts (8 semanas)

### Sprint 8–9 (Semanas 1–4): CalDAV
- [ ] CalDAV server (RFC 4791) em Rust
- [ ] Calendário pessoal: CRUD events
- [ ] Recurrence rules (RFC 5545 RRULE)
- [ ] Salas de reunião (resource mailboxes)
- [ ] Scheduling: free/busy lookup (RFC 6638)
- [ ] Calendário compartilhado
- [ ] iCal export/import

### Sprint 10–11 (Semanas 5–8): CardDAV + UI Calendar
- [ ] CardDAV server (RFC 6352) em Rust
- [ ] vCard 4.0 import/export
- [ ] Sincronização GAL → contatos pessoais
- [ ] UI: calendar view (mês/semana/dia/agenda)
- [ ] UI: criar/editar/excluir eventos com convites
- [ ] Email de convite (iTIP, RFC 5546)
- [ ] RSVP handling (accept/decline/tentative)

---

## Fase 3 — Drive + Office Online (12 semanas)

### Sprint 12–14 (Semanas 1–6): Drive
- [ ] WebDAV server (RFC 4918) em Rust
- [ ] Upload tus.io resumable (arquivos grandes)
- [ ] Versionamento de arquivos
- [ ] Compartilhamento de links (JWT assinado, TTL)
- [ ] Quotas por usuário (enforcement DB + storage)
- [ ] Lixeira + restauração
- [ ] Audit log de acessos a arquivos

### Sprint 15–17 (Semanas 7–12): LibreOffice Online
- [ ] Deploy LibreOffice Online upstream (não Collabora CODE)
- [ ] WOPI bridge em Rust (expresso-wopi)
- [ ] Co-edição Writer, Calc, Impress
- [ ] Preview de documentos (PDF rendering)
- [ ] Integração Drive ↔ LOOL seamless
- [ ] Lock de documento durante edição

---

## Fase 4 — Chat (Matrix) (10 semanas)

### Sprint 18–20 (Semanas 1–6): Matrix Homeserver
- [ ] Deploy Conduwuit (Matrix homeserver em Rust)
- [ ] Integração SSO Keycloak ↔ Matrix
- [ ] Canais (rooms) por workspace/departamento
- [ ] Mensagens diretas E2EE
- [ ] Reactions, threads, edição, exclusão
- [ ] Compartilhamento de arquivos via Drive

### Sprint 21–22 (Semanas 7–10): UI Chat
- [ ] SvelteKit Matrix client (Element-inspirado, mas próprio)
- [ ] Notificações (Web Push)
- [ ] Status de presença
- [ ] Search em mensagens (Tantivy)
- [ ] Mobile PWA chat

---

## Fase 5 — Meet + Admin (12 semanas)

### Sprint 23–25 (Semanas 1–6): Expresso Meet
- [ ] mediasoup SFU server em Node.js/Rust wrapper
- [ ] WebRTC video/audio (VP9/AV1)
- [ ] Screen share
- [ ] Gravação para MinIO
- [ ] Agenda de reuniões (Calendar integration)
- [ ] Salas de espera (lobby)
- [ ] Chat em reunião

### Sprint 26–28 (Semanas 7–12): Admin + Tenant Mgmt
- [ ] Multi-tenant admin dashboard
- [ ] Gerenciamento de usuários (SCIM 2.0)
- [ ] Gerenciamento de domínios
- [ ] Quotas e billing básico
- [ ] Reports de uso (Grafana dashboards)
- [ ] Health dashboard do serviço

---

## Fase 6 — Compliance + AI (16 semanas)

### Sprint 29–32: Compliance Core
- [ ] eDiscovery: busca imutável, exportação MBOX
- [ ] Legal Hold: freeze de mailbox/drive
- [ ] DLP: scan de PII em e-mail e uploads
- [ ] Sensitivity Labels: classificação de dados
- [ ] Portal de direitos LGPD (DSAR self-service)
- [ ] Compliance Manager dashboard

### Sprint 33–36: Expresso AI
- [ ] Deploy Ollama + Llama 3.x no servidor
- [ ] AI resumo de thread de e-mail
- [ ] AI smart reply
- [ ] AI resumo de reunião (Whisper.cpp)
- [ ] Semantic search (embeddings + Qdrant)
- [ ] RAG sobre dados do tenant (Graph API)

---

## Definition of Done (DoD) — Geral

- [ ] Feature implementada com testes unitários (≥ 80% coverage na lógica pura)
- [ ] Testes de integração para endpoints críticos
- [ ] Sem warnings `cargo clippy` / `eslint`
- [ ] OpenTelemetry traces em todos os endpoints
- [ ] Audit log para todas as ações sensíveis
- [ ] Documentação de API (OpenAPI 3.1)
- [ ] Migração de banco documentada e reversível
- [ ] Deploy em ambiente de staging (Proxmox)
- [ ] Performance dentro dos targets definidos na arquitetura

---

## KPIs por Fase

| KPI | Fase 1 | Fase 3 | Fase 5 | Fase 7 |
|-----|--------|--------|--------|--------|
| Usuários suportados (single node) | 100 | 500 | 1.000 | 10.000 |
| Uptime | 99% | 99.5% | 99.9% | 99.95% |
| Latência IMAP p99 | < 200ms | < 100ms | < 50ms | < 50ms |
| Cobertura de testes | 60% | 70% | 80% | 85% |
| Conformidade LGPD itens | 5/15 | 10/15 | 13/15 | 15/15 |

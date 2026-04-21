# Roadmap — Expresso V4

> Roadmap incremental de desenvolvimento por fases e sprints

> Status tracking atualizado em 20 de abril de 2026 para refletir o estado validado no workspace.
> `Baseline validada` = crate compila + smoke test HTTP executado no ambiente local.
> Itens podem estar em estado "scaffold/placeholder" mesmo quando marcados como concluidos.

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

## Status Atual dos Serviços (Repositorio em 20/04/2026)

Legenda: `Implementado` = funcionalidade executavel relevante; `Baseline validada` = crate sobe e respondeu a smoke test HTTP; `Scaffold` = crate/entrypoint existe, mas ainda placeholder; `Planejado` = sem crate funcional no workspace.

| Serviço | Status | Evidência no repo |
|---------|--------|-------------------|
| `expresso-mail` | Implementado (parcial) | HTTP API + SMTP + IMAP core (CAPABILITY, LOGIN, LIST, SELECT, FETCH, STORE, EXPUNGE, CLOSE, LOGOUT, NOOP) |
| `expresso-calendar` | Baseline validada | `GET /health`=200 + `GET /ready`=503 sem DB; bootstrap degradado |
| `expresso-contacts` | Baseline validada | `GET /health` + `GET /ready` JSON validado |
| `expresso-drive` | Baseline validada | `GET /health` + `GET /ready` JSON validado |
| `expresso-wopi` | Baseline validada | `GET /health` + `GET /ready` JSON validado |
| `expresso-auth` | Implementado | OIDC RP completo (PKCE authorization_code, /auth/{login,callback,refresh,logout,me}) validado e2e contra Keycloak 25 |
| `expresso-admin` | Baseline validada | `GET /health` + `GET /ready` JSON validado |
| `expresso-compliance` | Baseline validada | `GET /health` + `GET /ready` JSON validado |
| `expresso-search` | Implementado (parcial) | Tantivy full-text index/search API + integração com mail ingest |
| `expresso-flows` | Baseline validada | `GET /health` + `GET /ready` JSON validado |
| `expresso-notifications` | Baseline validada | `GET /health` + `GET /ready` JSON validado |
| `expresso-chat` | Implementado (parcial) | Synapse bridge + canais/mensagens + JWT strict (Authorization: Bearer) validado e2e |
| `expresso-meet` | Implementado (parcial) | Reuniões + Jitsi JWT issuance + JWT strict validado e2e |

---

## Fase 1 — Expresso Mail MVP (14 semanas)

### Sprint 1–2 (Semanas 1–4): Infraestrutura Base
- [x] Monorepo scaffold (Cargo workspace + pnpm workspace)
- [ ] Dockerfile para Debian 13 base image
- [x] Docker Compose: PostgreSQL 16, Redis 7, MinIO
- [x] Migration engine (sqlx migrations)
- [x] Schema inicial: tenants, users, mailboxes, messages
- [x] CI/CD pipeline (GitHub Actions ou Gitea CI)
- [x] Observabilidade inicial: tracing + Prometheus

### Sprint 3–4 (Semanas 5–8): SMTP + IMAP Server
- [ ] SMTP server (porta 25 + 587 STARTTLS) em Rust
- [x] IMAP4rev1 core commands (CAPABILITY, LOGIN, LIST, SELECT, FETCH, STORE, EXPUNGE, CLOSE, LOGOUT, NOOP) — imap-codec
- [ ] Sieve filters básicos (entrega, pasta, rejeitar)
- [ ] Anti-spam: Rspamd integration
- [ ] Anti-malware: ClamAV integration
- [ ] DKIM signing automático por domínio
- [ ] SPF + DMARC validation na entrada
- [x] Armazenamento de mensagens no MinIO — S3 ObjectStore + fallback FS

### Sprint 5–6 (Semanas 9–12): WebMail (SvelteKit)
- [ ] UI SvelteKit: lista de e-mails, leitura, composição
- [ ] Thread view (conversas agrupadas)
- [ ] Inbox rules UI (Sieve)
- [x] Pesquisa de e-mail (Tantivy) — expresso-search + mail ingest integration
- [x] Anexos — attachment list/download API (MIME parsing via mail-parser) + S3 client lib
- [x] Out-of-office (Sieve Vacation) — /api/v1/mail/vacation PUT/GET; user_vacation table; sieve gerado server-side (integração c/ delivery loop pendente)
- [x] Catálogo de endereços (GAL) com autocomplete — /api/v1/gal/search (diretório + contatos do usuário via ILIKE)

### Sprint 7 (Semanas 13–14): Auth + gov.br
- [x] Keycloak setup + realm config (deploy/keycloak/seed-realm.sh)
- [x] gov.br OIDC adapter (sso.acesso.gov.br) — Keycloak external IdP (seed-realm §10, condicional GOVBR_CLIENT_ID/SECRET), IdP mappers sub→govbr_cpf_hash + confiabilidades, claims propagados p/ access_token, GovbrFederation::from_ctx extrai metadata, audit log auth.federation.govbr no callback, migration govbr_user_map (provisioning admin segue pendente)
- [x] OIDC RP backend (expresso-auth: PKCE+/auth/me) + SPA cookie-session wiring (frontend/expresso-web: /login, /me, layout guard, vite proxy)
- [x] MFA: TOTP + WebAuthn (Keycloak required actions + realm WebAuthnPolicy ES256/RS256; /auth/me expõe mfa.totp/webauthn/acr/amr via RFC 8176)
- [x] RBAC: realm roles SuperAdmin/TenantAdmin/User/Readonly + AuthContext::has_any_role helper
- [x] Audit log de autenticação (tracing target="audit" JSON: login.start, login.success, token.refreshed, logout, login.rate_limited) + rate-limit /auth/login (20 req/60s)

---

## Fase 2 — Calendar + Contacts (8 semanas)

### Sprint 8–9 (Semanas 1–4): CalDAV
- [ ] CalDAV server (RFC 4791) em Rust
- [ ] Calendário pessoal: CRUD events
- [x] Recurrence rules (RFC 5545 RRULE — subset DAILY/WEEKLY/MONTHLY/YEARLY + BYDAY/COUNT/UNTIL/INTERVAL)
- [ ] Salas de reunião (resource mailboxes)
- [x] Scheduling: free/busy lookup (RFC 6638) — GET /api/v1/scheduling/freebusy (multi-attendee, 370d window cap, RLS-safe tenant+email join)
- [x] Calendário compartilhado (ACL READ/WRITE/ADMIN via POST /calendars/:id/acl)
- [x] iCal export/import — GET /api/v1/calendars/:id/export.ics (VCALENDAR download), POST /api/v1/calendars/:id/import (batch VEVENT upsert with per-event error capture)

### Sprint 10–11 (Semanas 5–8): CardDAV + UI Calendar
- [ ] CardDAV server (RFC 6352) em Rust
- [x] vCard 4.0 import/export — GET /api/v1/addressbooks/:id/export.vcf (text/vcard download), POST /api/v1/addressbooks/:id/import (batch vCard upsert via replace_by_uid)
- [x] Sincronização GAL → contatos pessoais — POST /api/v1/gal/save (user_id ou email → personal addressbook, UID estável dir:<user_id>, idempotente; auto-cria addressbook "Pessoal" se ausente)
- [ ] UI: calendar view (mês/semana/dia/agenda)
- [ ] UI: criar/editar/excluir eventos com convites
- [x] Email de convite (iTIP, RFC 5546)
- [x] RSVP handling (accept/decline/tentative)

---

## Fase 3 — Drive + Office Online (12 semanas)

### Sprint 12–14 (Semanas 1–6): Drive
- [ ] WebDAV server (RFC 4918) em Rust
- [x] Drive REST API scaffold (files CRUD + upload/download + soft-delete, migração drive_files com RLS)
- [ ] Upload tus.io resumable (arquivos grandes)
- [x] Versionamento de arquivos — drive_file_versions (RLS) + auto-archive on overwrite; GET /drive/files/:id/versions + /:v download
- [x] Compartilhamento de links (token 32B base64url + sha256 persistido, TTL default 7d/max 30d, revoke por id, público /drive/share/:token)
- [x] Quotas por tenant — drive_quotas + fn drive_quota_used (soma files vivos + versões); default 10 GB; enforce no upload (507)
- [x] Lixeira + restauração — GET /drive/trash, POST /drive/files/:id/restore, DELETE ?permanent=true (purge blob+row)
- [x] Audit log de acessos — target='audit' em upload/version/download/share/trash/restore/purge

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
- [x] Integração JWT Keycloak ↔ chat (RS256 via JWKS) — bridge SSO→Matrix application_service ativo
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

# Changelog

## [Sprint #46] — 2026-04-24 — UI web multi-tenant

### Added
- `expresso-auth-rp`: `TenantProviderCache` (lazy OIDC discovery per realm)
  resolvendo `authorization_endpoint`/`token_endpoint`/`end_session_endpoint`
  via Host header — matching backend multi-realm flow.
- Config `AUTH_RP__ISSUER_TEMPLATE`, `AUTH_RP__REDIRECT_URI_TEMPLATE`,
  `AUTH_RP__POST_LOGOUT_TEMPLATE` (placeholders `{realm}` + `{host}`).
- `PendingLogin.realm` + `PendingLogin.redirect_uri` — pinning per-request.
- Unit tests `oidc::multi_provider::tests` (2 cases).

### Changed
- `expresso-web` env in prod: `PUBLIC__AUTH_LOGIN=/auth/login` (relative),
  `PUBLIC__WEB_BASE_URL=""` → browser usa Host atual em todo fluxo OIDC.
- `compose-auth-rp.yaml` → `image: expresso-auth:fase46` + 3 env vars novas.

### Verified
- 5/5 smoke probes web (pilot, pilot2, fallback expresso.local).
- 14/14 backend DAV probes (pilot+pilot2) PASS pós-deploy.
- Keycloak aceita `redirect_uri=https://{pilot,pilot2}.expresso.local/auth/callback`.


Todas as mudanças notáveis. Formato baseado em [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versionamento: sprints numerados sequencialmente (sem semver atualmente; workspace interno).

## [Unreleased]

## Sprint #45 — 2026-04-24 — Multi-realm rollout COMPLETO (7/7)

### Added
- `libs/expresso-auth-client`: `OidcConfig.audiences()` / `primary_audience()` — suporte CSV a `AUTH__OIDC_AUDIENCE` (multi-audience JWT validation).
- 5 unit tests novos (`validator::tests::*`) cobrindo single/multi/trim/empty/fallback.
- `ops/smoke-chat-meet.sh`: smoke E2E chat + meet por tenant.
- `ops/smoke-dav.sh`: estendido para 7 probes (JWT + calendar + contacts + drive + mail + chat + meet).
- `ops/tenant-add.sh`: helper checklist onboarding novo tenant multi-realm.
- `docs/MULTI-REALM-JWT.md`: guia arquitetural (componentes, config, troubleshooting).
- `deny.toml` + CI job `cargo deny check bans sources`.
- CI jobs: `shellcheck` (ops/*.sh), `cargo doc` build-check.
- `publish = false` em todos workspace crates (27 Cargo.toml).

### Changed
- Chat + Meet: rebuild `:fase45`, compose env com `AUTH__OIDC_AUDIENCE="account,expresso-web"`, `AUTH__OIDC_ISSUER_TEMPLATE`, `AUTH__TENANT_HOSTS`, `extra_hosts`.
- `README.md`: links para MULTI-REALM-JWT + TENANT-ONBOARDING + OBSERVABILITY.

### Fixed
- Migração `20260423180000_audit_log.sql`: idempotente via `ALTER TABLE ADD COLUMN IF NOT EXISTS` (antes falhava em bases com schema antigo).

### Known Limitations
- UI web (`expresso-web` + `expresso-nginx`) permanece single-realm: rollout
  multi-realm cobre apenas backends API/DAV. Browser login via
  `pilot.expresso.local` não resolve ainda. Próxima sprint: nginx vhosts
  per-tenant + auth-rp issuer dinâmico. Ver [docs/MULTI-REALM-JWT.md](docs/MULTI-REALM-JWT.md#escopo--apidav-apenas-2026-04-24).

### Production state
7 serviços × 2 tenants (pilot, pilot2) = 14 probes E2E PASS a cada 10min via systemd timer. Prometheus alerts `ExpressoSmokeDav{Failing,Stale}` ativos.

## Sprint #44 — 2026-04-24 — Drive + Mail multi-realm

### Added
- Drive multi-realm (pilot+pilot2) ATIVO.
- Mail multi-realm (pilot+pilot2) ATIVO (após fix migration).
- `ops/smoke-drive.sh`, `ops/smoke-mail.sh`.

## Sprint #43 — 2026-04-24 — Calendar + Contacts multi-realm

### Added
- Calendar + Contacts multi-realm (pilot+pilot2) ATIVOS.
- `ops/smoke-calendar.sh`, `ops/smoke-contacts.sh`, `ops/smoke-multirealm.sh`.
- systemd timers `expresso-smoke-dav@{pilot,pilot2}.timer` (10min interval).
- Prometheus alerts `ExpressoSmokeDavFailing` + `ExpressoSmokeDavStale`.

## Sprint #42 — 2026-04-24 — Chat + Meet multi-realm refactor

### Added
- `libs/expresso-auth-client`: `MultiRealmValidator`, `TenantResolver`.
- Middleware axum por serviço: resolve tenant via header Host, injeta `AuthContext`.
- Chat + Meet refactor para multi-realm (activação em sprint #45 após resolver conflito de audience).

---

Sprints anteriores (#2 → #40b) documentados em [SESSION_HANDOFF.md](SESSION_HANDOFF.md).

# Session Handoff — Expresso v4

**Last session end:** sprint #40b (expresso-imip-dispatch deployed on prod 125, dry-run; Prom+Grafana wired). Working tree clean, pushed to `origin/main`.

## Status da trilha — #2 → #40b shipped (40 sprints)

Pipeline NATS totalmente observável: **produtor → broker → consumidor**.

### Últimos sprints fechados nesta sessão

| # | Commit | Descrição |
|---|--------|-----------|
| 26 | `cd0606a` | `ops/nats/e2e-smoke.sh` — smoke end-to-end JetStream |
| 27 | `5a510b4` | `ops/nats/tail.sh` + `ops/nats/README.md` |
| 28 | `e7856d0` | Novo crate `expresso-event-audit` (consumer JetStream → log JSON) |
| 29 | `abac2be` | `expresso-admin` 2FA enforcement via `ADMIN_REQUIRE_2FA` |
| 30 | `bf0913b` | `expresso-admin` — relatório de cobertura TOTP `/users/totp-status` |
| 31 | `138e44f` | `expresso-event-audit` — `/healthz`, `/readyz`, `/metrics` (prometheus) + `event_audit_events_total{stream}` |
| 32 | `a7b62a5` | Produtores `expresso-calendar` + `expresso-contacts` — `calendar_nats_publish_total{kind,result}` + `contacts_nats_publish_total{kind,result}` (result ∈ ok/err/serialize_err), zero pré-populado |
| 33 | `c6cf4b8` | Grafana dashboard extension — +5 painéis (produtor publish rate, audit consume, lag, errors, contacts JetStream). Artefato-only, sem deploy. |
| 34 | `09afe87` | Prometheus alerting rules — `ops/prometheus/alerts/expresso.yml` (9 rules, 3 groups). Validado com promtool. Artefato-only. |
| 35 | `a18879d` | Observability stack template — alertmanager.yml + prometheus.yml + compose-observability.yaml + README. amtool+promtool validados em 125. Artefato-only. |
| 36 | `d7fad94` | Observability stack deployed em 125 — expresso-prometheus/alertmanager/nats-exporter up. Rules 3 grupos × 9, targets 4 up, 12 séries calendar_nats_publish_total confirmadas. |
| 37 | `6c1e833` | Grafana 13.0.1 provisioned + deployed em 125 — datasource Prometheus + dashboard `expresso-overview` (11 panels) auto-importado. |
| 38 | `2ab025f` | Novo crate `libs/expresso-imip` — iCal REQUEST/CANCEL + MIME multipart builder (RFC 6047). 9 testes unitários. Groundwork p/ iMIP dispatch. |

## Estado em produção (125)

- `expresso-admin:t30` (+`:latest`) porta 8101
- `expresso-calendar:t32` (+`:latest`) porta 8002
- `expresso-contacts:t32` (+`:latest`) porta 8003
- `expresso-event-audit:t31` standalone container, `--network host`, `METRICS_ADDR=0.0.0.0:9191`
- NATS JetStream streams: `EXPRESSO_CALENDAR`, `EXPRESSO_CONTACTS` (max_age 7d)
- `expresso-prometheus` 127.0.0.1:9090 (stack ~/expresso-obs/compose-observability.yaml)
- `expresso-alertmanager` 127.0.0.1:9093 (receivers webhook placeholder)
- `expresso-nats-exporter` (rede expresso_default)
- `expresso-grafana` 127.0.0.1:3000 (v13.0.1, admin/admin default)
- Consumer durables: `event-audit-expresso_calendar`, `event-audit-expresso_contacts` (`DeliverPolicy::New`)

## Infra / acessos (não commitar)

- Jump host: `sshpass -p 'tbrn43687' ssh root@192.168.194.101`
- Prod host: `sshpass -p 'ExpressoDev2026' ssh debian@192.168.15.125`
- Compose file: `/home/debian/expresso/compose-phase3.yaml` (root-owned → `sudo cp` a partir de `/tmp/`)
- KC admin: `admin` / `Expr3ss0_KC_2026!`
- Test tenant: `40894092-7ec5-4693-94f0-afb1c7fb51c4`
- CalDAV: client_id `expresso-dav`, secret `zMU4ENzuNqKGU0pS5JYTvaw4vwAgLD0i`
- NATS dentro do container: `172.17.0.1:4222` (client) / `8222` (monitoring)

## Pipeline de build (101 → 125)

```bash
# Build no 101 (cargo com cache):
cd /root/expresso-build && docker run --rm -v $PWD:/src -w /src \
  -v /root/cargo-cache:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/src/.target rust:1-bookworm bash -c \
  'apt-get install mold clang build-essential pkg-config libssl-dev libpq-dev >/dev/null && \
   cargo build -p <pkg> --release'

# Dockerfiles quick estão em /root/expresso-build/Dockerfile.<svc>.quick
# Esperam binário <svc>.bin — cp .target/release/<svc> <svc>.bin antes de build
```

## Quirks conhecidos

- `IntCounterVec` só aparece em `/metrics` após primeiro `.with_label_values().inc()`. `Lazy::force` sozinho **não basta**. Pré-popular com `inc_by(0)` em todas as permutações `{kind, result}` no startup.
- `Dockerfile.<svc>.quick` espera binário em `<svc>.bin` (com `.bin`).
- `get_by_uid()` retorna `Result<Event>` (não Option) — usar `.ok().map(|e| e.id)`.
- Edits em `/home/debian/expresso/*.yaml` exigem `sudo cp` a partir de `/tmp/`.
- `askama_escape::escape` não exposto → inline `esc()` p/ `&<>"`.
- Build warnings tolerados: `TokenResp.access_token` (auth-client), `CounterProposal`/`CounterRepo` (calendar).
- Rate-limit layer order: `.layer(from_fn(ratelimit::layer)).layer(Extension(rate_limiter))`.

## Observability — loop completo

| Camada | Métrica |
|--------|---------|
| Produtor | `calendar_nats_publish_total{kind,result}`, `contacts_nats_publish_total{kind,result}` |
| Broker | `nats_stream_messages{stream}` (via prometheus-nats-exporter, `:8222`) |
| Consumidor | `event_audit_events_total{stream}` |

**Lag PromQL:**
```
sum(rate(calendar_nats_publish_total{result="ok"}[5m]))
  - sum(rate(event_audit_events_total{stream="EXPRESSO_CALENDAR"}[5m]))
```

## Próximos sprints candidatos (não iniciados)

2. **iMIP dispatch** — estender `expresso-event-audit` (ou novo crate) com SMTP via `lettre` + remontagem iCal p/ attendees em `event_created/updated/cancelled`. Escopo grande (~2h), exige config SMTP.
3. **Realm-per-tenant wizard** — KC admin REST p/ criar realm isolado por tenant. Escopo grande.

## Workflow TaskSync (retomada)

Sprint loop: **implement → build 101 → ship 125 → smoke → doc → commit → push → `ask_user`**.
Session ID usado: `"12"`. User costuma responder `"segue"` / `"autonomous temporary"` / nº próximo sprint.

## Arquivos relevantes

- Roadmap: [ROADMAP_DEPLOYMENT_STATUS.md](ROADMAP_DEPLOYMENT_STATUS.md) (seções #2→#32)
- Ops NATS: [ops/nats/](ops/nats/) (smoke.sh, e2e-smoke.sh, tail.sh, README.md)
- Grafana: [ops/grafana/expresso-overview.json](ops/grafana/expresso-overview.json) (11 painéis, inclui counters #31/#32 a partir de #33)
- Observability lib: [libs/expresso-observability](libs/expresso-observability)
- Event audit worker: [services/expresso-event-audit](services/expresso-event-audit)

## Sprint #40c-fase1 — expresso-tenant-provision (realm-per-tenant Phase 1)

Standalone CLI tool that idempotently provisions a full Keycloak realm per tenant.

**Crate**: `services/expresso-tenant-provision/` (new, 478-line main.rs, clap 4 derive).

**Provisions** (idempotent — skip if exists):
- Realm with security defaults: sslRequired=external, bruteForceProtected, passwordPolicy length(12)+upper+digit+history(3)
- 3 clients: `expresso-web` (public, PKCE S256), `expresso-dav` (confidential, directAccessGrants), `expresso-admin` (confidential, serviceAccountsEnabled)
- `tenant_id` hardcoded-claim protocol mapper (realm name = tenant_id) on all 3 clients → access/id/userinfo tokens
- 4 realm roles: SuperAdmin, TenantAdmin, User, Readonly
- Initial admin user with temp password + TenantAdmin role assignment

**Flags**: `--kc-url --realm --display --admin-email --admin-password --base-redirect --dry-run` (env: KC_ADMIN_PASS, TENANT_ADMIN_EMAIL, TENANT_ADMIN_PASSWORD).

**Dry-run**: fetches admin token + existence GETs but never POSTs; prints realm body and returns full summary. Validated end-to-end on prod 125 against `expresso-keycloak` — returns:
```json
{ "realm": "tenant-demo", "realm_created": true, "clients_created": ["expresso-web","expresso-dav","expresso-admin"], "roles_created": ["SuperAdmin","TenantAdmin","User","Readonly"], "admin_user_id": "(dry-run)", "dry_run": true }
```

**Tests**: 7 unit tests on pure body builders — all pass. Isolated workspace `/tmp/tp-test` on 125 used for `cargo test -p expresso-tenant-provision`.

**Deploy state**: artifact-only. No compose service, no systemd. Operator runs binary manually when provisioning a new tenant.

**Not yet** (future fases of realm-per-tenant plan):
- Fase 2 — services resolve realm via Host header (expresso-web/dav/admin → lookup realm from tenant domain map)
- Fase 3 — migration script: move existing `expresso` realm users into per-tenant realms
- Fase 4 — drop `tenant_id` user attribute; source of truth = realm

**Last session end**: sprint #40c-fase1 (expresso-tenant-provision CLI; dry-run validated on prod 125).

---

## Sessão atual — shipped

Trilha `realm-per-tenant` completa (#40c fase2→fase4) + iMIP habilitado em prod (#41).

| # | Commit | Descrição |
|---|--------|-----------|
| 40c-fase2-lib | `892fcda` | `expresso-auth-client`: `MultiRealmValidator` + resolver por Host header |
| 40c-fase2-lib | `aba8b94` | `TenantAuthenticated` extractor axum (resolve realm → valida token) |
| 40c-fase2-step2 | `e46a888` | Wire chat+meet: flag `AUTH__OIDC_ISSUER_TEMPLATE` + `AUTH__TENANT_HOSTS` (compat single-realm default) |
| 40c-fase2-step2 | `37f1c03` | Wire calendar+auth-rp (mesmo padrão) |
| 40c-fase3 | `993342f` | Novo crate `expresso-tenant-migrate` (CLI migra users de realm legado → realms per-tenant, flag `--send-reset`, dry-run default, 5 testes) |
| 40c-fase4 | `997c522` | Drop mapper hardcoded `tenant_id`: `AuthContext::from_raw` deriva do `iss` via `realm_from_iss`. Fallback p/ claim legado. 22 testes auth-client. |
| 40c ops | `d845765` | `ops/deploy-fase2-notes.md` — runbook de deploy fase2 |
| 40c cleanup | `22df1e4` | remove `tracing::warn` não usado |
| 41 | `eabe16c` | IMIP_ENABLED=true em compose-phase3 (SMTP_HOST=expresso-postfix:25, STARTTLS=false, FROM=calendar@expresso.local) |
| 40c-fase4 ops | `d7e1a1e` | `ops/keycloak/remove-hardcoded-tenant-mapper.sh` — script idempotente limpa mapper legado |
| docs | `22c8b84` | `docs/TENANT-ONBOARDING.md` — runbook 7 seções (provision→migrate→cleanup→multi-realm→iMIP→rollback) |
| ops/alerts | `c7e082b` | 3 regras Prometheus iMIP (`ExpressoImipSendErrors`/`ParseErrors`/`DispatcherDown`), aplicadas em prod via `/-/reload` |

### Deploy fase2 em prod 125 (binários ativos)

- `expresso-chat:fase2` (`1f90c794e63f`) + tag `:latest`, preserva `:pre-fase2`
- `expresso-meet:fase2` (`3cb8b4acee3c`)
- `expresso-calendar:fase2` (`08be07e3069f`)
- `expresso-auth:fase2` (`d167dcc66d8e`)

Símbolos confirmados no binário (strings): `AUTH__OIDC_ISSUER_TEMPLATE`, `MultiRealmValidator`, `multi-realm validator ready`.

**Modo atual**: compat single-realm (sem `AUTH__OIDC_ISSUER_TEMPLATE` setado → fallback p/ legacy `AUTH__OIDC_ISSUER`). Para ativar multi-realm, setar template + `AUTH__TENANT_HOSTS` no compose.

### iMIP end-to-end validado

- NATS publish envelope REQUEST → `imip-dispatch` log `sent`
- Postfix smtpd recebe da 172.19.0.27 → LMTP → dovecot `status=sent delivered`
- Métrica `imip_dispatch_total{method="REQUEST",result="ok"} = 1`
- Alerts state: 3/3 `inactive` (dispatcher saudável)

**Quirk envelope**: `method` é UPPERCASE `REQUEST`|`CANCEL`. Campos `dtstart`/`dtend` (não `start_utc`), `organizer_cn`, `common_name` (não `name`).

### Realm cleanup em prod

`ops/keycloak/remove-hardcoded-tenant-mapper.sh` executado contra realm `expresso` (prod 125): 2 clients (`web`, `dav`), zero mappers hardcoded removidos — realm já compliant com fase4.

**Session ID TaskSync**: `"14"`.

## 2026-04-24 — Fase 10 concluída (pilot multi-realm ativo + per-tenant metrics)

- `bf870e7` ops/pilot-multirealm-activation.md — runbook + format gotchas
- `29956b7` per-tenant auth metrics: `auth_validation_total{realm,result}` + `auth_realm_cache_size`
- `9e0606a` Grafana 3 panels (200-202) + 2 Prometheus alerts (ExpressoAuthValidationErrorsHigh, ExpressoAuthNoRealmsLoaded)

**Prod state (125):**
- auth-rp + calendar rodando image `:fase10b` com multi-realm + metrics wired
- Prometheus scrape `expresso-auth-rp:8012` healthy
- Grafana `expresso-overview.json` reloaded (19 panels)
- Alerts: 14 rules SUCCESS promtool

**E2E validated:**
- `GET /auth/me` com Host=pilot.expresso.local → 200 `tenant_id=30aa38fd-...`
- Prom query `auth_validation_total{realm="30aa38fd-...",result="ok"}=2`
- Prom query `auth_realm_cache_size=1`

**Gotchas documentados:**
- `AUTH__TENANT_HOSTS` sep = `:` (≠ `=`); format `host:realm,host2:realm2`
- Template placeholder = `{realm}` (≠ `{tenant}`)
- Audience default KC direct-grant = `account`
- Prometheus bind-mount: file replace via cp → inode muda → precisa `docker restart` (reload não basta)
- expresso-observability Registry: migrado p/ `prometheus::default_registry()` → metrics de libs (auth-client) aparecem no /metrics

## 2026-04-24 — Fase 10e→10h concluída (2-tenant scale + smoke-as-metric + systemd template)

| Fase | Commit | Descrição |
|------|--------|-----------|
| 10d | `7b32d9a` | `ops/smoke-multirealm.sh` — JWT + /auth/me + Prom metric check (exit 0/1/2/3) |
| 10e | `8ff2863` | Scale validation: pilot2 realm `3b11c7a2-...` provisioned + TENANT_HOSTS hot-swap (hosts:2) |
| 10f | `4755199` | systemd unit legacy + timer (OnUnitActiveSec=10min) |
| 10g | `e9360e2` | Pushgateway + trap EXIT emit `expresso_smoke_multirealm_{success,last_run_timestamp_seconds}` + 2 alerts (Failing, Stale) |
| 10h | `14b951e` | Template `expresso-smoke-multirealm@.{service,timer}` — per-tenant instance + RandomizedDelaySec=60s |

### Prod state 125 adicional

- `expresso-pushgateway` 127.0.0.1:9091 (persist pg-data)
- Prometheus job `pushgateway` (`honor_labels: true`) healthy
- systemd timers ativos:
  - `expresso-smoke-multirealm@pilot.timer`
  - `expresso-smoke-multirealm@pilot2.timer`
  - legacy `expresso-smoke-multirealm.timer` → disabled
- `/etc/expresso/smoke-multirealm-{pilot,pilot2}.env` (mode 600)

### E2E validated 2-tenant

- pilot (realm `30aa38fd-...`) smoke count=5+ → PASS → push → Prom scrape → `expresso_smoke_multirealm_success{tenant="30aa38fd-..."}=1`
- pilot2 (realm `3b11c7a2-...`) smoke count=2+ → PASS → push → `expresso_smoke_multirealm_success{tenant="3b11c7a2-..."}=1`
- Loop observabilidade fechado: falha → pushgateway → Prom → AlertManager (rules `ExpressoSmokeMultirealmFailing` + `ExpressoSmokeMultirealmStale`)

### Sprint #40c FINAL — 10 fases shipped

```
10a bf870e7  runbook activation
10b 29956b7  per-tenant auth metrics (lib)
10c 9e0606a  Grafana + Prom alerts
10d 7b32d9a  smoke script
10e 8ff2863  2-tenant scale (pilot+pilot2)
10f 4755199  systemd timer
10g e9360e2  pushgateway + smoke-as-metric + staleness alert
10h 14b951e  systemd template per-tenant + RandomizedDelaySec
```

### Credenciais pilot2 (não-commit)

- Realm UUID: `3b11c7a2-44d1-4935-963b-ba622b70786a`
- Admin: `admin@pilot2.expresso.local` / `PilotAdmin!2026Strong`
- DAV secret: `dav-pilot2-secret-2026-strong`
- Host: `pilot2.expresso.local`

### Gotchas adicionais desta janela

- compose volumes: edits via python precisam inserir chave dentro do bloco `volumes:` — append no final gera seção órfã sob `networks:`. Sempre validar com `docker compose config --quiet`.
- pushgateway scrape: `honor_labels: true` é essencial p/ preservar `job=smoke_multirealm` + `tenant=<uuid>` originais; senão Prometheus sobrescreve.
- Pushgateway POST grouping key: `/metrics/job/<job>/tenant/<uuid>` — cada grupo é idempotente (POST replace, DELETE remove).
- `set -euo pipefail` + `trap EXIT` + `|| true` no push garante que falhas de rede no pushgateway não mascarem exit code do teste.

## 2026-04-24 — Sprint #42: chat + meet RequestCtx multi-realm (ship)

| Step | Commit/Tag | Descrição |
|------|------------|-----------|
| code | `1d562c5` | chat/meet `RequestCtx` → MultiRealmValidator precedence + per-tenant `auth_validation_total` |
| build | image `expresso-{chat,meet}:fase42` | rust:1-bookworm release build (1m49s) em 101 |
| deploy | prod 125 | `compose-chat-meet.yaml up -d --force-recreate` — containers up |

### Runtime prod 125 (pós-recreate)

- `expresso-chat` :8010 up, HTTP listening
- `expresso-meet` :8011 up, HTTP listening
- Compat fallback: sem `AUTH__OIDC_ISSUER_TEMPLATE` no compose → mesmo path single-realm legacy (erro JWKS `auth.expresso.local` → dev header mode — pre-existente, ≠ regressão)
- strings binário: `MultiRealmValidator` + `AUTH__OIDC_ISSUER_TEMPLATE` + `AUTH__TENANT_HOSTS` presentes

### Próximos passos candidatos

- Ativar multi-realm chat/meet: KC clients `expresso-chat` + `expresso-meet` em pilot/pilot2 realms + env `AUTH__OIDC_ISSUER_TEMPLATE` + `AUTH__TENANT_HOSTS` + `extra_hosts` em compose-chat-meet.yaml
- Aplicar mesmo pattern em calendar + contacts (próximos serviços tenant-aware)
- Smoke test chat endpoint com JWT pilot

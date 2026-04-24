# Session Handoff — Expresso v4

**Last session end:** sprint #37 (Grafana provisioned + deployed). Working tree clean, pushed to `origin/main`.

## Status da trilha — #2 → #37 shipped (36 sprints)

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

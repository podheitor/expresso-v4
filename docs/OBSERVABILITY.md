# Observability — Prometheus + Grafana

## Arquitetura

```
Cada serviço Rust → GET /metrics (text/plain; version=0.0.4)
                         ↓
                  expresso-prometheus:9090  (scrape a cada 15s)
                         ↓
                  expresso-grafana:3000     (datasource auto-provisionada)
                         ↓
                  Dashboard "Expresso — Overview"
```

## Exposição `/metrics`

Lib compartilhada [libs/expresso-observability/src/lib.rs](../libs/expresso-observability/src/lib.rs):
- `metrics_router<S>() -> Router<S>` — monta `GET /metrics`
- `registry()` — acesso à `Registry` global (registrar métricas customizadas)
- `register(metric)` — helper que registra + retorna clone
- `HTTP_REQUESTS_TOTAL` — contador built-in (opt-in via middleware)

### Integração em um serviço

```rust
// src/api/mod.rs
use expresso_observability::metrics_router;

pub fn router(state: AppState) -> Router {
    Router::new()
        .merge(health::routes())
        .merge(metrics_router())   // ← GET /metrics
        // ...
}
```

## Cobertura — 15/15 serviços

| Serviço          | Porta   | Endpoint /metrics        |
|------------------|---------|--------------------------|
| expresso-mail    | 8001    | `http://expresso-mail:8001/metrics`    |
| expresso-chat    | 8004    | `http://expresso-chat:8004/metrics`    |
| expresso-meet    | 8011    | `http://expresso-meet:8011/metrics`    |
| expresso-calendar| 8002    | `http://expresso-calendar:8002/metrics`|
| expresso-contacts| 8003    | `http://expresso-contacts:8003/metrics`|
| expresso-drive   | 8004    | `http://expresso-drive:8004/metrics`   |
| expresso-auth    | 8100    | `http://expresso-auth:8100/metrics`    |
| expresso-admin   | 8101    | `http://expresso-admin:8101/metrics`   |
| expresso-compliance | 8009 | `http://expresso-compliance:8009/metrics` |
| expresso-search  | 8007    | `http://expresso-search:8007/metrics`  |
| expresso-wopi    | 8008    | `http://expresso-wopi:8008/metrics`    |
| expresso-web     | 8080    | `http://expresso-web:8080/metrics`     |
| expresso-notifications | 8006 | `http://expresso-notifications:8006/metrics` |
| expresso-flows   | 8005    | `http://expresso-flows:8005/metrics`   |
| expresso-milter  | 9091    | `http://expresso-milter:9091/metrics` (porta auxiliar; 8891 = milter) |

## Deploy

### Prometheus + Grafana

Stack já definida em [deploy/docker/compose.yaml](../deploy/docker/compose.yaml):

```bash
cd deploy/docker
docker compose up -d prometheus grafana
```

- Prometheus UI: <http://localhost:9090>
- Grafana UI: <http://localhost:3001> (admin/admin)

### Provisionamento Grafana

- [deploy/docker/grafana/provisioning/datasources/prometheus.yaml](../deploy/docker/grafana/provisioning/datasources/prometheus.yaml) — datasource `Prometheus` apontando para `http://expresso-prometheus:9090`
- [deploy/docker/grafana/provisioning/dashboards/default.yaml](../deploy/docker/grafana/provisioning/dashboards/default.yaml) — provider lê `/var/lib/grafana/dashboards/*.json`
- [deploy/docker/grafana/dashboards/expresso-overview.json](../deploy/docker/grafana/dashboards/expresso-overview.json) — dashboard "Expresso — Overview":
  - Services UP (stat)
  - Up status per service (timeseries)
  - HTTP requests rate (5m) por serviço
  - CPU seconds rate
  - Memory RSS

## Adicionar métricas customizadas

```rust
use once_cell::sync::Lazy;
use prometheus::IntCounter;
use expresso_observability as obs;

static MESSAGES_DELIVERED: Lazy<IntCounter> = Lazy::new(|| {
    obs::register(IntCounter::new("mail_messages_delivered_total",
                                   "Messages successfully delivered via LMTP").unwrap())
});

// No código de entrega:
MESSAGES_DELIVERED.inc();
```

Métrica aparece em `/metrics` automaticamente, sem config adicional no Prometheus (já está coberta pelo job do serviço).

## Validação E2E (VM 192.168.15.125)

```bash
$ curl http://localhost:8001/health
{"service":"expresso-mail","status":"ok"}

$ curl http://localhost:8001/ready
{"db":true,"status":"ready"}

$ curl -I http://localhost:8001/metrics
HTTP/1.1 200 OK
content-type: text/plain; version=0.0.4
```

Status: **Phase 1 (health) + Phase 2 (metrics) + Phase 3 (stack)** concluídos. Commit `871b49a`.

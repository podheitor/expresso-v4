# Grafana dashboards — Expresso

## Importar
1. Grafana → Dashboards → Import → Upload JSON file.
2. Selecione `expresso-overview.json`.
3. Escolha o datasource Prometheus (default uid=`prometheus`).

## Painéis
- **HTTP req/s por serviço** — `rate(http_requests_total[1m])` agrupado por `service` (label emitido pelo middleware `expresso_observability::http_counter_mw`).
- **HTTP 4xx/5xx** — erros server-side segmentados por serviço.
- **429 Rate-limited (5m)** — hits do rate limiter per-tenant (sprint #11).
- **Status mix** — distribuição global de status codes.
- **JetStream EXPRESSO_CALENDAR** — exige `prometheus-nats-exporter` rodando contra `http://expresso-nats:8222/varz|/jsz`.
- **/readyz up** — contagem de serviços com `up=1` no job `expresso-*`.

## Scrape config exemplo

```yaml
scrape_configs:
  - job_name: expresso-services
    static_configs:
      - targets:
          - expresso-calendar:8002
          - expresso-contacts:8003
          - expresso-admin:8101
          - expresso-auth:8012
    metrics_path: /metrics
  - job_name: nats
    static_configs:
      - targets: [prometheus-nats-exporter:7777]
```

## Sprint trilha
Entregue em #21 (dashboards) — artefato JSON, nenhum deploy adicional.

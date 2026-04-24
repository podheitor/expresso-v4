# Pushgateway deploy — prod 125

Added to observability stack p/ coletar métricas de jobs oneshot (systemd timers),
especificamente o smoke multi-realm.

## Estado prod (2026-04-24)

- Container: `expresso-pushgateway` (prom/pushgateway:latest)
- Bind: 127.0.0.1:9091
- Persistência: volume `pg-data` → /data/pg.data (flush 5min)
- Rede: `expresso_default` (descoberto pelo Prometheus como `expresso-pushgateway:9091`)

## Scrape config (prometheus.yml)

```yaml
- job_name: pushgateway
  honor_labels: true   # preserva labels originais do job (tenant, job_name)
  static_configs:
    - targets: ['expresso-pushgateway:9091']
```

## Smoke integration

`smoke-multirealm.sh` emite 2 gauges na saída (trap EXIT):

| Métrica | Tipo | Labels | Descrição |
|---------|------|--------|-----------|
| `expresso_smoke_multirealm_success` | gauge | tenant, job | 1=PASS, 0=FAIL |
| `expresso_smoke_multirealm_last_run_timestamp_seconds` | gauge | tenant, job | unix ts última execução |

Env necessário em `/etc/expresso/smoke-multirealm.env`:

```
PUSHGATEWAY_URL=http://127.0.0.1:9091
```

Push endpoint: `POST $PUSHGATEWAY_URL/metrics/job/smoke_multirealm/tenant/<realm-uuid>`

## Alerting

Regras em `ops/prometheus/alerts/expresso.yml` grupo `expresso-smoke-multirealm`:
cobrem falha do smoke e staleness (timer parado ou pushgateway fora).

## Rollback

```bash
sudo docker compose -f /home/debian/expresso-obs/compose-observability.yaml stop expresso-pushgateway
sudo docker compose -f /home/debian/expresso-obs/compose-observability.yaml rm -f expresso-pushgateway
# Remover job pushgateway do prometheus.yml + restart prometheus
```

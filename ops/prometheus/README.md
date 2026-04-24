# Prometheus config — Expresso V4

## Alerting rules

Arquivo: [alerts/expresso.yml](alerts/expresso.yml). 9 regras organizadas em 3
grupos:

### `expresso-nats-pipeline` (5 regras)
| Alert | Severity | Trigger |
|-------|----------|---------|
| `ExpressoNatsPublishErrors` | warning | publish err rate > 0.1/s por 5m |
| `ExpressoNatsPublishErrorsCritical` | critical | publish err rate > 1/s por 5m |
| `ExpressoEventAuditLagCalendar` | warning | publish(ok) − audit > 0.5/s por 10m |
| `ExpressoEventAuditLagContacts` | warning | idem contacts |
| `ExpressoEventAuditSilent` | critical | audit rate=0 + publishers ativos 15m |

### `expresso-service-health` (3 regras)
| Alert | Severity | Trigger |
|-------|----------|---------|
| `ExpressoServiceDown` | critical | `up{job=~"expresso-.*"}=0` por 2m |
| `ExpressoHttp5xxHigh` | warning | 5xx rate > 0.5/s por 5m |
| `ExpressoRateLimitedSpike` | info | 429 rate > 1/s por 10m |

### `expresso-nats-broker` (1 regra)
| Alert | Severity | Trigger |
|-------|----------|---------|
| `ExpressoJetStreamStalled` | warning | `nats_stream_messages` estável 10m + publishers ativos |

## Integração

Editar `prometheus.yml` e anexar:

```yaml
rule_files:
  - /etc/prometheus/alerts/expresso.yml

alerting:
  alertmanagers:
    - static_configs:
        - targets: ['alertmanager:9093']
```

Mount: `-v $PWD/ops/prometheus/alerts:/etc/prometheus/alerts:ro`.

## Validação

```bash
docker run --rm --entrypoint promtool \
  -v $PWD/ops/prometheus/alerts/expresso.yml:/w/expresso-alerts.yml \
  prom/prometheus:latest check rules /w/expresso-alerts.yml
# SUCCESS: 9 rules found
```

## Sprint trilha
Entregue em #34 — derivado dos counters #31 (event-audit) + #32 (publishers).
Artefato-only (sem rebuild de serviços).

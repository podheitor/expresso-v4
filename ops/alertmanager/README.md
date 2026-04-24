# Alertmanager — Expresso V4

Template de configuração para consumir as regras shipadas em #34
(`ops/prometheus/alerts/expresso.yml`).

## Arquivos

- [alertmanager.yml](alertmanager.yml) — rota + receivers + inhibit rules.
- Regras: [../prometheus/alerts/expresso.yml](../prometheus/alerts/expresso.yml).
- Compose: [../compose-observability.yaml](../compose-observability.yaml).

## Roteamento

| Severity | Receiver | group_wait | repeat |
|----------|----------|------------|--------|
| default  | `ops-default` | 30s | 3h |
| critical | `ops-critical` | 30s | 3h (continue:true → também default) |
| info     | `ops-info` | 2m | 24h |

## Inhibit rules

1. `ExpressoServiceDown` (por instance) silencia `component=service|rate-limit`.
2. `ExpressoNatsPublishErrorsCritical` silencia `ExpressoNatsPublishErrors`.

## Personalizar receivers

Webhooks atuais são placeholders `localhost:5001`. Substituir por:

- Slack: `slack_configs:` com `api_url` (secret via env).
- Teams: webhook_config genérico apontando para o connector.
- PagerDuty: `pagerduty_configs:` com `routing_key` para severity=critical.
- Email: `email_configs:` com `smtp_smarthost` (ex: `expresso-mail:587`).

## Deploy

```bash
cd ops/
sudo docker compose -f compose-observability.yaml up -d
# Checks:
curl localhost:9093/-/healthy          # alertmanager
curl localhost:9090/-/ready            # prometheus
curl localhost:9090/api/v1/rules | jq  # 9 regras carregadas
```

## Validação

```bash
docker run --rm --entrypoint amtool \
  -v $PWD/alertmanager.yml:/w/am.yml \
  prom/alertmanager:latest check-config /w/am.yml
# Checking '/w/am.yml' SUCCESS
```

## Sprint trilha
Shipado em #35 (stack completo: prometheus + alertmanager + nats-exporter).
Complementa #34 (rules) + #33 (dashboard) + #32/#31 (counters).

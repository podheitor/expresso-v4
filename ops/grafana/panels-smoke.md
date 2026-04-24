# Smoke multi-realm — Grafana queries

Target dashboard: expresso-overview (ID auto). Add these panels after deploy.

## Panel: Smoke status (stat)

```promql
expresso_smoke_multirealm_success
```
Legend: `{{tenant}}` · Thresholds: 0=red, 1=green · Value mapping: 0→FAIL, 1→PASS

## Panel: Time since last smoke (stat)

```promql
time() - expresso_smoke_multirealm_last_run_timestamp_seconds
```
Unit: seconds · Thresholds: >1800=red, >900=yellow, <900=green

## Panel: Smoke timeline (state timeline)

```promql
expresso_smoke_multirealm_success
```
Per tenant, binary up/down.

## Alerts firing

- `ExpressoSmokeMultirealmFailing` → expresso_smoke_multirealm_success==0 por 15m
- `ExpressoSmokeMultirealmStale` → now − last_run > 30min por 5m

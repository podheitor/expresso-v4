# Load Testing — Expresso V4

Scripts [k6](https://k6.io) para validação de SLA antes de releases.

## Pré-requisitos

```bash
# Instalar k6
# Debian/Ubuntu:
sudo gpg -k && sudo gpg --no-default-keyring \
  --keyring /usr/share/keyrings/k6-archive-keyring.gpg \
  --keyserver hkp://keyserver.ubuntu.com:80 \
  --recv-keys C5AD17C747E3415A3642D57D77C6C491D6AC1D69
echo "deb [signed-by=/usr/share/keyrings/k6-archive-keyring.gpg] https://dl.k6.io/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/k6.list
sudo apt update && sudo apt install k6

# macOS
brew install k6
```

## Executar

```bash
# Smoke (1 VU, 30s — verifica que os endpoints respondem)
BASE_URL=http://localhost:8001 TOKEN=<jwt> k6 run tests/load/mail-smoke.js

# SLA (rampa até 50 VUs, 5 min — valida p99 < 300ms, error < 1%)
BASE_URL=http://localhost:8001 TOKEN=<jwt> k6 run tests/load/mail-sla.js

# Stress (rampa até 200 VUs — encontra ponto de quebra)
BASE_URL=http://localhost:8001 TOKEN=<jwt> k6 run tests/load/mail-stress.js

# Todos os serviços de uma vez
bash tests/load/run-all.sh
```

## Thresholds de SLA

| Métrica | Target |
|---------|--------|
| http_req_duration (p99) | < 300ms |
| http_req_duration (p95) | < 150ms |
| http_req_failed | < 1% |
| checks | > 99% |

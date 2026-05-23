# Deployment Runbook — Expresso V4

Guia operacional para deploy, upgrade e rollback em produção.

## Pré-requisitos

- Docker Engine 24+ e Docker Compose v2.20+
- PostgreSQL 16 acessível (managed ou container)
- Ferramentas: `migrate` CLI ([golang-migrate](https://github.com/golang-migrate/migrate)), `docker`, `curl`

## Deploy inicial

### 1. Configuração de ambiente

```bash
cp .env.example .env
# Preencha obrigatoriamente:
#   POSTGRES_PASSWORD, S3_SECRET_KEY, KEYCLOAK_ADMIN_PASSWORD,
#   KEYCLOAK_CLIENT_SECRET, MAIL_DOMAIN, DKIM_KEY_PATH (se --profile mta)
```

### 2. Geração de chaves DKIM (se usar MTA)

```bash
bash scripts/dkim-keygen.sh
# Publica o registro TXT no DNS antes de subir o perfil mta.
```

### 3. Primeiro boot

```bash
docker compose pull
docker compose up -d
# Aguardar todos os serviços ficarem healthy:
watch docker compose ps
```

### 4. Migrations

```bash
export DATABASE_URL="postgres://expresso:${POSTGRES_PASSWORD}@localhost:5432/expresso"

# Instalar migrate CLI (uma vez)
curl -L https://github.com/golang-migrate/migrate/releases/latest/download/migrate.linux-amd64.tar.gz \
  | tar xz && sudo mv migrate /usr/local/bin/

# Aplicar todas as UPs
migrate -path migrations -database "$DATABASE_URL" up
```

### 5. Seed inicial (opcional — dev/staging)

```bash
bash scripts/seed-demo.sh
```

### 6. Verificação de saúde

```bash
for port in 8001 8002 8003 8004 8005 8006 8007 8008 8009 8010 8011 8100 8101; do
  status=$(curl -sf "http://localhost:${port}/health" && echo OK || echo FAIL)
  echo "  :${port} — ${status}"
done
```

---

## Upgrade de versão

### Rolling upgrade (zero-downtime recomendado)

```bash
# 1. Build das novas imagens
docker compose build

# 2. Aplicar novas migrations ANTES de trocar as imagens
migrate -path migrations -database "$DATABASE_URL" up

# 3. Trocar serviços um a um (começar pelos sem estado)
for svc in expresso-search expresso-notifications expresso-compliance \
           expresso-flows expresso-wopi expresso-admin \
           expresso-calendar expresso-contacts expresso-drive \
           expresso-chat expresso-meet expresso-mail expresso-auth expresso-web; do
  docker compose up -d --no-deps "$svc"
  sleep 5
  curl -sf "http://localhost:$(docker compose port $svc 8001 2>/dev/null | cut -d: -f2)/health" \
    || echo "WARN: health check falhou para $svc"
done
```

### Upgrade atômico (downtime aceito)

```bash
migrate -path migrations -database "$DATABASE_URL" up
docker compose up -d --force-recreate
```

---

## Rollback

### Rollback de migration (uma versão)

```bash
# Desfaz a última migration UP aplicada
migrate -path migrations/down -database "$DATABASE_URL" down 1

# Desfaz N migrations
migrate -path migrations/down -database "$DATABASE_URL" down N
```

Cada arquivo em `migrations/down/` tem o mesmo nome da UP correspondente e reverte exatamente o que a UP criou.

### Rollback de imagem

```bash
# Re-tag a versão anterior e recrie o serviço
docker tag expresso-mail:prev expresso-mail:latest
docker compose up -d --no-deps expresso-mail
```

**Regra**: sempre faça rollback de migration ANTES do rollback de imagem quando a versão antiga não suporta o schema novo.

---

## Blue-Green deploy

Usado quando a migration é pesada (reindex, backfill grande) e não é possível rolling.

```bash
# 1. Suba o stack "green" na mesma máquina com portas deslocadas +1000
# (ou em host separado apontando para o mesmo Postgres)
COMPOSE_PROJECT_NAME=expresso-green \
  docker compose -f docker-compose.yaml -f deploy/docker/compose-green-override.yaml up -d

# 2. Aplique as migrations no schema novo (sem afetar o blue)
migrate -path migrations -database "$DATABASE_URL" up

# 3. Teste o green com smoke tests
bash ops/smoke-mail.sh && bash ops/smoke-dav.sh

# 4. Troque o upstream no nginx (ou load balancer) para apontar para green

# 5. Derrube o blue
COMPOSE_PROJECT_NAME=expresso docker compose down
```

---

## Monitoramento pós-deploy

```bash
# Checar métricas de erro nos primeiros 10 minutos
curl -s 'http://localhost:9090/api/v1/query?query=sum(rate(http_requests_total{status=~"5.."}[5m]))' \
  | jq '.data.result[0].value[1]'

# Checar fila NATS
curl -s http://localhost:8222/jsz?accounts=true | jq '.account_details[].stream_detail[].state'
```

Dashboards de referência em Grafana (`:3001`):
- **Expresso Overview** — taxa de erro, latência p99 por serviço
- **NATS JetStream** — lag de consumidores, erros de publish

---

## Checklist de release

- [ ] `cargo test --workspace` passou no CI
- [ ] Migrations testadas em staging com dados reais
- [ ] `migrate up` rodado antes das novas imagens
- [ ] Healthchecks verdes após deploy
- [ ] Taxa de erro HTTP 5xx < 0.1% por 10 minutos
- [ ] NATS lag de consumidores voltou ao baseline
- [ ] Nenhum alerta crítico disparado no Grafana/Alertmanager

---

## Recuperação de desastre

### Postgres

```bash
# Restore de backup (pg_dump)
pg_restore -h localhost -U expresso -d expresso backup.dump

# Reaplica migrations a partir do estado do backup
migrate -path migrations -database "$DATABASE_URL" up
```

### MinIO (objetos do Drive)

```bash
# Mirror de bucket para novo MinIO
mc mirror minio-old/expresso minio-new/expresso --overwrite
```

### Redis

Redis é usado apenas para cache e pub/sub — perda é tolerada. Reinicie o container; os serviços se reconectam automaticamente.

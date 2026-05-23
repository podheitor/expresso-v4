# Expresso V4

> Suite colaborativa completa, equivalente ao Microsoft 365, desenvolvida com foco em usabilidade extrema, privacidade, performance e código próprio.

## Quickstart

```bash
# 1. Variáveis de ambiente
cp .env.example .env
# Preencha os valores marcados com :? (POSTGRES_PASSWORD, S3_SECRET_KEY, etc.)

# 2. Suba a stack completa
docker compose up -d

# 3. Aguarde todos os serviços ficarem healthy (≈60s)
docker compose ps

# 4. Aplique as migrations
export DATABASE_URL="postgres://expresso:${POSTGRES_PASSWORD}@localhost:5432/expresso"
migrate -path migrations -database "$DATABASE_URL" up

# 5. Seed de demonstração (opcional)
bash scripts/seed-demo.sh
```

**Perfis disponíveis:**
- `docker compose up -d` — stack principal (infra + todos os serviços)
- `docker compose --profile dev up -d` — inclui Mailpit (SMTP catch-all para dev)
- `docker compose --profile mta up -d` — inclui Postfix + milter (requer DKIM keys)

**Portas principais:**

| Serviço | Porta |
|---------|-------|
| Keycloak (IdP) | 8080 |
| expresso-auth | 8100 |
| expresso-mail (HTTP) | 8001 |
| expresso-calendar | 8002 |
| expresso-contacts | 8003 |
| expresso-drive | 8004 |
| expresso-notifications | 8006 |
| expresso-search | 8007 |
| expresso-wopi | 8008 |
| expresso-compliance | 8009 |
| expresso-chat | 8010 |
| expresso-meet | 8011 |
| expresso-admin | 8101 |
| Web UI | 3000 |
| Grafana | 3001 |
| Prometheus | 9090 |
| MinIO Console | 9001 |

Para procedimentos de upgrade, rollback e blue-green deploy, ver [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

## Status

**Production-ready** — todos os serviços core implementados com persistência real.

| Fase | Módulo | Equivalente M365 | Status |
|------|--------|------------------|--------|
| Fase 1 | Expresso Mail | Exchange Online / Outlook | Completo |
| Fase 2 | Expresso Calendar | Outlook Calendar | Completo (CalDAV + iMIP) |
| Fase 2 | Expresso Contacts | Outlook People | Completo (CardDAV) |
| Fase 3 | Expresso Drive | OneDrive | Completo (tus.io + WOPI) |
| Fase 3 | Office Online (LibreOffice) | Word/Excel/PPT Online | Completo (WOPI bridge) |
| Fase 4 | Expresso Chat | Microsoft Teams Chat | Completo |
| Fase 5 | Expresso Meet | Teams Meetings | Completo |
| Fase 5 | Expresso Admin | M365 Admin Center | Completo |

## Stack Tecnológico

- **Backend**: Rust (Axum, Tokio, sqlx, lettre, tonic)
- **Frontend**: SvelteKit + WASM
- **Banco**: PostgreSQL 16 + Redis 7 + MinIO
- **Auth**: Keycloak + gov.br OIDC + OAuth2.1
- **Office**: LibreOffice Online upstream (WOPI bridge em Rust)
- **Infra**: Debian 13 + Docker + Proxmox (lab)
- **Observability**: OpenTelemetry + Prometheus + Grafana

## Documentação

- [Quickstart / Deployment](docs/DEPLOYMENT.md)
- [Arquitetura Técnica](docs/ARCHITECTURE.md)
- [Roadmap](docs/ROADMAP.md)
- [Mapeamento M365 → Expresso V4](docs/M365_MAPPING.md)
- [Conformidade Governo Brasileiro](docs/COMPLIANCE_GOV_BR.md)
- [Segurança](docs/SECURITY.md)
- [Observability](docs/OBSERVABILITY.md)
- [Multi-Realm JWT Validation](docs/MULTI-REALM-JWT.md)
- [Tenant Onboarding Runbook](docs/TENANT-ONBOARDING.md)
- [Infra Lab (Proxmox)](docs/INFRA_LAB.md)
- [Load Testing](tests/load/README.md)

## Conformidade

- LGPD (Lei 13.709/2018)
- e-PING (Padrões de Interoperabilidade Gov BR)
- GSI IN 01/2020 + IN 05/2021
- ICP-Brasil (certificação digital)
- gov.br OIDC (níveis bronze/prata/ouro)
- Zero-trust + NIST CSF

## Licença

GNU Affero General Public License v3 — ver [LICENSE](LICENSE).

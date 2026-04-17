# Expresso V4

> Suite colaborativa completa, equivalente ao Microsoft 365, desenvolvida com foco em usabilidade extrema, privacidade, performance e código próprio.

## Status

🚧 **Fase de Planejamento** — Abril 2026

## Módulos Planejados

| Fase | Módulo | Equivalente M365 | Status |
|------|--------|------------------|--------|
| Fase 1 | Expresso Mail | Exchange Online / Outlook | 📐 Planejado |
| Fase 2 | Expresso Calendar | Outlook Calendar | 📐 Planejado |
| Fase 2 | Expresso Contacts | Outlook People | 📐 Planejado |
| Fase 3 | Expresso Drive | OneDrive | 📐 Planejado |
| Fase 3 | Office Online (LibreOffice) | Word/Excel/PPT Online | 📐 Planejado |
| Fase 4 | Expresso Chat | Microsoft Teams Chat | 📐 Planejado |
| Fase 5 | Expresso Meet | Teams Meetings | 📐 Planejado |
| Fase 5 | Expresso Admin | M365 Admin Center | 📐 Planejado |

## Stack Tecnológico

- **Backend**: Rust (Axum, Tokio, sqlx, lettre, tonic)
- **Frontend**: SvelteKit + WASM
- **Banco**: PostgreSQL 16 + Redis 7 + MinIO
- **Auth**: Keycloak + gov.br OIDC + OAuth2.1
- **Office**: LibreOffice Online upstream (WOPI bridge em Rust)
- **Infra**: Debian 13 + Docker + Proxmox (lab)
- **Observability**: OpenTelemetry + Prometheus + Grafana

## Documentação

- [Plano Estratégico Completo](docs/PLAN.md)
- [Mapeamento M365 → Expresso V4](docs/M365_MAPPING.md)
- [Arquitetura Técnica](docs/ARCHITECTURE.md)
- [Conformidade Governo Brasileiro](docs/COMPLIANCE_GOV_BR.md)
- [UX & Identidade Visual](docs/UX_IDENTITY.md)
- [Roadmap](docs/ROADMAP.md)
- [Infra Lab (Proxmox)](docs/INFRA_LAB.md)

## Conformidade

- 🇧🇷 LGPD (Lei 13.709/2018)
- 🔐 e-PING (Padrões de Interoperabilidade Gov BR)
- 🛡️ GSI IN 01/2020 + IN 05/2021
- 📜 ICP-Brasil (certificação digital)
- 🏛️ gov.br OIDC (níveis bronze/prata/ouro)
- 🔒 Zero-trust + NIST CSF

## Sistema Operacional (Lab)

**Debian 13 "Trixie"** — escolha definitiva para todas as VMs do projeto.

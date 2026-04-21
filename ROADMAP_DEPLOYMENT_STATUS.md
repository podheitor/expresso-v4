# Expresso V4 — Status de Deployment

**Atualizado**: 2026-04-19  
**Status Geral**: ✅ **Phase 2 Completa** — Infrastructure & expresso-mail funcional

---

## Phase 2: Infrastructure & Harness (✅ COMPLETA)

### Completed ✅
- [x] Proxmox setup (5 VMs provisionadas, todas rodando)
- [x] Docker infra em todas VMs (CE v29 + compose plugin)
- [x] PostgreSQL 16 + Redis 7 (DB layer, healthy)
- [x] MinIO S3-compatible (Storage, healthy)
- [x] Keycloak 25 (IAM, realm `expresso` criado + client)
- [x] NATS 2.10 (Message broker, healthy)
- [x] Grafana + Prometheus + Loki (Observability stack)
- [x] **expresso-mail service** (first Rust service deployed, healthy)
  - Email bootstrap (admin@expresso.local)
  - SMTP/IMAP/HTTP API active
  - DB migrations applied
  - Keycloak integration configured

### Artifacts
- Dockerfile.mail → Multi-stage Rust build (rust:latest, 142MB final)
- compose-mail.yaml → Ready in ~/expresso/compose-mail.yaml on VM 125
- CONNECTIONS.md → Todas credenciais + endpoints documentados

---

## Phase 3: Service Deployment (⏳ FUTURO)

### Planned
- [ ] expresso-calendar (scaffold ready)
- [ ] expresso-contacts (scaffold ready)
- [ ] expresso-drive (scaffold ready)
- [ ] expresso-chat (scaffold ready)
- [ ] expresso-meet (scaffold ready)
- [ ] expresso-auth (OAuth server, scaffold ready)
- [ ] expresso-admin (Admin panel, scaffold ready)
- [ ] expresso-compliance (Audit, scaffold ready)
- [ ] expresso-search (Elasticsearch integration, scaffold ready)
- [ ] expresso-wopi (Office integration, scaffold ready)
- [ ] expresso-notifications (Alerts, scaffold ready)
- [ ] expresso-flows (Automation, scaffold ready)

**Build Strategy**: Cada service segue modelo de `expresso-mail`: Dockerfile + compose-*.yaml + env vars via config::Environment

---

## Phase 4: Backup & DR (⏳ FUTURO)

### Planned
- [ ] PostgreSQL backup automation
- [ ] MinIO bucket replication
- [ ] Keycloak realm export
- [ ] Volume snapshots (Proxmox)
- [ ] Point-in-time recovery procedures

---

## Known Constraints & Workarounds

| Issue | Root Cause | Workaround |Applied |
|-------|-----------|-----------|---------|
| CPU x86-64-v2 incompatibility | VM CPU model mismatch | `qm reboot` + `cpu: host` config | ✅ |
| NATS restart (invalid flags) | Version mismatch in compose | flags: `--store_dir` not `--store-dir` | ✅ |
| Keycloak UBI no curl/wget | Container minimal image | `/dev/tcp` bash healthcheck | ✅ |
| Dockerfile.mail missing migrations | Incomplete COPY directives | Added `COPY migrations/` | ✅ |
| Rust MSRV (aws-config) | 1.86 insufficient for aws-config@1.8.15 | Used `rust:latest` | ✅ |

---

## Metrics

- **Total Services**: 12 (mail + 11 more in scaffold)
- **Current Running**: 9/9 (100%)
- **Current Healthy**: 5/5 core + 3/3 obs = 8/9 (Grafana status varies)
- **Uptime**: 45-50 minutes (since latest deploy)
- **Docker images built**: 1 (expresso-mail:latest, 142MB)
- **Build time**: ~20-25 min (Rust dependencies + aws-sdk)

---

## Next Scheduled

1. Deploy expresso-calendar (similar to expresso-mail pattern)
2. Validate SMTP external connectivity (telnet tests)
3. Configure Prometheus scrape jobs + Grafana dashboards
4. Load testing with concurrent users

---

**Owner**: DevOps Automation  
**Last Updated**: 2026-04-19 14:14 UTC

---

## Update 2026-04-19 (Phase 3 Runtime Baseline)

### Completed
- [x] Deployed remaining Rust/Axum services baseline runtime on VM 125:
  - expresso-admin (8101)
  - expresso-auth (8100)
  - expresso-calendar (8002)
  - expresso-compliance (8009)
  - expresso-contacts (8003)
  - expresso-drive (8004)
  - expresso-flows (8005)
  - expresso-notifications (8006)
  - expresso-search (8007)
  - expresso-wopi (8008)
- [x] Added minimal `axum` runtime (`/health`) to scaffold services to keep containers alive.
- [x] Verified open ports + HTTP 200 on `/health` for all new services.

### Notes
- `expresso-chat` and `expresso-meet` were not part of workspace build targets in this cycle.
- `deploy/docker/compose-phase3.yaml` added for reproducible deployment.

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

---

## Update 2026-04-21 (expresso-chat alignment)

### Completed
- [x] Validated `expresso-chat` builds in workspace (`cargo check -p expresso-chat` OK).
- [x] Fixed blocking compile errors:
  - `RoomPreset` now derives `Serialize` (`#[serde(rename_all = "snake_case")]`) — `services/expresso-chat/src/matrix/mod.rs`.
  - Dropped unused `routing::get` import — `services/expresso-chat/src/api/messages.rs`.
- [x] Confirmed `Dockerfile.chat` pattern matches `Dockerfile.mail` (multi-stage, debian:bookworm-slim runtime).
- [x] Added `expresso-chat` entry to `deploy/docker/compose-phase3.yaml` on port **8010**
  (avoids collision with `expresso-drive` default 8004).
  - Uses `SERVER__HOST` / `SERVER__PORT` env convention (matches service code).
  - `DATABASE__URL`, `MATRIX__*` placeholders left for per-env injection.

### Notes
- Chat service has a real BFF scaffold (Matrix CS API wrapper, channels/messages routes,
  tenant repos) — not just a /health stub like the other Phase 3 services.
- `expresso-meet` still outside the workspace (`src/` empty, not in `Cargo.toml` members).
  Deferred to dedicated scaffolding cycle.
- Residual warning: `MatrixConfig.admin_token` never read (admin provisioning pending).

### Next
1. Build `expresso-chat:latest` image on VM 125 and deploy via `compose-phase3.yaml`.
2. Wire a Synapse homeserver (or docker stub) to exercise `/api/v1/channels` end-to-end.
3. Scaffold `expresso-meet` (workspace member + minimal axum runtime).

---

## Update 2026-04-21 (chat + meet deployed on VM 125)

### Completed
- [x] Scaffolded `expresso-meet` (workspace member, axum `/health` + `/ready`, port 8011).
  - `services/expresso-meet/Cargo.toml` + `services/expresso-meet/src/main.rs` created.
  - `Dockerfile.meet` binary path corrected (`expresso_meet` → `expresso-meet`).
  - Added to root `Cargo.toml` workspace members.
- [x] Built Docker images on VM 125 (192.168.15.125):
  - `expresso-meet:latest` — 1m24s release build, sha256:f9a268cac357…
  - `expresso-chat:latest` — 1m45s release build, sha256:0a7169e92492…
- [x] Deployed via `~/expresso/compose-chat-meet.yaml`:
  - `expresso-chat` listening on 0.0.0.0:8010 → `/health` returns `{"service":"expresso-chat","status":"ok"}` (HTTP 200).
  - `expresso-meet` listening on 0.0.0.0:8011 → `/health` returns `{"service":"expresso-meet","status":"ok"}` (HTTP 200).

### Phase 3 service status (VM 125)
| Service | Port | Status |
|---------|------|--------|
| expresso-mail | (multi) | ✅ healthy (Phase 2) |
| expresso-calendar | 8002 | ✅ healthy |
| expresso-contacts | 8003 | ✅ healthy |
| expresso-drive | 8004 | ✅ healthy |
| expresso-flows | 8005 | ✅ healthy |
| expresso-notifications | 8006 | ✅ healthy |
| expresso-search | 8007 | ✅ healthy |
| expresso-wopi | 8008 | ✅ healthy |
| expresso-compliance | 8009 | ✅ healthy |
| expresso-chat | 8010 | ✅ healthy (NEW) |
| expresso-meet | 8011 | ✅ healthy (NEW) |
| expresso-auth | 8100 | ✅ healthy |
| expresso-admin | 8101 | ✅ healthy |

**12/12 Rust services running on VM 125.**

### Notes
- Compose project warned about orphan containers from prior deploys — cosmetic only, all services still up.
- `expresso-chat` deployed without DB/Matrix wiring; routes other than `/health` will return degraded responses until `MATRIX__*` + `DATABASE__URL` are injected.
- Residual warning: `MatrixConfig.admin_token` never read (admin provisioning pending).

### Next
1. Stand up Synapse homeserver (Matrix) + wire `expresso-chat` env to exercise `/api/v1/channels`.
2. Promote stub services to real functionality (auth OAuth, calendar CalDAV, etc.).
3. Phase 4: backup/DR for PostgreSQL + MinIO + Keycloak realm export.

---

## Update 2026-04-21b (Synapse + Jitsi BFFs wired e2e on VM 125)

### Completed
- [x] **Synapse homeserver** provisioned:
  - Container `expresso-synapse` (matrixdotorg/synapse:latest) on `expresso_default`
    network, external :8108 → internal :8008.
  - Postgres backend: dedicated `synapse` DB (C locale) on 192.168.15.123.
  - AppService registration `expresso-chat` (`@expresso-.*:expresso.local` +
    `#expresso-.*:expresso.local`, exclusive=true).
  - Admin user `@admin:expresso.local` created (register_new_matrix_user),
    access_token captured for `MATRIX__ADMIN_TOKEN`.
- [x] **expresso-chat e2e validated** against real Synapse v1.133:
  - `POST /api/v1/channels` → 201 + Matrix room id (`!ATuykkioAwCqIllxqc:…`)
  - `POST /api/v1/channels/:id/messages` → 201 + `event_id`
  - `GET  /api/v1/channels/:id/messages` → chunk with `m.room.message` events
  - Fix: `MatrixClient::ensure_registered` (commit `0e6c8b7`) — AS must
    pre-register users in its exclusive namespace via
    `m.login.application_service` before `?user_id=…` impersonation works on
    Synapse.
- [x] **expresso-meet e2e validated** (JWT path):
  - Migration `meetings_schema` applied (tables `meetings` +
    `meeting_participants` + RLS).
  - `POST /api/v1/meetings` → 201 + `join_url` + HS256 JWT with expected
    claims (`iss=expresso`, `sub=meet.expresso.local`, `context.user.*`,
    `context.features.*`).
  - `cargo test --package expresso-meet`: 3/3 passing (mint_round_trip_decodes,
    generate_room_name_has_prefix, join_url_is_https).
- [x] **Unit tests** added for chat Matrix localpart parsing (commit `04eb9d4`,
  `cargo test --package expresso-chat`: 3/3 passing).

### Phase 3 service status (VM 125 — unchanged)
- All 12/12 Rust services still up + healthy.
- `expresso-synapse` added as 13th container (Matrix homeserver).

### Deferred
- **Real Jitsi Meet infra** (Prosody + jicofo + jvb) — expresso-meet mints
  Jitsi-compatible JWTs today; full Jitsi stack stands up in a separate
  deployment cycle (TURN server + TLS certs + 5GB images).
- **Push to origin** — no git remote configured in local clone; await URL.
- **Phase 4 items remaining**:
  - SSO Keycloak ↔ Synapse (OIDC bridge via mod_auth_oidc or delegated auth).
  - E2EE direct messages, reactions/threads, file sharing via Drive.
  - SvelteKit Matrix client UI.

### Notes
- `MATRIX__ADMIN_TOKEN` now populated but still `#[allow(dead_code)]`; wiring
  lands with Keycloak→Matrix user provisioning flow.
- AppService registration namespace flipped `exclusive: true` — required for
  Synapse to accept user impersonation on first contact (pre-exclusive false
  rejected with `M_FORBIDDEN`).

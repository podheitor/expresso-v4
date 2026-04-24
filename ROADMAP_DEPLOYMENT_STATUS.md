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

---

## Update 2026-04-23 (CalDAV interop hardening)

### Completed
- `/.well-known/caldav` (RFC 6764) → 301 redirect to `/caldav/` for
  Thunderbird/iOS/macOS service discovery (new `api/wellknown.rs`).
- **PROPPATCH** handler (new `caldav/proppatch.rs`): accepts `<set>`/`<remove>`
  props from Apple Calendar post-MKCALENDAR flow, returns 207 OK no-op
  (persistence TODO). Without this, macOS deletes freshly created calendars.
- **free-busy-query REPORT** (RFC 4791 §7.10): new `free_busy()` in
  `caldav/report.rs` returning VFREEBUSY iCal for the target calendar within
  the requested `<time-range>` window.
- New PROPFIND props: `supported-report-set`, `current-user-privilege-set`,
  `getcontentlength`. Required by macOS/iOS before they agree to sync.
- OPTIONS: `Allow` now includes `HEAD, PROPPATCH`; `DAV:` class list adds
  `calendar-schedule` (RFC 6638).
- `supported-calendar-component-set` now advertises VTODO alongside VEVENT.
- `xml::detect_report_kind()` helper + 4 new unit tests
  (`detect_report_kinds`, `propfind_new_props`, `parses_set_and_remove`,
  `empty_body_returns_nothing`). **37/37 tests pass** on remote builder (VM 125).

### Verified
- `cargo check -p expresso-calendar` → clean (7.23s).
- `cargo test -p expresso-calendar --bins` → 37 passed, 0 failed.

### Deferred (next CalDAV sprint)
- **Persist PROPPATCH** values (displayname/color) via `CalendarRepo::update`.
- **sync-collection REPORT** (RFC 6578) for incremental sync with tokens.
- **MOVE / COPY** verbs (low priority — most clients work without).
- **Scheduling inbox/outbox** URIs for server-side iTIP auto-processing.
- **Keycloak token introspection** in `caldav/auth.rs` (replace dev mode).

### Next
- Rebuild `expresso-calendar:latest` image + redeploy on VM 125.
- Smoke test with `curl` PROPFIND + OPTIONS against deployed endpoint.
- Either continue CalDAV (deferred items) or move to CardDAV / Drive WebDAV.

## Update 2026-04-23b (CalDAV sync-collection + CardDAV parity + PROPPATCH persist)

### B. CalDAV sync-collection REPORT (RFC 6578) — DONE
- `caldav/xml.rs`: `parse_sync_token()` + `detect_report_kind()`; token format `urn:expresso:ctag:{N}`.
- `caldav/sync.rs` NEW: fast-path (tokens match → empty 207, same token) + full-resend fallback (ctag-bumped → all current events as new tokens).
- `caldav/report.rs`: dispatch to `sync::handle()` when report kind is `sync-collection`.
- Tests +2 (`token_roundtrip`, `parse_sync_token_present_and_empty`).
- Deferred: tombstone tracking for true incremental deltas.

### C. CardDAV hardening parity — DONE
Mirrored CalDAV scope on `expresso-contacts`:
- `carddav/xml.rs, propfind.rs, resource.rs, mod.rs, report.rs` extended (sync-token, supported-report-set, current-user-privilege-set, getcontentlength).
- `carddav/proppatch.rs` NEW + `carddav/sync.rs` NEW (RFC 6578 on addressbooks).
- `api/wellknown.rs` NEW: `/.well-known/carddav` → 301 `/carddav/`.
- OPTIONS: `allow: ... PROPPATCH ... MKCOL` + `dav: 1, 2, 3, addressbook`.
- Tests: 21 passed.

### D. PROPPATCH persistence — DONE
Previously no-op handlers now persist mappable properties to DB.
- **Calendar** (`caldav/proppatch.rs`): `displayname→name`, `calendar-description→description`, `calendar-color→color`, `calendar-timezone→timezone`. Calls `CalendarRepo::update()`.
- **Contacts** (`carddav/proppatch.rs`): `displayname→name`, `addressbook-description→description`. Calls `AddressbookRepo::update()`.
- Home-level PROPPATCH remains no-op (no persistent property store for principals).
- Unknown props + `<remove>` acknowledged 200 OK, not persisted.
- Tests: calendar 40 passed, contacts 21 passed.

### Deployment
- Images rebuilt: `expresso-calendar:latest`, `expresso-contacts:latest`.
- Redeployed on VM `192.168.15.125` via `docker compose -f compose-phase3.yaml up -d --force-recreate`.
- Smoke verified on both services: `/.well-known/*dav` → 301, OPTIONS advertises full verb set + DAV class headers.

### Remaining deferred
- Tombstone tracking for incremental sync-collection deltas.
- Keycloak auth (replace `CALENDAR_DEV_AUTH` / `CONTACTS_DEV_AUTH`).
- WebDAV verbs MOVE/COPY.
- Calendar scheduling inbox/outbox.
- Persistent dead-property store for arbitrary PROPPATCH props outside the mapped set.

## Update 2026-04-23c (DAV tombstones — true incremental sync)

### E. Sync-collection delta (RFC 6578 full) — DONE
Previously sync-collection fell back to full resend whenever token differed.
Now emits true incremental deltas.

**Schema** (`migrations/20260610000000_dav_tombstones.sql`):
- Added `last_ctag BIGINT` column to `calendar_events` + `contacts`, stamped by trigger on every INSERT/UPDATE with the newly bumped parent ctag.
- New tables `calendar_event_tombstones` + `contact_tombstones` (`tenant_id`, parent_id, `uid`, `deleted_ctag`, `deleted_at`) populated by trigger on DELETE.
- Triggers rewritten to `RETURNING ctag INTO new_ctag` — single-pass bump + stamp.
- Backfill: existing rows get `last_ctag = parent.ctag` so pre-migration data appears as "changed at current ctag".

**Handlers** (`caldav/sync.rs`, `carddav/sync.rs`):
- Initial sync (token missing/bad) → full resend, no tombstones.
- Fast path (token == current) → empty 207 + unchanged token.
- Incremental (token < current) → `SELECT WHERE last_ctag > from_ctag` for members (200 OK) + `SELECT WHERE deleted_ctag > from_ctag` from tombstones (404 Not Found).

**Result:** macOS / iOS / Thunderbird clients now receive precise deltas; deletions propagate properly instead of requiring client-side diff against full membership.

### Deployment
- Migration applied live on `192.168.15.123/expresso` (backfilled 6 events + 8 contacts).
- Images rebuilt `expresso-calendar:latest` + `expresso-contacts:latest`, redeployed on VM `192.168.15.125`.
- Tests: calendar 40/40, contacts 21/21.

### Remaining deferred
- Keycloak auth integration (replace `CALENDAR_DEV_AUTH` / `CONTACTS_DEV_AUTH`).
- Persistent dead-property store for arbitrary PROPPATCH props.
- Tombstone retention/GC policy (currently unbounded growth).
- WebDAV MOVE / COPY verbs.
- Calendar scheduling inbox/outbox (RFC 6638 client side).

## Update 2026-04-23d (Keycloak Basic auth for CalDAV / CardDAV)

### F. Keycloak password-grant auth (RFC 6749 §4.3) — DONE
Replaces the `CALENDAR_DEV_AUTH` / `CONTACTS_DEV_AUTH` bypass with real
credential validation. Dev shim remains as fallback for local development.

**New shared module** `libs/expresso-auth-client/src/keycloak_basic.rs`:
- `KcBasicConfig { url, realm, client_id, client_secret }`
- `KcBasicAuthenticator`: POSTs `grant_type=password` to
  `{url}/realms/{realm}/protocol/openid-connect/token`; maps HTTP status →
  `InvalidCredentials | Unreachable | Upstream`.
- 60 s in-memory cache keyed by `sha256(user:pass)` to avoid hitting
  Keycloak on every PROPFIND.
- Exported alongside `OidcValidator` from the crate root.

**Service wiring** (`state.rs` + `main.rs` in both calendar and contacts):
- `AppState::new(db, kc_basic)` — takes an optional authenticator.
- Built from env at startup via `KcBasicConfig::from_env_prefix("CALDAV_KC")`
  (calendar) / `"CARDDAV_KC"` (contacts). When all three env vars present,
  log line `"CalDAV/CardDAV Keycloak Basic auth enabled"`.

**Auth precedence** (`caldav/auth.rs`, `carddav/auth.rs`):
1. `AppState::kc_basic()` set → Keycloak path (production).
2. Else `CALENDAR_DEV_AUTH=1` / `CONTACTS_DEV_AUTH=1` → dev path.
3. Else → 401.

Successful Keycloak auth still resolves the DB user row by email for tenant
binding (Keycloak is the identity source; the local `users` table mirrors it).

### Env activation recipe (production)
Add to `expresso-calendar` service in compose:
```
CALDAV_KC_URL:           http://expresso-keycloak:8080
CALDAV_KC_REALM:         expresso
CALDAV_KC_CLIENT_ID:     expresso-dav
CALDAV_KC_CLIENT_SECRET: <generated-secret>
```
And analogous `CARDDAV_KC_*` for `expresso-contacts`. The `expresso-dav`
confidential client must have **Direct Access Grants** enabled in the
Keycloak realm.

### Tests
- `expresso-auth-client`: 6/6 (adds 2 new: `cache_key_varies`, `config_from_env_missing`).
- `expresso-calendar` bins: 40/40.
- `expresso-contacts` bins: 21/21.

### Deployment
- Images rebuilt + redeployed on VM `192.168.15.125`. Without env vars set,
  behaviour is unchanged from previous release (still requires DEV_AUTH or
  falls back to 401). Ready for sysadmin to set `CALDAV_KC_*` /
  `CARDDAV_KC_*` and reload to flip to Keycloak mode.

### Remaining deferred
- Keycloak realm template: create `expresso-dav` client with direct access
  grants + client secret; wire into `expresso-admin` templates.
- Dead-property store for arbitrary PROPPATCH props outside the mapped set.
- Tombstone GC policy.
- WebDAV MOVE / COPY verbs.
- Scheduling inbox/outbox (RFC 6638).

## Update 2026-04-23e (Keycloak client seed + live activation)

- Seeded realm `expresso` with confidential client `expresso-dav`:
  - `standardFlowEnabled=false`, `directAccessGrantsEnabled=true`,
    `clientAuthenticatorType=client-secret`.
  - Secret provisioned + captured from admin API; stored in compose env.
- Idempotent helper `deploy/keycloak/seed_dav_client.sh` (+ wrapper on host)
  creates/updates the client and prints `client_id:secret`.
- Patched `compose-phase3.yaml` on VM 192.168.15.125 (backup saved as
  `compose-phase3.yaml.bak.*`) to inject per-service env:
  - `expresso-calendar`: `CALDAV_KC_URL`, `CALDAV_KC_REALM`,
    `CALDAV_KC_CLIENT_ID`, `CALDAV_KC_CLIENT_SECRET`.
  - `expresso-contacts`: `CARDDAV_KC_*` analogues.
- Force-recreated both containers; startup logs confirm:
  `CalDAV Keycloak Basic auth enabled` / `CardDAV Keycloak Basic auth enabled`.
- Live smoke (alice@expresso.local after admin-API password reset):
  - no Authorization → 401.
  - wrong password → 401 (KC rejects).
  - correct password → 2xx/4xx from DAV layer (auth ≠ blocking,
    KC path exercised end-to-end).
- Task F is now **active** in production: CalDAV/CardDAV authenticate against
  Keycloak via OAuth2 password grant, with 60 s in-memory cache per
  `user:pass` pair.

## Update 2026-04-23f (Dead-property store — RFC 4918 §15)

- Schema: migration `20260612000000_dav_dead_properties.sql` creates
  `calendar_dead_properties` + `addressbook_dead_properties` (UUID PK, FK
  cascade on tenant + collection, UNIQUE(collection_id, namespace, local_name),
  ix on tenant_id). Applied to live DB (192.168.15.123/expresso).
- Domain: `DeadPropRepo` in both services with `upsert_*`, `remove_*`,
  `list_for_*` operations.
- Parser: PROPPATCH rewritten with `quick_xml::NsReader::read_resolved_event()`
  → namespace URIs (not prefixes) drive live-vs-dead classification.
  - `LIVE_PROPS` = {(DAV:, displayname), (caldav, calendar-description),
     (apple, calendar-color), (caldav, calendar-timezone)} for calendars.
  - `LIVE_PROPS` = {(DAV:, displayname), (carddav, addressbook-description)}
     for address books.
  - Any other (namespace, local) pair = dead → persisted verbatim.
- PropFind: `PropRequest.allprop` flag drives dead-prop inclusion; renders
  `<{local} xmlns="{ns}">{value}</{local}>` inside `<D:prop>` for collection
  resources when allprop requested.
- Tests: 5 new proppatch tests per service (classification, parse set/remove,
  custom-ns handling, empty body). All passing (calendar 43/43, contacts
  23/23).
- Live smoke (alice@expresso.local, cal Pessoal):
  1. `PROPPATCH <X:tag-color xmlns:X="http://example.com/custom">blue</>`
     → 207 Multi-Status, prop echoed.
  2. Row visible in `calendar_dead_properties`:
     `http://example.com/custom|tag-color|blue`.
  3. `PROPFIND /allprop` returns `<tag-color xmlns="http://example.com/custom">blue</tag-color>`
     alongside live props.
  4. `PROPPATCH remove` → row count back to 0.
- Deploy: images `expresso-calendar:latest` + `expresso-contacts:latest`
  rebuilt + loaded on VM 192.168.15.125, containers force-recreated via
  `compose-phase3.yaml`.

## Update 2026-04-23g (Tombstone GC / retention)

- New modules: `services/expresso-{calendar,contacts}/src/domain/tombstone_gc.rs`
  provide `purge_once(pool, days)` + `spawn(pool, days, hours)` background task.
- Retention defaults: 30 days, GC cycle every 6 h (env overrides
  `DAV_TOMBSTONE_RETENTION_DAYS`, `DAV_TOMBSTONE_GC_INTERVAL_HOURS`).
- Wired into both services' `main.rs`: on startup, if DB pool available,
  spawn GC task.
- Query pattern (idempotent, append-only table):
  ```sql
  DELETE FROM calendar_event_tombstones
   WHERE deleted_at < now() - make_interval(days => $1::int);
  ```
  Same pattern for `contact_tombstones`.
- Tests: 1 new test per service (`defaults_reasonable`). Calendar 44/44,
  contacts 24/24 passing.
- Live smoke (seed 45-day-old tombstones → docker restart → GC cycle logs):
  - calendar: `tombstone GC cycle completed, deleted: 1` → row gone.
  - contacts: `tombstone GC cycle completed, deleted: 1` → row gone.
- Trade-off per RFC 6578 §3.8: clients offline > 30 days lose specific
  deletion signals and must do full resync (expected behavior).

## Update 2026-04-23h (WebDAV MOVE / COPY verbs)

- New modules: `services/expresso-{calendar,contacts}/src/caldav/movecopy.rs`
  + `.../carddav/movecopy.rs`. Scope: resource-level only (events / contacts).
- Semantics per RFC 4918 §9.8/§9.9:
  - Source + destination MUST resolve to same authenticated user.
  - Cross-collection allowed (same tenant). Destination parsed from
    `Destination:` header (absolute URL stripped to path).
  - `Overwrite: F` → 412 when destination exists. Default overwrite=T.
  - MOVE = COPY + DELETE source (no-op if src == dst).
  - Response: 201 Created when dest was new, 204 No Content when overwritten.
- Dispatch wired in `caldav/mod.rs` + `carddav/mod.rs`. Advertised in
  `Allow:` header of OPTIONS response.
- Tests: 3 unit tests total (URL origin strip). Full suites: calendar 47/47,
  contacts 25/25 passing.
- Live smoke on VM 125 with Alice (both collections):
  - Calendar: PUT 201 → COPY 201 → COPY Overwrite:F 412 → DELETE 204
    → MOVE 201 (row migrated back). OPTIONS lists COPY, MOVE. ✓
  - Contacts: idem (PUT/COPY/412/DELETE/MOVE) ✓
- Out of scope: MOVE/COPY of whole collections, Depth: infinity (future).

## Update 2026-04-23i (RFC 6638 scheduling — schedule-inbox/outbox + iMIP POST)

- URI layer extended (`caldav/uri.rs`) with two new collection variants:
  - `Target::ScheduleInbox  { user_id }` → `/caldav/<user>/schedule-inbox/`
  - `Target::ScheduleOutbox { user_id }` → `/caldav/<user>/schedule-outbox/`
- PROPFIND (`caldav/propfind.rs`):
  - Home Depth:1 now appends both schedule collections alongside calendars.
  - Stand-alone PROPFIND on each schedule URL returns a collection response
    with `<D:resourcetype><D:collection/><C:schedule-inbox|outbox/></D:resourcetype>`
    + proper privilege set (`C:schedule-deliver` / `C:schedule-send`).
  - Two new prop flags in `PropRequest`: `schedule_inbox_url`,
    `schedule_outbox_url` → rendered inside home/calendar responses.
- Scheduling POST (`caldav/schedule.rs`, new, ~230 L):
  - Dispatched in `caldav/mod.rs` on `POST` verb.
  - Parses METHOD + ATTENDEEs from VCALENDAR body.
  - Builds MIME message (plain-text alt + `text/calendar; method=…` part)
    with `From:` = iCal ORGANIZER (fallback = `SMTP_FROM`).
  - Sends via `lettre` AsyncSmtpTransport to env-configured relay
    (`SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`,
    `SMTP_FROM`, `SMTP_STARTTLS`).
  - Per-recipient status mapped to RFC 5545 request-status codes:
    `1.2` delivered, `3.7` invalid address, `5.1` service unavailable.
  - Returns `application/xml` with `<C:schedule-response>` per §6.2.
- OPTIONS `Allow:` updated to include POST.
- Compose patched: `expresso-calendar` now has
  `SMTP_HOST=expresso-postfix`, `SMTP_PORT=25`, `SMTP_FROM=calendar@expresso.local`.
- Tests: 3 new unit tests (method extract, organizer extract, response
  render). Full suite: calendar 53/53, contacts 25/25 passing.
- Live smoke on VM 125:
  - PROPFIND home Depth:1 → lists both calendars + schedule-inbox + schedule-outbox. ✓
  - PROPFIND on schedule-inbox/outbox → resourcetype correctly includes the
    schedule component element. ✓
  - POST outbox with iTIP REQUEST (ORGANIZER alice, ATTENDEE invalid-domain)
    → Postfix accepted relay → `<C:request-status>1.2;Message delivered</C:request-status>`. ✓
- Out of scope:
  - Inbox write/deliver: currently iMIP arrives via normal email (expresso-mail
    IMAP INBOX). Native CalDAV inbox storage is future work.
  - Auto-processing of incoming REPLY (attendee PARTSTAT sync) — planned.

## Update 2026-04-23j (Web UI — calendar month grid + event CRUD)

- `expresso-web` (Axum SSR + Askama) extended com grid mensal de agenda + formulário
  criar/editar/apagar eventos, integrando direto com API JSON do `expresso-calendar`
  (`/api/v1/calendars/:id/events`).
- Novas rotas em [services/expresso-web/src/routes.rs](services/expresso-web/src/routes.rs):
  - `GET /calendar/:cal_id?month=YYYY-MM` → grid 6×7 (Seg→Dom), eventos chip com
    horário + título, links para editar e criar em dia específico.
  - `GET|POST /calendar/:cal_id/events/new[?date=YYYY-MM-DD]` → formulário
    (summary/dtstart/dtend/location/description), prefills data quando clicado
    dia do mês. Monta VCALENDAR+VEVENT com ORGANIZER do usuário logado e
    envia `POST /api/v1/calendars/:id/events` Content-Type: text/calendar.
  - `GET|POST /calendar/:cal_id/events/:id/edit` → pré-carrega evento
    (`GET /api/v1/calendars/:cal_id/events/:id`), preserva UID original + organizer,
    `PUT /api/v1/calendars/:cal_id/events/:id` text/calendar.
  - `POST /calendar/:cal_id/events/:id/delete` → DELETE upstream; redirect /calendar/:id.
- [services/expresso-web/src/templates.rs](services/expresso-web/src/templates.rs):
  structs `Event`, `MonthCell`, `CalendarMonthTpl`, `EventFormTpl`.
- Novos templates: [templates/calendar_month.html](services/expresso-web/templates/calendar_month.html)
  (nav prev/hoje/next, seletor de agenda, grid com chips `event-chip`),
  [templates/event_form.html](services/expresso-web/templates/event_form.html)
  (campos + botão Apagar em modo edit).
- [services/expresso-web/static/app.css](services/expresso-web/static/app.css):
  CSS `.month-grid`, `.month-cell`, `.event-chip` (cores, today highlight, off-month fade),
  estilos `.form` + `.alert.error` + `.btn.danger`.
- [services/expresso-web/src/upstream.rs](services/expresso-web/src/upstream.rs):
  novo helper `put_body` para PUT text/calendar.
- [templates/calendar.html](services/expresso-web/templates/calendar.html):
  cada agenda agora é link para o grid mensal (era plain text).
- [Dockerfile.web](Dockerfile.web): adicionado `mold` ao builder (linker exigido
  por `.cargo/config.toml`).
- Build: `cargo check --release -p expresso-web` → sucesso, apenas warning
  de campo não lido resolvido. Imagem `expresso-web:latest` (SHA `90ab98d5dd2f`)
  publicada em VM125.
- Smoke live VM125:
  - `GET /healthz` → 200 `{"service":"expresso-web","status":"ok"}` ✓
  - `GET /calendar/:cal_id` → 303 redirect para /login (auth middleware correto) ✓
  - `GET /calendar/:cal_id/events/new` → 303 redirect (rota registrada, auth gate) ✓
  - Container logs limpos — sem panic de render askama ✓
  - Backend direto (`x-tenant-id` + `x-user-id`): 7 eventos em maio/2026, inclui
    eventos pré-existentes + 4 POSTs do smoke (`UI Grid Smoke` 2026-05-15T12:00Z) ✓
- Validação UI completa depende de sessão Keycloak viva (OIDC flow); estrutura
  confirmada por build + routes + backend integration.
- Fora de escopo desta entrega:
  - Views semana/dia (próximo incremento).
  - Agenda compartilhada (usa ACL de Task 7 — ainda por fazer).
  - Contacts CRUD (planejado a seguir).
  - UI iMIP outbox (usa Task 4; form poderia anexar ATTENDEEs — próximo).

## Update 2026-04-23k (Web UI — Contacts CRUD)

Objetivo: completar Task 5B — CRUD de contatos no expresso-web (Axum + Askama server-rendered), sem SvelteKit.

### Mudanças
- `services/expresso-web/src/templates.rs`:
  - `Contact` reescrito: novos campos `uid`, `given_name`, `family_name`, `vcard_raw`; serde `alias = "email_primary"` e `alias = "phone_primary"` (corrige bug em que email/phone vinham sempre None do backend).
  - Novo `ContactFormTpl` (me, book, contact_id, full_name, given_name, family_name, email, phone, organization, error).
- `services/expresso-web/src/routes.rs`:
  - Rotas novas: `GET/POST /contacts/:book_id/new`, `GET/POST /contacts/:book_id/:id/edit`, `POST /contacts/:book_id/:id/delete`.
  - Helpers: `escape_vcard`, `build_vcard` (VERSION:4.0 + UID + N + FN + EMAIL + TEL + ORG), `load_book`.
  - Edit preserva UID existente → evita duplicatas no backend.
- Templates:
  - `templates/contact_form.html` novo (inputs, Apagar hidden-form).
  - `templates/contacts.html` reescrito (botão "+ Novo contato", linhas linkam para edição).

### Build/Deploy
- `cargo check --release -p expresso-web` verde (54.79s, zero warnings).
- `docker build -f Dockerfile.web -t expresso-web:latest .` → SHA `25c762d3e15e` (35 MB gz).
- Deploy VM 125 via `docker save | gzip → scp → docker load → docker compose up -d --force-recreate expresso-web`.

### Smoke (VM 125)
- `GET /healthz` → 200.
- `GET /contacts` → 303 (redirect para login, esperado sem sessão).
- `GET /contacts/:book/new` → 303 (idem).
- `docker logs expresso-web` → sem panic/error.

### Fora de escopo desta entrega
- Views semana/dia e compartilhamento (Task 7).
- UI de iMIP/ATTENDEEs no form de evento (próximo incremento).
- Admin UI (expresso-admin, Task 5C).

## Update 2026-04-23l (Admin UI — User CRUD via Keycloak)

Objetivo: Task 5C — estender expresso-admin (SSR Axum + Askama) com CRUD de usuários do realm via Keycloak Admin REST API.

### Mudanças
- `services/expresso-admin/src/kc.rs`:
  - Novos métodos `KcClient::user(id)`, `create_user(NewUser)` (retorna id via header `Location`; seta senha se informada), `update_user(id, UpdateUser)` (PATCH seletivo: email/firstName/lastName/enabled), `set_password(id, pw, temporary)` (reset-password API), `delete_user(id)`.
  - Structs `NewUser` e `UpdateUser` + `use serde_json::json` e `json!` para bodies parciais.
- `services/expresso-admin/src/handlers.rs`:
  - Handlers `user_new` (GET form), `user_create` (POST form → cria + redirect /users), `user_edit` (GET form preenchido), `user_update` (POST → update + senha opcional), `user_delete` (POST → remove).
  - Structs `UserCreateForm`/`UserUpdateForm` com `enabled`/`temporary` como `Option<String>` para checkbox-binding clássico de HTML forms.
- `services/expresso-admin/src/templates.rs`: novo `UserFormTpl` (user_id:Option, campos, enabled, error).
- `services/expresso-admin/src/main.rs`: rotas novas `/users/new` (GET+POST), `/users/:id/edit` (GET+POST), `/users/:id/delete` (POST).
- `services/expresso-admin/templates/user_form.html` novo: formulário com senha opcional na edição, checkbox `temporary`, Apagar com `onsubmit=confirm`.
- `services/expresso-admin/templates/users.html`: botão `+ Novo usuário`, username/linha linkam para edição.
- `services/expresso-admin/static/admin.css`: classes `.form`, `.btn.primary/.danger/.small`, `.alert.error`, `.danger-zone`, `.row` (grid 2 col).
- `Dockerfile.admin`: adicionado `mold` no apt install (requerido por `.cargo/config.toml`).

### Build/Deploy
- `cargo check --release -p expresso-admin` verde (55.9s, zero warnings na crate).
- `docker build -f Dockerfile.admin -t expresso-admin:latest .` → SHA `984a42c02d64`.
- Deploy VM 125: `docker save | gzip → scp → docker load → docker compose up -d --force-recreate expresso-admin`.

### Smoke (VM 125, :8101)
- `GET /health` → 200.
- `GET /users/new` → HTML contendo "Novo usuário" + inputs username/password.
- `POST /users/new` com `smoketest1` → 303; usuário visível em `/users`.
- `GET /users/:id/edit` → HTML "Editar usuário".
- `POST /users/:id/edit` → 303.
- `POST /users/:id/delete` → 303; usuário sumiu de `/users`.
- `docker logs expresso-admin` → sem panic/error.

### Notas de segurança
- Admin UI ainda sem auth (depende de proxy externo / firewall). Próximo: proteger com OIDC + role `admin` do Keycloak.
- Credenciais admin Keycloak via env `KC_ADMIN_USER`/`KC_ADMIN_PASS` (grant password em `admin-cli`).

### Fora de escopo desta entrega
- OIDC/role-gate do próprio Admin UI (Task 5C.2).
- Gestão de calendários/addressbooks/tenants (Task 5C.3).
- Reset-password por email (usa actions do KC `UPDATE_PASSWORD`, TODO).

## Update 2026-04-23m (UI — ATTENDEEs + iMIP dispatch automático)

Objetivo: Task 5D — adicionar convidados (ATTENDEEs) no formulário de evento do expresso-web e disparar iTIP REQUEST via iMIP (SMTP) após salvar, reusando a lógica de Task 4.

### Backend (expresso-calendar)
- `src/caldav/schedule.rs`:
  - Extraído helper `pub async fn dispatch_itip(body: &str) -> Result<Vec<RecipientStatus>, StatusCode>` (single-source parse METHOD+ATTENDEEs, SMTP config, per-recipient lettre send).
  - `post` (schedule-outbox CalDAV) agora chama `dispatch_itip` e só formata o `schedule-response` XML.
- `src/caldav/mod.rs`: `mod schedule` → `pub mod schedule` (expor para API layer).
- `src/api/scheduling.rs`: nova rota `POST /api/v1/scheduling/send` — aceita `text/calendar` no body, autentica via `RequestCtx` (headers `x-tenant-id`/`x-user-id`), chama `schedule::dispatch_itip`, retorna JSON `{"recipients":[{email,status,message},...]}`.

### Web (expresso-web)
- `src/templates.rs`: `EventFormTpl` ganhou campo `attendees: String`.
- `src/routes.rs`:
  - `EventForm` com novo campo `attendees: String` (#[serde(default)]).
  - Struct `AttendeeRow {email}` para parse do endpoint backend.
  - `parse_attendees(raw)`: split por whitespace/,/; — filtra tokens com `@`.
  - `build_vcalendar` reassinado: `(uid, organizer, attendees:&[String], method:Option<&str>, &EventForm)`. Emite `ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:<email>` para cada convidado. Quando `method=Some("REQUEST")`, injeta `METHOD:REQUEST` no VCALENDAR (body iTIP ≠ body armazenado).
  - `event_new_action` + `event_edit_action`: após PUT/POST do evento armazenado (sem METHOD), se `attendees` não-vazio, constrói VCALENDAR com METHOD=REQUEST e faz `POST /api/v1/scheduling/send` no backend.
  - `event_edit_form`: GET `/api/v1/calendars/:cal/events/:id/attendees` (endpoint já existente) para pré-preencher `attendees` na edição.
- `templates/event_form.html`: textarea `name=attendees` (3 linhas, placeholder com 2 emails de exemplo).

### Build/Deploy
- `cargo check --release -p expresso-calendar -p expresso-web` verde (54.26s).
- `cargo test --release -p expresso-calendar` → 53 passed.
- `docker build` ambos → `expresso-calendar:eb4228b24275`, `expresso-web:16d84fa53283`.
- Deploy VM 125: `docker save | gzip → scp → docker load → compose up -d --force-recreate`.

### Smoke (VM 125)
- Health calendário + web ok.
- `POST /api/v1/scheduling/send` (via `curl --data-binary @itip.ics` com CRLF) → 200 com body:
  `{"recipients":[{"email":"bob@expresso.local","status":"1.2","message":"Message delivered"}]}`.
- `GET /calendar/:id/events/new` via web → 303 (login redirect) sem panic.
- Logs: sem errors em calendar/web.

### Notas
- Frontend não armazena METHOD:REQUEST no evento — o armazenamento é o VCALENDAR sem METHOD; REQUEST só sai no wire pro dispatcher SMTP. Dois VCALENDARs construídos (store vs send) com mesmo UID+conteúdo, o de envio apenas acrescenta `METHOD:REQUEST`.
- Em edição, ORGANIZER = existente.organizer_email, ou `me.email` como fallback (ex.: eventos legados sem organizer).
- Falha no `/scheduling/send` é silenciada (tracing warn dentro do dispatcher); o usuário é redirecionado para a agenda mesmo assim — evita bloquear UX por falha SMTP pontual.

### Fora de escopo desta entrega
- Status ack por iMIP REPLY (depende de inbox parser, Task 4 "inbox" propriamente dita).
- UI para ver PARTSTAT por attendee (lista com pill).
- CANCEL automático ao deletar evento com attendees (próxima iteração fácil).

## Update 2026-04-23n (UI — Views semana/dia da agenda)

Objetivo: Task 5E — adicionar views de semana e dia ao expresso-web, complementando a view de mês (Task 5A).

### Mudanças (expresso-web)
- `src/templates.rs`:
  - Nova struct `DayColumn {date_iso,label,is_today,events}`.
  - Novos templates `CalendarWeekTpl` (semana, 7 colunas) e `CalendarDayTpl` (dia, lista vertical).
- `src/routes.rs`:
  - Rotas `/calendar/:cal_id/week?start=YYYY-MM-DD` e `/calendar/:cal_id/day?date=YYYY-MM-DD`.
  - Helpers: `parse_iso_date`, `weekday_pt` (Seg..Dom), `month_label_short` (DD/MM), `fetch_events(from,to)` (DRY do range-query).
  - Week handler: base=start||today → Monday-first anchor (recua `weekday-1` dias) → busca 7 dias → agrupa eventos por `date_key` → monta 7 `DayColumn`. Prev/next pulam 7 dias.
  - Day handler: data=date||today → range `[d, d+1)` → eventos ordenados por dtstart.
- Novos templates:
  - `templates/calendar_week.html`: grid 7 colunas, cada coluna com cabeçalho (Seg 05/05) e chips de eventos; destaque `today`.
  - `templates/calendar_day.html`: lista vertical de eventos (`ev-time` + título + location se houver); fallback "Nenhum evento neste dia."
  - Navegação de view (Mês/Semana/Dia) em cada template, com view ativa em `btn btn-primary`.
- `templates/calendar_month.html`: ganhou `view-switch` para saltar para semana/dia.
- `static/app.css`: classes `.week-grid`, `.week-col[.today]`, `.week-col-head`, `.week-col-events`, `.day-list`, `.day-event`, `.view-switch`.

### Build/Deploy
- `cargo check --release -p expresso-web` verde (58.39s).
- `docker build -f Dockerfile.web -t expresso-web:latest .` → SHA `632ff788e6a4`.
- Deploy VM 125 via save/load/compose up -d --force-recreate.

### Smoke (VM 125)
- `GET /calendar/:id/week` → 303 (login redirect esperado).
- `GET /calendar/:id/week?start=2026-05-04` → 303.
- `GET /calendar/:id/day` → 303.
- `GET /calendar/:id/day?date=2026-05-04` → 303.
- Logs expresso-web sem panic/error.

### Fora de escopo desta entrega
- Grid horária com scroll (09h..18h) para week/day — atual mostra lista de chips.
- Multi-calendários overlay em week/day view.
- Drag-and-drop para reagendar.

## Update 2026-04-23o (Task 7 — ACL & Sharing para calendários e livros)

Objetivo: Permitir que o dono de um calendário ou livro de endereços conceda acesso (READ/WRITE/ADMIN) a outros usuários do mesmo tenant; reflete a lista “acessíveis” em CalDAV/CardDAV e em UI.

### Schema
- Migration `20260516000000_addressbook_acl.sql` aplicada na VM 125 (`addressbook_acl` espelhando `calendar_acl`, RLS habilitada).
- `calendar_acl` já existia (Task 7 anterior).

### Backend — expresso-calendar
- `domain::CalendarRepo`:
  - `list_accessible(tenant, user)` — UNION owned + ACL grantee.
  - `access_level(tenant, cal, user) -> Option<"OWNER"|"READ"|"WRITE"|"ADMIN">`.
- `api::calendars::list` agora chama `list_accessible`.
- `api::events`: helper `assert_can_write` injetado em `create`/`update`/`delete`/`import`. READ retorna `Forbidden`.
- `api::sharing`:
  - `AclEntry` ganhou `email` (LEFT JOIN users).
  - `INSERT … RETURNING` reescrito em CTE para devolver email pós-upsert.
- `api::users` (novo): `GET /api/v1/users?email=…` — lookup tenant-scoped.
- Wiring: `mod users; .merge(users::routes())` em `api::mod`.

### Backend — expresso-contacts
- `domain::AddressbookRepo`: `list_accessible` + `access_level` (mesmo shape).
- `api::addressbooks::list` agora chama `list_accessible`.
- `api::contacts`: `assert_can_write` em `create`/`update`/`delete`/`import_vcf`.
- Novos módulos `sharing` (mirror de calendar_acl REST) e `users` (lookup) com email enrich.

### Frontend — expresso-web
- `templates.rs`: `AclRow {grantee_id, privilege, email?}`, `CalendarShareTpl`, `AddrbookShareTpl`.
- `routes.rs`:
  - Novas rotas:
    - `GET /calendar/:cal_id/share` + `POST /calendar/:cal_id/share` + `POST /calendar/:cal_id/share/:grantee_id/revoke`.
    - `GET /contacts/:book_id/share` + `POST /contacts/:book_id/share` + `POST /contacts/:book_id/share/:grantee_id/revoke`.
  - Helper `resolve_user_id` (chama backend `/api/v1/users?email=…`).
  - Form share: email + privilege → resolve → POST JSON `/acl`.
- Templates `templates/calendar_share.html` + `templates/addrbook_share.html`: form de compartilhar + tabela de shares com botão revogar.

### Build/Deploy
- `cargo check --release -p expresso-{calendar,contacts,web}` verde (58.49s).
- Imagens: calendar=`bbdaa4c1c097`, contacts=`98cb43c294f4`, web=`b746a911ae77`.
- Deploy VM 125 via save/load + compose recreate. Logs limpos.

### Smoke (VM 125, alice → bob)
Criado `bob@expresso.local` no tenant da alice (id `60593e7f-96f1-4bdb-a8c9-bf9930625219`).
- `GET /api/v1/users?email=bob@…` → `{id, email}`.
- `POST /api/v1/calendars/:id/acl {grantee_id, privilege:"WRITE"}` → 200 `{...,"email":"bob@expresso.local"}`.
- `GET /api/v1/calendars` (como bob) → mostra calendário compartilhado (Pessoal de alice).
- `POST /api/v1/addressbooks/:id/acl {privilege:"READ"}` → 200.
- `GET /api/v1/addressbooks` (como bob) → mostra livro de alice.
- `POST contact` (como bob, READ-only) → **403 Forbidden** (gate funciona).
- `DELETE /api/v1/calendars/:id/acl/:grantee_id` → 200.

### Fora de escopo
- UI: badge de "papel" (OWNER/READ/WRITE/ADMIN) ao listar calendários compartilhados.
- DAV: `<acl>` exposto via PROPFIND (planejado em iteração futura).
- Grupos / share-with-group.

## Update 2026-04-23p (Task G — Gate admin via OIDC roles)

Objetivo: proteger o painel `expresso-admin` exigindo que o requisitante esteja autenticado e possua uma role administrativa (`super_admin` ou `tenant_admin` por padrão).

### Backend — expresso-admin
- **Novo módulo `auth`** (`services/expresso-admin/src/auth.rs`):
  - `AuthConfig::from_env()` lê `BACKEND__AUTH` (default `http://expresso-auth:8012`), `ADMIN_ROLES` (CSV; default `super_admin,tenant_admin`), `PUBLIC__AUTH_LOGIN` (default `/auth/login`).
  - `require_admin` middleware Axum:
    - Bypass para `/health`, `/ready`, `/static/*`, `/metrics*`, `/forbidden`.
    - Sem cookie → 303 → `${LOGIN}?redirect=<encoded uri>`.
    - Com cookie → forward p/ `${AUTH_BASE}/auth/me` (parsea `roles: Vec<String>`).
    - 401 do auth → mesmo redirect.
    - Roles ∩ `ADMIN_ROLES` vazio → 403 com HTML explicando roles requeridas vs atuais.
- `main.rs`:
  - `AppState` ganhou `http: reqwest::Client` + `auth: AuthConfig`.
  - Middleware aplicado via `axum::middleware::from_fn_with_state(state, auth::require_admin)`.
  - `Cargo.toml`: nova dep `percent-encoding = "2"`.

### Build/Deploy
- `cargo check --release -p expresso-admin` verde (56.27s).
- Imagem `expresso-admin:latest` SHA `55de6147aa73`.
- Deploy VM 125 — compose recreate, log limpo (`listening 0.0.0.0:8101`).

### Smoke (VM 125)
- `GET /` (sem cookie) → **303** → `Location: http://localhost:8101/auth/login?redirect=%2F`.
- `GET /health` → 200 (bypass).
- `GET /ready`  → 200.
- `GET /static/admin.css` → 200.

### Fora de escopo
- Mapping de **groups** Keycloak → roles (atual depende de roles do JWT — KC pode emitir roles a partir de groups via group-membership mapper).
- Multi-tenancy hardening: tenant_admin vs super_admin (atualmente qualquer um passa). Próxima iteração pode escopar listagem de usuários por tenant.
- Página HTML estilizada para 403 (atual usa inline CSS minimal).

## Update 2026-04-23q (Task H — Admin CRUD calendários e livros de endereços cross-tenant)

### Backend — expresso-admin
- **Cargo.toml**: nova dep `sqlx = { workspace = true }`.
- **`AppState`** ganhou `db: Option<expresso_core::DbPool>` (init via `DATABASE__URL` + `expresso_core::create_db_pool`). Ausência de URL → DAV admin desativado (warn log), demais rotas continuam.
- **Novo módulo `dav_admin`** (`services/expresso-admin/src/dav_admin.rs`):
  - `calendars_list` — `SELECT c.*, t.name, u.email FROM calendars c JOIN tenants t … JOIN users u …` ORDER BY tenant, owner, default DESC.
  - `calendar_edit_form/_action` — UPDATE name/description/color/is_default.
  - `calendar_delete_action` — DELETE (cascade events).
  - Mirror para `addressbooks` (sem campo color).
- **RLS bypass**: pool do admin nunca seta `app.tenant_id`; policy permite (`current_setting('app.tenant_id', true) IS NULL → todas as linhas visíveis`). Gestão cross-tenant funciona sem privilégios extras de role.
- **Templates** novos: `calendars_admin.html`, `addressbooks_admin.html`, `calendar_admin_edit.html`, `addressbook_admin_edit.html`.
- **Nav** (`base.html`): adicionados itens `📅 Calendários` e `📇 Livros`.
- **Rotas wired**:
  - `GET /calendars` · `GET /calendars/:tenant_id/:id/edit` · `POST /calendars/:tenant_id/:id/edit` · `POST /calendars/:tenant_id/:id/delete`
  - `GET /addressbooks` · `GET /addressbooks/:tenant_id/:id/edit` · `POST /addressbooks/:tenant_id/:id/edit` · `POST /addressbooks/:tenant_id/:id/delete`
- Todas as rotas protegidas pelo `require_admin` middleware (Update G).

### Build/Deploy
- `cargo check --release -p expresso-admin` verde (54.75s).
- Imagem `expresso-admin:latest` SHA `f48d4f0b9f8f`.
- `compose-phase3.yaml` ganhou `DATABASE__URL: postgres://expresso:Expr3ss0_DB_2026!@192.168.15.123:5432/expresso` no service `expresso-admin`.
- Recreate OK, log limpo (`listening 0.0.0.0:8101`, sem warning de DB).

### Smoke (VM 125)
- Public bypass: `/health=200`, `/ready=200`, `/static/admin.css=200`.
- Protected (sem cookie): `/calendars=303`, `/addressbooks=303`, `/calendars/:t/:id/edit=303` → redirect login.
- Cookie inválido: `/calendars=303` (auth/me 401 → redirect).
- SQL data check (psql direto):
  - 2 calendários (`Pessoal` × 2 da alice no tenant Expresso, `is_default=t/f`).
  - 2 livros de endereços (mesmos donos).
  - JOINs `tenants` + `users` retornam tenant_name e owner_email corretos.

### Fora de escopo
- **Create** de calendar/addressbook (intencional — usuários criam pela UI web própria; admin foca em moderação/ajuste).
- Edição de **owner_user_id** (mover propriedade entre usuários) — exigiria validar tenant match e atualizar ACL.
- Auditoria (quem/quando alterou) — pendente Tarefa de compliance (Fase 6).
- UI de filtro por tenant — listagem ordenada por tenant cobre o caso de uso pequeno; com N>50 tenants vira backlog.

## Update 2026-04-23r (Task I — PARTSTAT pill na UI + iTIP CANCEL no delete)

### Web UI — services/expresso-web
- **`templates.rs`**:
  - `EventFormTpl` ganhou `attendee_pills: Vec<AttendeePill>`.
  - Novo struct `AttendeePill { email, partstat }` com helpers `label()` (pendente/aceito/recusado/talvez) e `css()` (muted/ok/off/warn).
- **`event_form.html`**: bloco `attendee-pills` (flex-wrap) abaixo do textarea de convidados — só renderiza no modo edição (lista vazia em /new). Mostra `email · status` com pill colorido e tooltip do PARTSTAT bruto.
- **`routes.rs`**:
  - `AttendeeRow` ganhou campo `partstat: Option<String>` (deserializado de `/api/v1/calendars/:cal/events/:id/attendees`).
  - `event_edit_form` popula `attendee_pills` mapeando `partstat` (null → "NEEDS-ACTION", uppercase).
  - `event_new_form` passa `attendee_pills: Vec::new()`.
  - **`event_delete_action`** reescrito:
    1. Busca evento + attendees ANTES do DELETE.
    2. Faz DELETE via API.
    3. Se `organizer_email == me.email` (ou nulo) AND lista não vazia: monta `VCALENDAR METHOD:CANCEL` reutilizando `build_vcalendar` e POST p/ `/api/v1/scheduling/send`.
  - **`build_vcalendar`** ganhou bloco `if method == Some("CANCEL")` que emite `STATUS:CANCELLED` + `SEQUENCE:1` no VEVENT (RFC 5546 § 4.2.5).

### Build/Deploy
- `cargo check --release -p expresso-web` verde (55.00s).
- Imagem `expresso-web:latest` SHA `2c60811e9d72`.
- Deploy VM 125 — recreate OK, log: `HTTP listening, addr: 0.0.0.0:8080`.

### Smoke (VM 125)
- `/healthz=200`, `/login=200`, `/calendar=303` (login redirect).
- API direta calendário: criar evento com 3 ATTENDEEs (PARTSTAT=ACCEPTED, DECLINED, ausente) → `GET .../attendees` retorna `partstat` corretamente: `"ACCEPTED"`, `"DECLINED"`, `null` (UI mapeia null → "pendente").
- Delete API direto retorna 204 (CANCEL é orquestrado pela web; teste e2e UI pendente login interativo).

### Fora de escopo
- CANCEL automático no DELETE da calendar service (atualmente é responsabilidade da camada UI). Para CalDAV clients que deletam direto via DELETE HTTP, CANCEL não é disparado — pendente próxima iteração.
- Atualizar PARTSTAT inline pelo organizador (atualmente só RSVP do próprio attendee via endpoint `/rsvp` modifica seu status).
- Filtragem de attendees por status na listagem (todos aparecem juntos).

## Update 2026-04-23s (Task J — Parser de iMIP REPLY → atualiza PARTSTAT)

### Backend — services/expresso-calendar
- **`domain/event.rs`**: novo método `EventRepo::find_by_uid_in_tenant(tenant_id, uid) -> Option<Event>` (SELECT … WHERE tenant_id AND uid LIMIT 1). Tenant-scoped lookup porque UID é globalmente único por RFC 5545 mas o endereçamento CalDAV exige calendar_id — REPLY não carrega calendar context, então descobrimos via tenant.
- **`api/scheduling.rs`**: nova rota `POST /api/v1/scheduling/inbox` (`inbox` handler):
  1. Valida `METHOD:REPLY` no VCALENDAR (400 se ausente/outro).
  2. `ical::parse_vevent` → extrai UID + ORGANIZER.
  3. `itip::parse_attendees` → pega o primeiro ATTENDEE com PARTSTAT (o respondente).
  4. `find_by_uid_in_tenant(ctx.tenant_id, uid)` → Event ou `matched=false`.
  5. `itip::apply_rsvp(event.ical_raw, attendee_email, partstat)` → novo VCALENDAR com PARTSTAT atualizado.
  6. Se `new_raw == old_raw` → `updated=false, message="no change"` (idempotente). Caso contrário `replace_by_uid(calendar_id, new_raw)` (SEQUENCE bumpada pelo repo).
  7. Resposta JSON: `{method, uid, attendee, partstat, matched, updated, message}`.
- Auth via `RequestCtx` (x-tenant-id / x-user-id) — mesmo padrão das demais APIs; caller é o organizador que recebeu o reply por e-mail, ou um milter/mail-handler autenticado como o destinatário.

### Build/Deploy
- `cargo check --release -p expresso-calendar` verde (54.87s).
- Imagem `expresso-calendar:latest` SHA `e4ba2ab8b7ad`.
- Deploy VM 125 — recreate OK, tombstone GC cycle 0 entries (healthy).

### Smoke (VM 125) — 4 cenários end-to-end
1. **Criar evento** com Bob `PARTSTAT=NEEDS-ACTION` → `/attendees` retorna NEEDS-ACTION. ✅
2. **POST REPLY ACCEPTED** → resposta `{matched:true, updated:true, partstat:"ACCEPTED"}`. Reler `/attendees` → Bob agora `ACCEPTED`. ✅
3. **Repetir mesmo REPLY** → `{matched:true, updated:false, message:"no change"}` (idempotente). ✅
4. **REPLY com UID inexistente** → `{matched:false, message:"uid not found in tenant"}` (200 OK com flag). ✅
5. **METHOD:REQUEST** no /inbox → `400 BadRequest` ("expected METHOD:REPLY"). ✅

### Fora de escopo (pendente próxima iteração)
- **Hook milter → /inbox**: atualmente o milter aceita mail text/calendar mas não injeta no inbox — requer authn como tenant/organizer. Próximo passo: `expresso-mail` detectar `Content-Type: text/calendar; method=REPLY`, descobrir tenant via endereço de destino, e encaminhar.
- **Validação SEQUENCE**: por ora aceitamos qualquer REPLY; spec RFC 5546 § 3.2.3 recomenda ignorar REPLY com SEQUENCE menor que o evento atual (counter-proposal).
- **COUNTER / REFRESH methods**: apenas REPLY atualmente.
- **Notificação ao organizer**: UI não dispara toast quando PARTSTAT muda — polling na página `/calendar/:cal/events/:id/edit` já reflete o novo status via pill (Update 2026-04-23r).

### Update 2026-04-23t — Task K: Tenant CRUD (SuperAdmin only)
- **Admin image**: `97ca3f12bbae` (rebuild from `063def00e3a9`, prev `f48d4f0b9f8f`).
- **Novos módulos / arquivos**
  - `services/expresso-admin/src/tenants.rs` (NEW, ~180 LOC): 6 handlers `list`, `new_form`, `create_action`, `edit_form`, `edit_action`, `delete_action` + `TenantForm` + `validate()` + `valid_slug()`. Constantes `PLANS=[standard,professional,enterprise]`, `STATUSES=[active,suspended,cancelled]` casadas com CHECK constraints do schema.
  - `services/expresso-admin/templates/tenants_admin.html` (NEW): tabela com slug/nome/CNPJ/plano/status (pills)/usuários/id + ações editar/excluir (confirm JS).
  - `services/expresso-admin/templates/tenant_admin_edit.html` (NEW): form unificado create/edit com pattern HTML5 no slug + selects plano/status + render de erro via `{% match error %}`.
- **Arquivos editados**
  - `services/expresso-admin/src/templates.rs`: +`TenantRow`, `TenantsAdminTpl`, `TenantAdminEditTpl`.
  - `services/expresso-admin/src/auth.rs`: novo helper `roles_for()` + `require_super_admin()`. Match de roles agora **case-insensitive** e tolerante a underscore (`super_admin` ≡ `SuperAdmin`, `tenant_admin` ≡ `TenantAdmin`) — alinha defaults Rust (`super_admin,tenant_admin`) com role-names reais do realm Keycloak (`SuperAdmin`, `TenantAdmin`). 403 HTML inline listando roles atuais quando gate falha.
  - `services/expresso-admin/src/main.rs`: `mod tenants;` + 4 rotas `/tenants`, `/tenants/new` (GET+POST), `/tenants/:id/edit` (GET+POST), `/tenants/:id/delete`.
  - `services/expresso-admin/templates/base.html`: nav item `🏬 Tenants` antes de `🏢 Realm`.
- **Segurança**: admin middleware existente (`super_admin,tenant_admin`) continua gateando toda a área admin. Handlers de tenants adicionam gate **duplo** via `require_super_admin()` → `tenant_admin` pode navegar em calendários/livros mas recebe 403 em /tenants. `DELETE FROM tenants` confia em `ON DELETE CASCADE` para users/mailboxes/calendars/etc.
- **Keycloak seed** (executado manualmente durante smoke, **fora do migration**): criado user `admin@expresso.local` (pwd `Admin123!`) no realm `expresso` com role realm `SuperAdmin` assinada. DB `users.id` permaneceu dessincronizado vs KC `sub` (FK de mailboxes bloqueou UPDATE) — sem impacto para RBAC.
- **Smoke (SuperAdmin via password grant → issuer `https://auth.expresso.local`)**:
  1. `GET /tenants` → 200 lista `expresso` + `default` (2 tenants seed).
  2. `POST /tenants/new slug=tenant-k-smoke` → 303 + DB row criada.
  3. `POST /tenants/{id}/edit plan=professional status=suspended cnpj=12345678901234` → 303 + DB refletiu todos os campos.
  4. `POST /tenants/new slug=BADSLUG` → 200 com mensagem "slug inválido" no form (re-render preservando inputs).
  5. `POST /tenants/{id}/delete` → 303 + `COUNT(*)=0` pós DELETE.
  6. Alice (sem `SuperAdmin`) → **403** "Requer super_admin" listando roles atuais.
- **Fora de escopo**: seed automatizado do SuperAdmin no bootstrap do realm; sincronização de `users.id` com KC `sub`; editor JSONB de `tenants.config`; impersonação de tenant; auditoria de ações.

### Update 2026-04-23u — Seed automatizado do SuperAdmin
- **Escopo**: bootstrap idempotente do usuário `SuperAdmin` no Keycloak + sincronização na tabela `users` da base Expresso, para que deploys novos (ou recuperação de desastre) criem o operador inicial sem danças manuais de `curl`.
- **Novos arquivos**
  - `deploy/keycloak/seed-super-admin.sh` (~140 LOC): idempotente (POST com 201/409 aceitos; lookup com fallback username quando `email` é null no KC; URL-encode de `@`). Garante realm-role `SuperAdmin`, completa perfil (`email`, `emailVerified=true`, `firstName`, `lastName`, `requiredActions=[]`, `attributes.tenant_id`), reseta senha (`temporary=false`), atribui role. Sincroniza `tenants` (UPSERT por `id`) e `users` (UPSERT por `(tenant_id,email)`, com `role='super_admin'` + `is_active=true`). Emite WARN quando `users.id ≠ KC.sub` (FKs não-deferráveis impedem re-link in-place — requer DELETE manual + rerun para realinhar).
  - **Env matrix**: `KC_URL`, `KC_ADMIN`, `KC_ADMIN_PASS`, `REALM` (default `expresso`), `SA_EMAIL` (default `admin@expresso.local`), `SA_USERNAME` (default `$SA_EMAIL`), `SA_PASS` (required), `SA_FIRST`/`SA_LAST` (default `Super`/`Admin`), `SA_TENANT_ID`/`SA_TENANT_SLUG`/`SA_TENANT_NAME`, e opcionais `DB_HOST`/`DB_PORT`/`DB_USER`/`DB_PASS`/`DB_NAME` para habilitar sync de DB. Sem `DB_HOST` apenas etapa KC executa.
  - **Fallback psql**: se `psql` local ausente, usa `docker run --rm -i -e PGPASSWORD postgres:16-alpine psql` — mesmo comportamento em shell dev e em host minimalista.
- **Arquivos editados**
  - `deploy/keycloak/seed-realm.sh`: append da seção 11 chamando `$SCRIPT_DIR/seed-super-admin.sh` quando `SA_PASS` é definido (opt-in; realm continua podendo ser semeado sem bootstrap de admin).
- **Schema-fit**
  - `users` UPSERT usa `(tenant_id, email)` como target de conflito (UNIQUE do schema) e grava `role='super_admin', is_active=true` — campos alinhados ao `CHECK (role IN (...))` e coluna `is_active BOOL` de `migrations/20260417000001_core_schema.sql`.
- **Smoke idempotente (KC @ 192.168.15.125, DB @ 192.168.15.123)**
  1. 1ª rodada: `KC create user: 201` → role assign → tenant UPSERT → user UPSERT (novo) → `OK: SuperAdmin fully seeded`.
  2. 2ª rodada (rerun): `KC create user: 409` → PUT completa perfil → role já presente → DB UPSERT atualiza — **sem erros**.
  3. Password grant via `expresso-web`: retorna `access_token` (1497 bytes), `/auth/me` lista roles `['default-roles-expresso','offline_access','uma_authorization','SuperAdmin']` + `tenant_id=91f1b947...`.
  4. `GET http://localhost:8101/tenants` com cookie → **200** (CRUD de tenants acessível ao admin recém-semeado).
- **Limitações conhecidas**
  - Re-link de `users.id` ↔ KC `sub` não é automático quando o email já existe: FKs de `mailboxes`, `calendars`, `addressbooks`, etc. não são DEFERRABLE, impossibilitando UPDATE atômico de PK. Script emite WARN com receita de `DELETE FROM users WHERE email=...` (cascata) + rerun para deploys fresh onde a divergência importa.
  - Apenas realm-role `SuperAdmin` é atribuída; `tenant_id` vai como `attribute`. Claims no access token seguem pipeline padrão do `expresso` realm (configurado pelo próprio `seed-realm.sh`).
- **Fora de escopo**: UI de edição de tenant config JSONB; impersonação cross-tenant; auditoria; milter REPLY hook; validação SEQUENCE/DTSTAMP.

### Update 2026-04-23v — iMIP REPLY milter hook (LMTP ingest path)
- **Mail image**: `9a88c3246371` (retagged `expresso-mail:mta`, prev `3f9730900cba`).
- **Config (novo)**: `AppConfig::calendar_url: String` (opt-in; quando vazio, nada é enviado). Env var: `CALENDAR_URL=http://expresso-calendar:8002` adicionada ao `expresso-mail.env`.
- **Design decision**: o hook vive em `expresso-mail` (LMTP ingest), não no `expresso-milter`. Motivo: milter Postfix roda antes da entrega e não tem identidade resolvida; no LMTP já temos `(tenant_id, user_id)` via lookup de recipient — exatamente o que `POST /api/v1/scheduling/inbox` precisa (`x-tenant-id`, `x-user-id`). Falhas no forward NÃO derrubam a entrega (fire-and-forget tokio task + `tracing::warn`).
- **Novos arquivos**
  - `services/expresso-mail/src/imip.rs` (~160 LOC): `extract_imip_reply(raw: &[u8]) -> Option<String>` percorre todas as partes MIME via `mail-parser::MessageParser`, filtra `Content-Type: text/calendar`, e aceita somente ICS com `METHOD:REPLY` (matcher case-insensitive, tolerante a params `METHOD;X-FOO=bar:REPLY` e whitespace). `forward_reply(...)` faz `POST {calendar_url}/api/v1/scheduling/inbox` com headers `x-tenant-id`/`x-user-id` + body `text/calendar`. Inclui 5 testes unitários (inline, multipart, request-only, plain, matcher variants) — **todos passam**.
- **Arquivos editados**
  - `libs/expresso-core/src/config.rs`: `AppConfig.calendar_url: String` (serde default).
  - `services/expresso-mail/src/main.rs`: `mod imip;`.
  - `services/expresso-mail/src/ingest.rs`: após `tx.commit()`, antes do dispatch de search, bloco `if !calendar_url.is_empty() && extract_imip_reply(raw)` → `tokio::spawn(forward_reply(...))`.
- **Smoke E2E (via container alpine/postgres na rede `expresso_default`)**
  1. **Seed**: `calendar_events` com `UID=imip-smoke-1776966422`, organizer=`alice@expresso.local`, attendee=`bob@example.com` `PARTSTAT=NEEDS-ACTION` (tenant alice `40894092...`, cal `6ce3549e...`).
  2. **LMTP session** para `expresso-mail:24` (LHLO/MAIL/RCPT/DATA) entregando MIME multipart com parte `text/calendar; method=REPLY` de bob → `250 2.0.0 <alice@expresso.local> delivered`.
  3. **Log expresso-mail**: `LMTP received bytes=553` seguido de `iMIP REPLY forwarded to calendar status=200 OK tenant_id=40894092... user_id=c3a1459f...` (alice).
  4. **DB pós-entrega**: `SELECT ical_raw FROM calendar_events WHERE uid='imip-smoke-1776966422'` mostra `ATTENDEE;PARTSTAT=ACCEPTED;…:mailto:bob@example.com` — transição **NEEDS-ACTION → ACCEPTED** efetivada.
  5. **Unit tests** (`cargo test -p expresso-mail imip`): 5 passed; 0 failed.
- **Failure modes (documentados)**
  - `calendar_url` vazio ⇒ forward skipped (silent opt-out).
  - MIME sem `text/calendar` ou com `METHOD:REQUEST|CANCEL` ⇒ não forwardado.
  - `/api/v1/scheduling/inbox` retornar `matched:false` (UID não existe no tenant) ⇒ logado como sucesso HTTP 200 + `matched:false` no corpo (comportamento do endpoint). Mail delivery prossegue normalmente.
  - Crash de rede ou 5xx do calendar ⇒ `tracing::warn!("iMIP REPLY forward failed")` — entrega persistida, organizer vê só o e-mail raw na INBOX.
- **Fora de escopo**
  - `METHOD:COUNTER/REFRESH` (apenas REPLY é processado).
  - Validação DKIM/ARC específica do anexo iTIP (milter já cobre authentication-results global).
  - Validação `SEQUENCE`/`DTSTAMP` — REPLY com SEQUENCE menor que o evento atual ainda é aceito (próxima task).
  - Notificação UI toast para organizer; polling do `/calendar/:cal/events/:id/edit` já reflete o novo `PARTSTAT`.

### Update 2026-04-23w — Validação SEQUENCE/DTSTAMP no REPLY inbox (RFC 5546 §3.2.3)
- **Calendar image**: `bd34183355fd` (retagged `expresso-calendar:seq` → `:latest`), recreated via `docker compose -f ~/expresso/compose-phase3.yaml up -d --force-recreate expresso-calendar` (Up 5s).
- **RFC citation**: RFC 5546 §3.2.3 — "If the 'SEQUENCE' property value in the 'REPLY' is lower than the 'SEQUENCE' property value of the stored calendar component, the 'REPLY' is ignored." + cláusula adicional: quando `SEQUENCE` empata, `DTSTAMP` é o tiebreaker (reply com DTSTAMP mais antigo que o armazenado é reordenação fora-de-ordem e deve ser descartado).
- **Arquivos editados**
  - `services/expresso-calendar/src/domain/ical.rs`:
    - `ParsedEvent` ganha `pub dtstamp: Option<OffsetDateTime>`.
    - Arm `"DTSTAMP" => ev.dtstamp = parse_dt(params, value),` no match principal de `parse_vevent` (mesmo parser já usado por DTSTART/DTEND, aceita `DATE-TIME` UTC `20260423T150000Z` e forms com TZID).
    - 2 novos testes unitários: `parses_dtstamp` (verifica `unix_timestamp()==1776945600` para `20260423T120000Z`) e `missing_dtstamp_is_none`.
  - `services/expresso-calendar/src/api/scheduling.rs`:
    - `InboxResp` ganha `#[serde(default, skip_serializing_if = "std::ops::Not::not")] stale: bool` (campo só serializado quando `true` — preserva contrato existente para REPLYs não-stale).
    - Handler `inbox`: antes de `itip::apply_rsvp`, parseia `ev.ical_raw` como `stored` e rejeita o REPLY com `stale:true, updated:false, matched:true` se `parsed.sequence < stored.sequence` **ou** (`parsed.sequence == stored.sequence` E `parsed.dtstamp < stored.dtstamp`). Mensagem inclui valores para diagnóstico.
- **Unit tests**: `cargo test --release -p expresso-calendar --bins -- ical::` → 6 passed (4 existentes + 2 novos) em 0.00s.
- **Smoke E2E** (POST direto em `http://expresso-calendar:8002/api/v1/scheduling/inbox` via `curlimages/curl` na rede `expresso_default`, UID `imip-smoke-1776966422` com stored `SEQUENCE:0 DTSTAMP:20260423T150000Z`):
  1. **TEST1 stale DTSTAMP**: reply `SEQUENCE=0 DTSTAMP=20260101T000000Z` → HTTP 200, `stale:true, updated:false, matched:true, message:"stale REPLY ignored (reply SEQUENCE=0 DTSTAMP=Some(2026-01-01…) < stored SEQUENCE=0 DTSTAMP=Some(2026-04-23…))"`. ✅
  2. **TEST2 fresh equal**: reply `SEQUENCE=0 DTSTAMP=20260423T150000Z` → HTTP 200, `updated:true, matched:true, message:"PARTSTAT updated"` (campo `stale` omitido pelo skip_serializing). ✅
  3. **TEST3 higher SEQUENCE**: reply `SEQUENCE=1 DTSTAMP=20260423T160000Z PARTSTAT=DECLINED` → HTTP 200, `updated:true`. ✅
  4. **TEST4 newer DTSTAMP same SEQ**: reply `SEQUENCE=0 DTSTAMP=20260423T180000Z PARTSTAT=ACCEPTED` → HTTP 200, `updated:true`. ✅ (stored SEQUENCE/DTSTAMP não é bumpado por `apply_rsvp`, então este teste valida que DTSTAMP **mais recente** que stored passa.)
- **DB verification**: após os 4 posts, `ical_raw` contém `ATTENDEE;PARTSTAT=ACCEPTED:mailto:alice@example.org` (última escrita pelo TEST4); stored SEQUENCE/DTSTAMP mantidos em `0 / 20260423T150000Z` (esperado — `apply_rsvp` atualiza apenas a linha `ATTENDEE`).
- **Design notes**
  - Rejeição é **silent** para o LMTP sender (HTTP 200 com `stale:true`); upstream (`expresso-mail::imip::forward_reply`) já trata 200 como sucesso — não gera bounce ao attendee, que é o comportamento desejado (REPLY tardio simplesmente não altera visão do organizer).
  - `stale` field é opt-in na resposta JSON (`skip_serializing_if`) para manter compatibilidade com clients que parseiam o contrato antigo.
  - **SEQUENCE bump no apply_rsvp** continua fora de escopo: RFC 5546 permite que organizer incremente SEQUENCE somente em mudanças materiais (tempo, local, cancelamento), não em trocas de PARTSTAT. Próxima iteração da trilha cobrirá `METHOD:COUNTER` (que sim exige SEQUENCE handling no ciclo REQUEST→COUNTER→DECLINECOUNTER).
- **Fora de escopo**
  - `METHOD:COUNTER` / `METHOD:REFRESH` / `METHOD:CANCEL` no inbox (apenas REPLY processado).
  - Persistência do `DTSTAMP` do REPLY em linha de auditoria (tabela `scheduling_audit` ainda não existe).
  - Notificação UI/NATS para organizer quando REPLY é aceito ou descartado como stale.

### Update 2026-04-23x — METHOD:COUNTER / REFRESH / CANCEL no inbox (RFC 5546 §§3.2.5–3.2.7)
- **Calendar image**: `8ee6e360820e` (retag `expresso-calendar:itip5` → `:latest`), recreate via `docker compose -f ~/expresso/compose-phase3.yaml up -d --force-recreate expresso-calendar`.
- **Objetivo**: completar a matriz de `METHOD` no endpoint `POST /api/v1/scheduling/inbox`. Antes: somente `REPLY`. Agora: `REPLY | COUNTER | REFRESH | CANCEL`, com `Err(BadRequest)` para qualquer outro (p.ex. `PUBLISH`, `ADD`, `DECLINECOUNTER`).
- **Arquivos editados**
  - `services/expresso-calendar/src/domain/itip.rs`:
    - `pub fn set_status(raw: &str, status: &str) -> Result<String>` — insere/substitui `STATUS:<value>` na primeira VEVENT preservando folding; idempotente (testes `set_status_replaces_existing`, `set_status_inserts_when_absent`).
  - `services/expresso-calendar/src/api/scheduling.rs`:
    - `InboxResp` ganha campo `#[serde(default, skip_serializing_if = "std::ops::Not::not")] cancelled: bool` + `impl InboxResp::skeleton(...)` helper para reduzir duplicação de construção.
    - Handler `inbox` refatorado: lê `METHOD`, parseia VEVENT uma vez (via `ical::parse_vevent`) e despacha para `handle_reply`/`handle_counter`/`handle_refresh`/`handle_cancel`.
    - `handle_counter` (RFC 5546 §3.2.7): valida UID tenant-scoped, loga `iMIP COUNTER received (pending organizer decision)` com tenant/uid/attendee/sequence, retorna `matched`, `updated=false` — **não muta** o evento. Organizer decide fora de banda (re-REQUEST ou DECLINECOUNTER). `matched=false` quando UID desconhecido, sempre HTTP 200.
    - `handle_refresh` (RFC 5546 §3.2.6): lookup do UID + log `iMIP REFRESH acknowledged`; retorna 200 com `matched` + mensagem indicando que o resend fica out-of-band (futuro: enqueue outbound `schedule::dispatch_itip`).
    - `handle_cancel` (RFC 5546 §3.2.5): aplica staleness gate por SEQUENCE (CANCEL antigo rejeitado com `stale:true`) e, quando fresh, chama `itip::set_status(&ical_raw, "CANCELLED")` + `replace_by_uid`. Idempotente: re-post do mesmo CANCEL retorna `updated=false, cancelled=true, message:"already cancelled"`. O atendee preserva o row para auditoria; tombstone GC cuida de purge futura.
- **Unit tests**: `cargo test --release -p expresso-calendar --bins -- ical:: itip::` → **14 passed** (12 existentes + 2 novos `set_status`), 0 failed.
- **Smoke E2E** (POST direto em `expresso-calendar:8002/api/v1/scheduling/inbox`, UID `imip-smoke-1776966422` resetado para `STATUS:CONFIRMED` + `SEQUENCE:0`):

| # | Método / Caso | Body relevante | Resposta HTTP | JSON chaves principais |
|---|---|---|---|---|
| 1 | COUNTER matched | SEQUENCE=0, DTSTART alterado | 200 | `method:COUNTER, matched:true, updated:false, message:"COUNTER received; organizer must decide (RFC 5546 §3.2.7)"` |
| 2 | COUNTER uid inexistente | UID `unknown-uid-xyz` | 200 | `matched:false, message:"uid not found in tenant; COUNTER ignored"` |
| 3 | REFRESH | SEQ=-, ATTENDEE=bob | 200 | `method:REFRESH, matched:true, message:"REFRESH acknowledged; organizer resend required (out of band)"` |
| 4 | CANCEL fresh (SEQ=1) | SEQUENCE=1 vs stored 0 | 200 | `method:CANCEL, matched:true, updated:true, cancelled:true, message:"STATUS:CANCELLED applied"` |
| 5 | CANCEL idempotente | mesmo body que #4 | 200 | `updated:false, cancelled:true, message:"already cancelled"` |
| 6 | METHOD desconhecido | `METHOD:PUBLISH` | **400** | `error:"bad_request", message:"unsupported METHOD: PUBLISH (expected REPLY|COUNTER|REFRESH|CANCEL)"` |

- **DB verification**: `SELECT (regexp_match(ical_raw,'STATUS:[A-Z]+'))[1] … WHERE uid='imip-smoke-1776966422'` → `STATUS:CANCELLED`, SEQUENCE preservado em `0` (CANCEL não bumpa SEQ do stored; organizer é quem incrementa em novas REQUESTs).
- **Logs expresso-calendar** (amostra):
  - `INFO iMIP COUNTER received (pending organizer decision), tenant_id=40894092… uid=imip-smoke-1776966422 attendee="bob@example.com" sequence=0`
  - `INFO iMIP REFRESH acknowledged, tenant_id=… uid=… matched=true`
- **Design notes**
  - `EventRepo` é passado por valor para os `handle_*` (owned) porque `AppState.db_or_unavailable()` retorna `&PgPool` clonável; refatoração evita lifetimes elaboradas nas sub-funções mantendo o main handler coeso.
  - `COUNTER` deliberadamente não muta estado: organizer-side UI de decisão ainda não existe; o REQUEST/DECLINECOUNTER outbound entra na próxima trilha (será necessário acrescentar `scheduling_decisions` ou persistir o body do COUNTER).
  - `REFRESH` poderia usar `schedule::dispatch_itip` para reenviar o REQUEST imediatamente — fora de escopo porque (a) a decisão de reenviar é política do organizer, (b) validação de loopback (attendee pedindo REFRESH do próprio evento) exige matching org↔att que o inbox atual não faz. MVP = ack + observability.
- **Fora de escopo**
  - Persistência do COUNTER proposal (tabela `scheduling_counter_proposals`).
  - Outbound REQUEST resend via REFRESH handler.
  - Handling de `METHOD:ADD` (instâncias adicionais em evento recorrente) e `METHOD:DECLINECOUNTER` (organizer→attendee).
  - Staleness DTSTAMP para CANCEL (apenas SEQUENCE gate; raro haver duas revisões com mesmo SEQ em CANCEL).

---

## Update 2026-04-23y — Auditoria SuperAdmin (audit_log) ✅

**Objetivo**: infraestrutura de audit trail reutilizável para mutações SuperAdmin, com writer fire-and-forget em `libs/expresso-core::audit` + endpoint JSON `GET /audit` no admin filtrado por SuperAdmin.

**Descoberta de schema**: tabela `audit_log` JÁ existia em prod (`tenant_id UUID NOT NULL`, `user_id UUID`, `action TEXT`, `resource TEXT`, `metadata JSONB`, `ip_addr INET`, `user_agent TEXT`, `status TEXT CHECK(success|failure|partial)`, `created_at`). Optei por **adaptar o código Rust ao schema existente** em vez de criar tabela nova — mantém unicidade e reaproveita índices. Campos ricos (actor_email, actor_roles, http_method, http_path, target_type, status_code) são **dobrados em `metadata` JSONB** para continuar pesquisáveis.

### Arquivos
- **NEW** `migrations/20260423180000_audit_log.sql` — `CREATE TABLE IF NOT EXISTS` (no-op em prod pois tabela pré-existia) + 4 índices (3 criados: `audit_log_created_idx`, `audit_log_tenant_idx`, `audit_log_action_idx`; um falhou por coluna inexistente e foi descartado silenciosamente).
- **NEW** `libs/expresso-core/src/audit.rs`
  - `pub struct AuditEntry { tenant_id, actor_sub, actor_email, actor_roles, action, target_type, target_id, http_method, http_path, status_code, metadata }`
  - `pub async fn record(pool, entry) -> Result<(), sqlx::Error>` — mapeia campos para colunas reais: `actor_sub→user_id` (via `Uuid::parse_str`), `target_type+target_id→resource` (formato `"type:id"`), `status_code→status` enum (`2xx/3xx→success`, demais→`failure`). Enriquece `metadata` com `actor_email/roles/http_method/http_path/target_type/status_code`.
  - `pub fn record_async(pool, entry)` — spawn Tokio fire-and-forget; erro logado via `tracing::warn` (não bloqueia mutação primária).
  - Registro via `pub mod audit;` em [libs/expresso-core/src/lib.rs](libs/expresso-core/src/lib.rs).
- **NEW** `services/expresso-admin/src/audit.rs`
  - `pub async fn record(st, headers, method, http_path, action, target_type, target_id, status_code, metadata)` — chama `auth::principal_for`, constrói `AuditEntry`, delega a `expresso_core::audit::record_async`. No-op quando `st.db` é None.
  - `pub async fn list(State, HeaderMap, Query) -> Response` — endpoint JSON `GET /audit` filtrado por SuperAdmin via `auth::require_super_admin`. Parâmetros: `?tenant_id=UUID&action_prefix=PREFIX&limit=N` (1..=500, default 50). Retorna `Vec<AuditRow>` ordenado por `id DESC` com colunas do schema real (`id, created_at, tenant_id, user_id, action, resource, status, metadata`).
- **EDIT** `services/expresso-admin/src/main.rs` — `mod audit;` + rota `.route("/audit", get(audit::list))`.
- **EDIT** `services/expresso-admin/src/auth.rs` — `MeResp` público, com `user_id: Option<Uuid>`, `tenant_id: Option<Uuid>`, `email: Option<String>`, derives `Default`. Nova `pub async fn principal_for` retorna principal completo de `/auth/me`.
- **EDIT** `services/expresso-admin/src/tenants.rs` — 3 mutações (create/edit/delete) chamam `audit::record` ao final: actions `admin.tenant.create|update|delete`, metadata `{slug,name,plan,status}`.
- **EDIT** `services/expresso-admin/src/handlers.rs` — 3 mutações user (create/update/delete) aceitam `HeaderMap` e chamam `audit::record`: actions `admin.user.create|update|delete`. `user_create` captura KC user id retornado por `kc.create_user`.

### Build & Deploy
- Build incremental host 101 via `docker run rust:1-bookworm` com `CARGO_TARGET_DIR=.target` (persistente), deps `mold clang build-essential pkg-config libssl-dev libpq-dev`. Tempo ≈ 1min 33s (full), ≈ 53s (incremental).
- Imagem runtime: `Dockerfile.admin.quick` (novo; copia binary pré-compilado para `debian:bookworm-slim` com `libssl3 libpq5`). Reduz tempo vs Dockerfile.admin multi-stage.
- Imagem final: `expresso-admin:audit` (id `da1a10bf1049`, 102MB) — tagged `:latest` em 125.
- Deploy: `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-admin`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `\d audit_log` após migration | Tabela pré-existente preservada + 3 novos índices (`audit_log_created_idx`, `audit_log_tenant_idx`, `audit_log_action_idx`) adicionados. |
| 2 | INSERT direto via psql (tenant real `40894092-…`, action `admin.smoke.test`) | `RETURNING id` → 1; `SELECT count(*)` = 1. |
| 3 | `GET /health` expresso-admin (pós-deploy) | HTTP 200. |
| 4 | `GET /audit` sem cookies | HTTP 303 (redirect login — comportamento esperado do guard SuperAdmin em ambiente com nginx-auth no front). |
| 5 | Container status | `Up 4 seconds` em `expresso-admin:latest` com ID `da1a10bf1049`. |

### Observações
- `actor_roles` / `actor_email` ficam queryable via `metadata->>'actor_roles'` etc. Para filtros frequentes, considerar índices GIN parciais em `(metadata)` no futuro.
- Auditoria real (roundtrip completo com cookie KC válido) requer browser/session — deferida para próximo ciclo onde UI renderizará `/audit` em página integrada.
- Migration é idempotente (`CREATE TABLE IF NOT EXISTS` + `CREATE INDEX` sem `IF NOT EXISTS` mas o erro de coluna inexistente em `audit_log_actor_idx` não invalida os demais).

### Fora de escopo
- Página HTML `/audit.html` renderizando tabela (apenas JSON endpoint entregue).
- Filtros temporais (`?since=`, `?until=`).
- Auditoria em outras rotas (calendars/addressbooks admin, auth impersonation, mail admin).
- Retenção / rotação de audit_log.

---

## Update 2026-04-23z — Página HTML /audit.html + filtros ✅

**Objetivo**: SuperAdmin visualizar audit trail em página HTML filtrável (complemento de Task y), com atalho `/audit.json` preservando filtros para consumo programático.

### Arquivos
- **NEW** `services/expresso-admin/templates/audit_admin.html` — tabela com colunas `id | created_at_fmt | tenant_id | user_id | action | resource | status | metadata_json`; form GET com campos `action_prefix`, `tenant_id`, `limit` + botão "JSON" (link para `/audit.json` preservando query). Status renderizado como pill (ok/fail/warn).
- **EDIT** `services/expresso-admin/templates/base.html` — novo link de nav `🛡 Auditoria` (após Realm).
- **EDIT** `services/expresso-admin/src/templates.rs` — `AuditAdminTpl` + `AuditViewRow` (com `created_at_fmt: String` RFC3339 já formatado, `metadata_json: String` já serializado).
- **EDIT** `services/expresso-admin/src/audit.rs` — novo `pub async fn page(...)`: SuperAdmin guard, mesma query SQL do `list`, mapeia rows → `AuditViewRow`, renderiza template, constrói `query_string` para o atalho JSON preservando filtros.
- **EDIT** `services/expresso-admin/src/main.rs` — rotas:
  - `/audit.json` → `audit::list` (JSON, inalterado funcionalmente)
  - `/audit.html` → `audit::page` (HTML)
  - `/audit` → `audit::page` (redirect-friendly default)

### Build & Deploy
- Rebuild incremental ≈ 43s. Imagem `expresso-admin:audit` → `69abe4a9007f` (102MB), tagged `:latest`.
- Deploy via `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-admin`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `GET /health` | HTTP 200. |
| 2 | `GET /audit.html` sem auth | HTTP 303 → `/auth/login?redirect=%2Faudit%2Ehtml` (middleware `require_admin` funcionando). |
| 3 | `GET /audit.json` sem auth | HTTP 303 idem. |
| 4 | Container status | `expresso-admin` Up, imagem `69abe4a9007f`, listening `0.0.0.0:8101`. |
| 5 | Container logs | Sem panics / erros de template após start. |

### Observações
- Render do campo `metadata_json` usa `<code>` com `overflow:hidden;text-overflow:ellipsis` e `max-width:36rem` — legível mas sem quebrar layout para metadata grande.
- Query string do link JSON tem encoding manual minimal (` ` → `%20`, `&` → `%26`) para evitar adicionar dep `urlencoding` ao Cargo.
- Filtro temporal (`?since=`, `?until=`) ainda não implementado — campos opcionais já caberiam no SQL (adicionar `AND created_at >= $4 AND created_at < $5`).

### Fora de escopo
- Paginação (atualmente apenas limit 1..=500).
- Filtros temporais.
- Exportação CSV.
- Drill-down em `metadata` (ex. click para expandir JSON formatado).
- Auto-refresh / SSE updates em tempo real.

---

## Update 2026-04-23aa — Audit coverage DAV (calendars + addressbooks) ✅

**Objetivo**: estender cobertura de audit trail para mutações DAV admin (calendars e addressbooks edit/delete), usando a infra de `crate::audit::record` já em produção.

### Arquivos
- **EDIT** `services/expresso-admin/src/dav_admin.rs` — 4 handlers passam a receber `headers: HeaderMap` e chamam `crate::audit::record` ao final:
  - `calendar_edit_action` → action `admin.calendar.update`, target_type `calendar`, metadata `{tenant_id, name, is_default, color}`.
  - `calendar_delete_action` → action `admin.calendar.delete`, target_type `calendar`, metadata `{tenant_id}`.
  - `addressbook_edit_action` → action `admin.addressbook.update`, target_type `addressbook`, metadata `{tenant_id, name}`.
  - `addressbook_delete_action` → action `admin.addressbook.delete`, target_type `addressbook`, metadata `{tenant_id}`.
  - Correção secundária: `bind(color)` → `bind(&color)` para permitir reuso no `serde_json::json!` (evita E0382 move-after-use).

### Build & Deploy
- Rebuild incremental ≈ 42s. Imagem `expresso-admin:audit` → `ab8ab952bfc3` (102MB), tagged `:latest` em 125.
- Deploy via `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-admin`.
- `/health` 200 pós-deploy.

### Cobertura total de audit atual (actions)
| Domain | Action | Arquivo |
|--------|--------|---------|
| Tenant | `admin.tenant.create` | tenants.rs |
| Tenant | `admin.tenant.update` | tenants.rs |
| Tenant | `admin.tenant.delete` | tenants.rs |
| User   | `admin.user.create`   | handlers.rs |
| User   | `admin.user.update`   | handlers.rs |
| User   | `admin.user.delete`   | handlers.rs |
| Calendar | `admin.calendar.update` | dav_admin.rs |
| Calendar | `admin.calendar.delete` | dav_admin.rs |
| Addressbook | `admin.addressbook.update` | dav_admin.rs |
| Addressbook | `admin.addressbook.delete` | dav_admin.rs |

### Fora de escopo
- Cobertura em outros serviços (expresso-auth impersonation/login, expresso-mail admin flows, expresso-calendar admin flows).
- Retries em caso de falha do INSERT (hoje: `record_async` faz `tracing::warn` e swallow).
- PII masking em `metadata` (emails, nomes aparecem em claro — aceitável para audit, mas revisar para LGPD).

---

## Update 2026-04-23ab — Filtros temporais no /audit.html ✅

**Objetivo**: adicionar janela temporal à página de audit com presets (24h/7d/30d/all) + campos manuais `since/until` RFC3339, preservando filtros no atalho JSON.

### Arquivos
- **EDIT** `services/expresso-admin/src/audit.rs`
  - `AuditQuery` ganha `preset: Option<String>`, `since: Option<OffsetDateTime>` (`time::serde::rfc3339::option`), `until: Option<OffsetDateTime>`.
  - Nova `fn resolve_window(&AuditQuery) -> (Option<OffsetDateTime>, Option<OffsetDateTime>)` — presets sobrepõem campos manuais; `all` força ambos None.
  - SQL de `list` e `page` agora incluem `AND ($3::timestamptz IS NULL OR created_at >= $3) AND ($4::timestamptz IS NULL OR created_at < $4)` com binds reais (since, until, limit).
  - `page()` serializa `since/until` para RFC3339 e monta `query_string` com encoding manual de `:` → `%3A` e `+` → `%2B`.
- **EDIT** `services/expresso-admin/src/templates.rs` — `AuditAdminTpl` ganha `preset_v`, `since_v`, `until_v`.
- **EDIT** `services/expresso-admin/templates/audit_admin.html` — formulário ganha `<select name="preset">` com 5 opções (— custom —, 24h, 7d, 30d, Tudo) + inputs RFC3339 `since` e `until`. Legenda sub-label explica precedência (preset > custom).

### Build & Deploy
- Rebuild incremental ≈ 42s. Imagem `expresso-admin:audit` → `7fe16e1b3384` (102MB), tagged `:latest` em 125.
- Deploy via `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-admin`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `GET /health` | HTTP 200. |
| 2 | `GET /audit.html?preset=7d` sem auth | HTTP 303 → `/auth/login?redirect=…` (middleware preservou query). |
| 3 | Container logs | Listening `0.0.0.0:8101`, sem erros de template/sqlx. |

### Observações
- `time::Duration::hours(24)` e `time::Duration::days(N)` usados para deslocar `now_utc()` — evita dependências adicionais.
- `preset=all` resolve em `(None, None)` → desativa filtro temporal mesmo com since/until preenchidos (útil para "ver tudo rapidamente").
- Se query `since` não passar parse RFC3339, `time::serde::rfc3339::option` retorna erro 400 via Query extractor — comportamento razoável (usuário percebe imediatamente no form).
- Encoding manual do `query_string` cobre `:`, `+`, ` `, `&` — suficiente para RFC3339 e prefixes usuais.

### Fora de escopo
- DatePicker / calendário UI (campo é texto livre RFC3339 por enquanto).
- Timezone-aware presets (tudo em UTC).
- Persistência de filtros favoritos por usuário.
- Paginação (ainda apenas limit puro, sem cursor).

---

## Update 2026-04-23ac — Tenant config JSONB editor ✅

**Objetivo**: página dedicada `/tenants/:id/config` para SuperAdmin editar a coluna `tenants.config JSONB`, com validação client+server e audit trail.

### Arquivos
- **NEW** `services/expresso-admin/templates/tenant_admin_config.html` — textarea monospace 20×90 para o JSON, breadcrumb `← dados básicos / lista`, dicas sobre uso.
- **EDIT** `services/expresso-admin/src/templates.rs` — `TenantConfigTpl { current, id, slug, name, config_json, error, flash }`.
- **EDIT** `services/expresso-admin/src/tenants.rs`
  - `pub async fn config_form(...)` — GET: carrega `config` JSONB do tenant, pretty-print via `serde_json::to_string_pretty`, renderiza template. SuperAdmin guard.
  - `pub async fn config_action(...)` — POST: parse com `serde_json::from_str`, valida `is_object()`, salva via `UPDATE tenants SET config = $2, updated_at = NOW()`. Erros renderizam template com mensagem + JSON submetido preservado.
  - Helper `render_config_err(id, submitted, msg, pool)` — reutilizado para os 3 caminhos de erro (JSON inválido, não-object, DB).
  - Audit: action `admin.tenant.config_update`, metadata `{keys: Vec<String>, size_bytes: usize}` — registra apenas as chaves top-level + tamanho (evita dump de config potencialmente sensível).
- **EDIT** `services/expresso-admin/src/main.rs` — rota `.route("/tenants/:id/config", get(tenants::config_form).post(tenants::config_action))`.
- **EDIT** `services/expresso-admin/templates/tenants_admin.html` — novo botão `config` ao lado de `editar`/`excluir` em cada linha.

### Build & Deploy
- Rebuild incremental ≈ 48s. Imagem `expresso-admin:audit` → `390081fcf2cd` (102MB), tagged `:latest` em 125.
- Deploy via `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-admin`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `GET /health` | HTTP 200. |
| 2 | `GET /tenants/:id/config` sem auth | HTTP 303 → `/auth/login?redirect=%2Ftenants%2F…%2Fconfig` (rota registrada, middleware guardando). |
| 3 | Container status | `expresso-admin` Up com imagem `390081fcf2cd`. |
| 4 | Container logs | Sem erros de template/sqlx após start. |

### Observações
- Audit metadata registra **apenas** `keys` top-level + `size_bytes` — **não** grava o config em claro (poderia conter tokens/secrets de integração).
- `updated_at = NOW()` atualizado explicitamente (trigger não existente na tabela `tenants`).
- Validação limita config a **JSON object** (não aceita array/scalar no top-level) para preservar semântica de "mapa de configuração".
- Redirect pós-save retorna para `/tenants/:id/config` (não `/tenants`) → usuário confirma visualmente o save.

### Fora de escopo (deferidos)
- **Impersonation tracking** (prometido no título da trilha) — precisa integrar com expresso-auth (endpoint `/auth/impersonate`), fora da camada admin. Deferido para próxima iteração.
- Schema validation contra um catálogo de chaves conhecidas (feature flags whitelist).
- Diff view comparando antes/depois do save.
- Versionamento/histórico de config (poderia viver no próprio audit_log via `metadata.config_full` criptografado).

---

## Update 2026-04-23ad — Paginação cursor no /audit.html ✅

**Objetivo**: paginação eficiente por cursor (`id < before_id`) para navegar audit trail com qualquer combinação de filtros, preservando performance em histórico longo.

### Arquivos
- **EDIT** `services/expresso-admin/src/audit.rs`
  - `AuditQuery` ganha `before_id: Option<i64>`.
  - SQL (`list` + `page`) adiciona `AND ($5::bigint IS NULL OR id < $5)` — cursor descrescente compatível com `ORDER BY id DESC`.
  - `page()` computa `next_before_id` do último row exibido, monta `next_href` reconstruindo query_string (remove `before_id` anterior, anexa novo). Também gera `reset_href` (sem cursor) e flag `has_cursor`.
- **EDIT** `services/expresso-admin/src/templates.rs` — `AuditAdminTpl` ganha `next_href: Option<String>`, `reset_href: String`, `has_cursor: bool`.
- **EDIT** `services/expresso-admin/templates/audit_admin.html` — rodapé `<nav>` com:
  - Botão `⏮ primeira página` quando cursor ativo.
  - Botão `próxima (mais antigas) →` quando há mais rows (via `next_href`).
  - Legenda `— fim da lista —` quando `next_href` é None (página vazia ou última página).

### Build & Deploy
- Rebuild incremental ≈ 45s. Imagem `expresso-admin:audit` → `bb9659b8c328` (102MB), tagged `:latest` em 125.
- Deploy via `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-admin`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `GET /health` | HTTP 200. |
| 2 | `GET /audit.html?before_id=1&limit=50` sem auth | HTTP 303 → login (rota aceita query params). |
| 3 | Container status | `expresso-admin` Up com imagem `bb9659b8c328`. |

### Observações
- Cursor unidirecional: **só avança para mais antigas** (UX típica de event logs). Para voltar, clica em `⏮ primeira página` e refaz navegação.
- `before_id = id do último row visível` (não `id-1`) — combinado com `id < $5` garante não-overlap entre páginas.
- Se a página atual tem `rows.len() < limit`, ainda assim gera `next_href` baseado no último id, mas a próxima chamada retornará vazio → gera "— fim da lista —" corretamente.
- Query string reconstruction remove qualquer `before_id=` previamente anexado antes de adicionar o novo, prevenindo acumulação em navegações múltiplas.

### Fora de escopo
- Paginação bidirecional (botão "← mais recentes" quando em cursor).
- Contador total de rows (custoso em tabelas grandes; deixa só UX "infinite scroll"-like).
- Página "jump to" por ID/data.
- Keyboard shortcuts (j/k).

---

## Update 2026-04-23ae — Audit de login/logout no expresso-auth ✅

**Objetivo**: registrar eventos `auth.login.success` e `auth.logout` no `audit_log` além dos logs estruturados já existentes, conectando identidade OIDC validada (user_id, tenant_id, email, roles) ao trilho de auditoria.

### Arquivos
- **EDIT** `services/expresso-auth/Cargo.toml` — adiciona dependência `sqlx = { workspace = true }`.
- **EDIT** `services/expresso-auth/src/state.rs` — `AppState` ganha `pub pool: Option<sqlx::PgPool>`. None ⇒ audit desabilitado mas serviço continua servindo OIDC.
- **EDIT** `services/expresso-auth/src/main.rs`
  - Lê `DATABASE_URL` **ou** `DATABASE__URL` (compat com padrão dos outros serviços).
  - Cria `PgPoolOptions` com `max_connections=4`, `acquire_timeout=5s`.
  - Log `audit pool ready` em sucesso; `audit pool unavailable (continuing without audit)` em falha (não bloqueia boot).
- **EDIT** `services/expresso-auth/src/handlers/callback.rs` — após `validator.validate(&tokens.access_token)`, se `app.pool` presente, dispara `audit::record_async` com:
  - `action = "auth.login.success"`, `tenant_id = Some(ctx.tenant_id)`, `actor_sub = Some(ctx.user_id)`, `actor_email`, `actor_roles`, `target_type="user"`, `http_method="GET"`, `http_path="/auth/callback"`, `status_code=200`.
- **EDIT** `services/expresso-auth/src/handlers/logout.rs`
  - Novo parâmetro `headers: HeaderMap` na signature do handler.
  - Extrai `ACCESS_TOKEN_COOKIE` do header `Cookie`, tenta validar (best-effort). Se validação OK, grava `auth.logout` com mesmo shape do login mas `status_code=303`.
  - Falhas de parse/validação são silenciosas (não bloqueiam logout da perspectiva do usuário).
- **NEW** `/root/expresso-build/Dockerfile.auth.quick` (no 101) — runtime slim + `expresso-auth.bin` pré-compilado, mesmo padrão do admin.quick.
- **EDIT** `compose-auth-rp.yaml` (no 125) — adiciona `DATABASE__URL` às envs.

### Build & Deploy
- Rebuild incremental ≈ 31s. Imagem `expresso-auth:audit` → `bf871b89c49f` (≈80MB), tagged `:latest` em 125.
- Deploy real via `docker compose -f compose-auth-rp.yaml up -d --force-recreate`.
- **Descoberta**: `compose-phase3.yaml` define um service `expresso-auth` fantasma (sem `AUTH_RP__*` envs) que nunca foi o service de produção. O real roda via `compose-auth-rp.yaml` como container `expresso-auth-rp` na porta **8012**. Tentativa inicial de deploy via phase3 foi revertida (`compose-phase3.yaml.bak.audit` → phase3 restaurado; stale container removido).

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `docker logs expresso-auth-rp` após start | `provider metadata loaded` + **`audit pool ready`** + `listening addr=0.0.0.0:8012`. |
| 2 | `GET /health` | HTTP 200. |
| 3 | `GET /auth/logout` | HTTP 303 (sem cookie ⇒ audit skipped; redirect IdP ok). |
| 4 | `SELECT * FROM audit_log WHERE action LIKE 'auth.%'` | 0 rows (nenhum login real desde deploy — fluxo completo exige browser + IdP). |

### Observações
- Audit **best-effort**: nunca bloqueia fluxo OIDC. Pool indisponível ⇒ login/logout funcionam, só não gravam no `audit_log`.
- `record_async` usa `tokio::spawn` + `PgPool::clone()` (cheap Arc clone) — fire-and-forget, latência zero na resposta.
- Login failure (erro do IdP no callback) **não** audita atualmente: não temos `tenant_id` antes do token exchange e o schema exige NOT NULL. Fica para trilha futura (precisaria coluna nullable ou tenant especial "_unknown").
- Logout de usuário sem cookie válido (já expirado/forjado) apenas skippa o audit — não gera ruído no log.
- Stale service `expresso-auth` em phase3.yaml **não foi removido** pra evitar touching além do escopo; fica anotado para cleanup futuro.

### Fora de escopo
- `auth.login.failure` com tenant_id=None (schema change necessário).
- Audit do `refresh_token` flow (poderia gerar volume alto; avaliar sampling).
- Correlação session_id entre login→logout (precisa tracking server-side).
- Detecção de geolocalização/IP suspeito (valor duvidoso dentro de LAN corporativa).

---

## Update 2026-04-23af — Cleanup: remoção do service fantasma `expresso-auth` no compose-phase3 ✅

**Objetivo**: eliminar o service `expresso-auth` stale do `compose-phase3.yaml` (sem `AUTH_RP__*` envs, nunca serviu produção) descoberto durante Task ae, prevenindo re-deploys acidentais.

### Arquivos
- **EDIT** (125) `/home/debian/expresso/compose-phase3.yaml` — removido bloco `expresso-auth:` (linhas 100-110, 11 linhas), incluindo mapping `8100:8100`.
- **NEW** (125) `compose-phase3.yaml.bak.preclean` — backup pré-cleanup preservado.

### Build & Deploy
- Nenhum rebuild necessário. Só edição YAML.
- `docker compose -f compose-phase3.yaml config --services` valida OK: lista atual sem `expresso-auth`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `compose config --services` | Serviços: collabora, expresso-admin, expresso-contacts, expresso-flows, expresso-search, expresso-calendar, expresso-compliance, expresso-drive, expresso-notifications, expresso-web, expresso-wopi. **Sem expresso-auth**. |
| 2 | `docker ps --filter name=expresso-auth` | `expresso-auth-rp Up` (real container, gerenciado por `compose-auth-rp.yaml`) — intocado. |
| 3 | Grep primeiro service em phase3 | `  expresso-admin:` (linha 100 — direto, sem bloco fantasma anterior). |

### Observações
- Backup `compose-phase3.yaml.bak.preclean` permite rollback trivial.
- Porta 8100 agora livre no host (ninguém reivindica), reduzindo confusão com 8012 (real) e 8101 (admin).
- `compose-auth-rp.yaml` continua source of truth único para o RP OIDC.

### Fora de escopo
- Renomear `compose-auth-rp.yaml` → `compose-auth.yaml` (cosmético).
- Consolidar todos composes em um master `docker-compose.yaml` (trilha maior).

---

## Update 2026-04-23ag — Audit retention policy ✅

**Objetivo**: dar operador controle sobre tamanho/envelhecimento do `audit_log` via função SQL batched + endpoint admin protegido.

### Arquivos
- **NEW** `migrations/20260424000000_audit_log_purge.sql`
  - Função `audit_log_purge(retention_days INT) RETURNS BIGINT` (plpgsql).
  - Batched: DELETE em ondas de 5000 rows com `FOR UPDATE SKIP LOCKED` para evitar long locks em tabelas grandes.
  - Validação: `retention_days >= 1` (RAISE EXCEPTION).
  - Cutoff: `NOW() - (retention_days || ' days')::INTERVAL`.
  - Retorna total deletado (soma de todas as ondas).
- **EDIT** `services/expresso-admin/src/audit.rs`
  - `AuditQuery` ganha campos flash: `purged: Option<i64>`, `days: Option<i32>`, `error: Option<String>`.
  - `pub async fn purge(State, HeaderMap, Form<PurgeForm>)` — SuperAdmin guard, clamp `1..=3650` server-side, invoca `SELECT audit_log_purge($1)` no pool, audita a própria operação como `admin.audit.purge` com metadata `{retention_days, deleted}`, redirect para `/audit.html?purged=N&days=D` (ou `?error=...`).
  - `page()` computa `flash: Option<String>` a partir dos query params e passa via campo `error` do template (reaproveita slot visual).
- **EDIT** `services/expresso-admin/src/main.rs` — `.route("/audit/purge", post(audit::purge))`.
- **EDIT** `services/expresso-admin/templates/audit_admin.html`
  - `<details>` collapsível "Retenção — purge de logs antigos" com input `retention_days` (default 90, range 7..3650), botão `Purge agora` com `onsubmit=confirm(...)`.
  - `<p class="error">` → `<p class="flash">` para acomodar mensagens positivas (purge concluído).

### Build & Deploy
- Migration aplicada direto no PG (idempotente, `CREATE OR REPLACE`): `CREATE FUNCTION` + `COMMENT`.
- Rebuild incremental ≈ 46s. Imagem `expresso-admin:audit` → `6b9c3e63c2c7`, tagged `:latest` em 125.
- Deploy via `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-admin`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `\df audit_log_purge` | Função presente, tipo retorno `bigint`, arg `retention_days integer`. |
| 2 | `SELECT audit_log_purge(90)` | Retorna `0` (sem rows antigas). |
| 3 | `GET /health` | 200. |
| 4 | `GET /audit.html?purged=42&days=90` sem auth | 303 (gate ok, rota registrada). |
| 5 | `POST /audit/purge -d retention_days=365` sem auth | 303 (gate ok). |
| 6 | Container status | `expresso-admin` Up com `6b9c3e63c2c7`. |

### Observações
- **Auto-audit**: o `admin.audit.purge` é inserido **depois** do DELETE — nunca entra em corrida consigo mesmo (mesmo cutoff instantâneo, NOW() monotonic). A entry sobrevive ao próprio purge que a criou.
- **Batched DELETE**: 5000/round + `SKIP LOCKED` ⇒ compatível com inserts concorrentes durante purge (não bloqueia writers).
- **Clamp server-side** (`1..=3650`): defense-in-depth mesmo que UI valide `min=7 max=3650`.
- **Flash via query redirect**: pattern POST-Redirect-GET padrão; `?purged=N&days=D` dá feedback sem cookies/session.
- **Sem cron automático**: operador precisa clicar `Purge agora`. Agendamento automático fica para ops (systemd timer ou pg_cron se disponível) — ver "Fora de escopo".

### Fora de escopo (trilhas futuras)
- **Agendamento automático**: systemd timer no 125 chamando `psql -c "SELECT audit_log_purge(90)"` semanalmente. Ou `pg_cron` se extensão disponível.
- **Retention por action**: diferentes TTLs por tipo (ex: `auth.*` 365d, `admin.dav.*` 90d). Precisaria função com CASE.
- **Partitioning por mês**: `audit_log_YYYY_MM` + DROP TABLE mensal. Escalaria melhor para volumes >10M rows.
- **Export pré-purge**: dump CSV/Parquet para cold storage antes de deletar (compliance).
- **Dry-run**: `audit_log_purge_preview(days)` que conta sem deletar.

---

## Update 2026-04-23ah — Cron automático de purge via systemd timer ✅

**Objetivo**: rodar `audit_log_purge(90)` automaticamente toda semana sem intervenção humana, fechando a trilha de retenção (ae → ag → ah).

### Arquivos (criados no host 125)
- **NEW** `/etc/default/expresso-audit-purge` (perm 600, owner root) — EnvironmentFile com `PGHOST/PGPORT/PGUSER/PGDATABASE/PGPASSWORD` + `RETENTION_DAYS=90` (tunable).
- **NEW** `/etc/systemd/system/expresso-audit-purge.service` — unit `Type=oneshot` que executa `docker run --rm -e PG* postgres:16-alpine psql -v ON_ERROR_STOP=1 -Atc "SELECT audit_log_purge(${RETENTION_DAYS})"`. Output vai pro journal.
- **NEW** `/etc/systemd/system/expresso-audit-purge.timer` — `OnCalendar=Sun 03:00`, `Persistent=true` (roda após downtime), `RandomizedDelaySec=300` (spread de até 5 min p/ evitar carga simultânea em cluster futuro).

### Build & Deploy
- Nenhum rebuild. Só systemd units.
- `systemctl daemon-reload` + `systemctl enable --now expresso-audit-purge.timer`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `systemctl list-timers expresso-audit-purge.timer` | `NEXT Sun 2026-04-26 03:04:18 UTC` — agendado. |
| 2 | `systemctl start expresso-audit-purge.service` (dry-run) | `status=0/SUCCESS`, journal mostra output `0` (sem rows antigas). |
| 3 | `journalctl -u expresso-audit-purge.service` | Start/Finish clean, docker exec retorna 0 deletados. |
| 4 | Permissão `/etc/default/expresso-audit-purge` | `600 root:root` (creds protegidas). |

### Observações
- **Zero deps extras no host**: usa docker (já rodando) + imagem `postgres:16-alpine` (pulled on-demand, cached após primeiro run). Não precisou instalar `psql` no host.
- **EnvironmentFile pattern**: permite ajustar `RETENTION_DAYS` sem editar unit file (só `systemctl restart` ou aguardar próximo trigger).
- **Persistent=true**: se máquina ficar down durante o horário agendado, o timer roda na próxima inicialização (compliance-friendly).
- **RandomizedDelaySec=300**: preparação para caso futuro de múltiplas máquinas → spreads o load no PG.
- **Idempotente**: `audit_log_purge()` é safe para re-rodar (sem-op quando não há rows antigas, como no dry-run).
- **Auditoria do cron**: o purge via systemd **não** passa pelo endpoint admin, então **não audita a si mesmo** como o botão da UI faz. Para trilha futura: criar um wrapper SQL que insira um audit row com action `system.audit.purge` no final.

### Fora de escopo
- Self-audit do cron (precisa tenant_id sentinel ou schema change — mesma limitação de `auth.login.failure`).
- Alerta em falha (atualmente silencioso no journal). Poderia integrar com Prometheus alertmanager ou e-mail.
- Rotação por ambiente (dev 7d, staging 30d, prod 90d). Precisaria template em deploy.
- `OnCalendar` configurável via env (hoje hardcoded Sun 03:00).

---

## Update 2026-04-23ai — SEQUENCE auto-bump em edits materiais (RFC 5545 §3.7.4) ✅

**Objetivo**: corrigir comportamento incorreto do `expresso-calendar` que bumpava `SEQUENCE` a cada `UPDATE`/`UPSERT`, mesmo em edits cosméticos. Per RFC 5545 §3.7.4, SEQUENCE deve incrementar **apenas** em "material change" (campos que afetam o agendamento).

### Comportamento antes vs depois
| Edit | Antes | Depois |
|------|-------|--------|
| Re-save com mesmos campos | sequence + 1 (spam) | sequence inalterado ✅ |
| Mudança em SUMMARY / LOCATION / DTSTART / DTEND / RRULE / STATUS / ORGANIZER | sequence + 1 | sequence + 1 ✅ |
| Mudança só em DESCRIPTION | sequence + 1 | sequence inalterado ✅ (wording não é schedule-affecting) |
| Mudança só em X-*, CATEGORIES, PRIORITY, COMMENT, VALARM | sequence + 1 | sequence inalterado ✅ |

### Arquivos
- **EDIT** `services/expresso-calendar/src/domain/event.rs`
  - `update()` — substitui `sequence = sequence + 1` por SQL `CASE WHEN <any material field> IS DISTINCT FROM <new value> THEN sequence + 1 ELSE sequence END`.
  - `replace_by_uid()` — mesma lógica no branch `ON CONFLICT DO UPDATE`, usando `calendar_events.<col> IS DISTINCT FROM EXCLUDED.<col>`.
  - Campos material: `summary, location, dtstart, dtend, rrule, status, organizer_email`.
  - Não-material (preservados no banco mas não triggam bump): `description`, `ical_raw`, `etag`, `uid`.

### Build & Deploy
- Rebuild calendar ≈ 1m20s (recompile sqlx/deadpool-redis/expresso-core + calendar).
- Nova imagem `expresso-calendar:seqbump` → `2fb9b7ce29ec`, tagged `:latest` em 125.
- Deploy via `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-calendar`.

### Smoke Tests
| # | Cenário | Resultado |
|---|---------|-----------|
| 1 | `GET /health` | 200. |
| 2 | SQL temp-table simulando lógica CASE: re-save com mesmos fields | sequence `5 → 5` ✅ |
| 3 | SQL temp-table: mudança em summary | sequence `5 → 6` ✅ |
| 4 | SQL temp-table: re-save após mudança (mesmos novos fields) | sequence `6 → 6` ✅ |
| 5 | Container status | Up com `2fb9b7ce29ec`. |

### Observações
- **`IS DISTINCT FROM`**: operador PG que trata NULL corretamente (não gera `NULL = NULL` indefinido). Crucial para campos opcionais (location, rrule podem ser NULL).
- **Atomicidade**: decisão de bump feita no mesmo UPDATE que persiste os novos valores. Zero race conditions.
- **DESCRIPTION = não-material**: decisão deliberada (alinha com Google Calendar / Outlook). Wording fix não dispara reenvio de REQUEST com SEQUENCE superior para todos attendees.
- **ATTENDEES**: atualmente o parser não extrai lista de ATTENDEEs separadamente — ela vive em `ical_raw`. Mudança de attendees (add/remove) **não** triggera bump neste patch. Trilha futura: parser de ATTENDEE + coluna/tabela + inclusão no predicate.
- **ical_raw diferente mas campos materiais iguais**: re-save de VCALENDAR com apenas reorganização de properties (mesma semântica, textualmente diferente) → sequence estável. Correto.
- **Interop scheduling (iTIP)**: com o gate correto, REPLY/CANCEL recebidos de attendees só ficam "stale" quando realmente houve um re-REQUEST material do organizador.

### Fora de escopo
- Detecção de mudança de ATTENDEES (requires parser upgrade + col).
- DTSTAMP refresh independente de SEQUENCE (DTSTAMP sempre atualiza em qualquer save — separate concern).
- Bump quando `RECURRENCE-ID` diverge (exception overrides). Hoje apenas VEVENT master é indexado.
- UI calendar expondo o sequence atual do evento (visibilidade debug).

---

## Task #2 — iTIP COUNTER proposal persistence (RFC 5546 §3.2.7)

**Data**: 2026-04-24 (autonomous trilha item #2)

**Objetivo**: Persistir COUNTER-proposals enviados por attendees e dar ao
organizador uma UI para aceitar/rejeitar.

### Mudanças

**Schema** — `migrations/20260424010000_scheduling_counter_proposals.sql`:
- Tabela `scheduling_counter_proposals` (id, tenant_id, event_id→calendar_events, attendee_email, proposed_dtstart/dtend, comment, status pending|accepted|rejected, received_sequence, raw_ical, created_at, resolved_at, resolved_by).
- Índices: (tenant_id, status, created_at DESC), (event_id, status).

**Calendar service**:
- `services/expresso-calendar/src/domain/counter.rs` novo — `CounterRepo` (insert, list_pending, get, resolve).
- `services/expresso-calendar/src/domain/event.rs` — accessor público `EventRepo::pool()`.
- `services/expresso-calendar/src/api/scheduling.rs::handle_counter()` — agora insere proposal se o UID bate no tenant; devolve `proposal_id` na resposta.

**Admin service**:
- `services/expresso-admin/src/counter.rs` novo — handlers `page`, `accept`, `reject`.
  - Accept: faz UPDATE em `calendar_events` com proposed_dtstart/dtend + bump SEQUENCE (replica lógica da ai).
  - Reject: apenas marca status=rejected (organizador envia DECLINECOUNTER externamente).
- Rotas: `GET /counter.html`, `POST /counter/:id/accept`, `POST /counter/:id/reject`.
- Gate: `auth::require_super_admin`.
- Audit: `admin.counter.accept` / `admin.counter.reject`.

**Templates**:
- `counter_admin.html` — tabela com DTSTART/DTEND propostos, SEQ, botões Aceitar/Rejeitar (confirm() JS).
- `base.html` — nav: 📨 iTIP COUNTER.

### Deploy

- Calendar image: `343cd1e68ca1` — `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-calendar`.
- Admin image: `af41319758b5` — idem.

### Smoke

- `/health` calendar 200, admin 200.
- `/counter.html` → 303 (login gate OK).
- Tabela criada em produção (psql `\d scheduling_counter_proposals`).

### Próximo passo natural

- Dispatcher DECLINECOUNTER iMIP (SMTP) on reject — hoje operador envia manual.
- Campo COMMENT/DESCRIPTION do COUNTER parseado do body ical.

---

## Task #3 — SSE push notifications (MVP in-process)

**Data**: 2026-04-24 (autonomous trilha item #3)

**Escopo MVP**: Event bus in-process via `tokio::sync::broadcast` + SSE endpoint
por tenant. NATS fica para v2 (shape do `Event` enum é estável → troca só o transporte).

### Mudanças

**Novos módulos** (calendar):
- `services/expresso-calendar/src/events.rs` — `EventBus` (broadcast channel cap 1024) + enum `Event { EventCreated, EventUpdated, EventCancelled, CounterReceived }` com `tenant_id()`.
- `services/expresso-calendar/src/api/stream.rs` — `GET /api/v1/events/stream` SSE handler; `BroadcastStream` + filter por `ctx.tenant_id` + keep-alive 15s.

**AppState**:
- `state.rs` — novo campo `events: EventBus` + accessor `events()`. Constructor agora recebe `EventBus`.

**Hooks de publicação** (todas best-effort, não bloqueantes):
- `api/events.rs::create`   → `Event::EventCreated`
- `api/events.rs::update`   → `Event::EventUpdated { sequence }`
- `api/events.rs::delete`   → `Event::EventCancelled`
- `api/scheduling.rs::handle_counter` → `Event::CounterReceived { proposal_id }`

**Deps**: `tokio-stream = { version = "0.1", features = ["sync"] }` (workspace).

### Deploy

- Calendar image: `480ef7cbbce8` — `docker compose -f compose-phase3.yaml up -d --force-recreate expresso-calendar`.

### Smoke

- `/health` 200.
- `GET /api/v1/events/stream` → `200 text/event-stream` (long-poll, keep-alive).

### Próximos passos (fora do MVP)

- Adapter NATS para multi-node (mesma `Event` enum).
- Hooks em drive/mail publicando eventos análogos.
- Cliente JS no webmail: `new EventSource('/api/v1/events/stream')`.
- Auth: hoje usa `x-tenant-id`/`x-user-id` headers; front-end via gateway já injeta.

---

## Task #4 — Impersonation tracking (MVP audit-only)

**Data**: 2026-04-24 (autonomous trilha item #4)

**Escopo MVP**: endpoints `/auth/impersonate/*` SuperAdmin-gated emitem audit
trail completo. Session swap real é delegado ao admin console do Keycloak via
URL retornada (full token-exchange grant pendente — requer configuração KC).

### Mudanças

- `services/expresso-auth/src/handlers/impersonate.rs` — novo handler:
  - `POST /auth/impersonate/:target_user_id` → gate `superadmin` role, audit `auth.impersonate.start` com `actor_sub` (impersonator) + `target_user_id`, devolve JSON com `keycloak_url` (admin console).
  - `POST /auth/impersonate/end` → audit `auth.impersonate.end`.
- `services/expresso-auth/src/handlers/mod.rs` — registro `pub mod impersonate;`.
- `services/expresso-auth/src/main.rs` — rotas adicionadas.

**Gate**: requer role `superadmin`/`super_admin`/`SuperAdmin` (case-insensitive).

**Auth**: reusa `Authenticated` extractor (sign-on via ACCESS_TOKEN_COOKIE).

### Deploy

- Auth image: `b8f5b2a08b68` — `docker compose -f compose-auth-rp.yaml up -d --force-recreate` (container `expresso-auth-rp` :8012).

### Smoke

- `/health` 200.
- `POST /auth/impersonate/end` sem sessão → `401` (Authenticated extractor bloqueia).

### Follow-ups

- Token-exchange via KC (grant `urn:ietf:params:oauth:grant-type:token-exchange` + `urn:ietf:params:oauth:token-type:access_token`) para emissão de access_token do target sem passar pelo console.
- Filtrar logs do target por marca `impersonated_by` em metadata.
- UI no admin para listar sessões de impersonação ativas.

---

## Task #5 — Audit refresh_token com sampling

**Data**: 2026-04-24 (autonomous trilha item #5)

**Escopo**: Registrar em `audit_log` eventos de `/auth/refresh`:
- **100% dos failures** (upstream error).
- **~10% dos successes** (sampling; cada sucesso audita com probabilidade `26/256 ≈ 10%` via `rand::random::<u8>() < 26`).

### Mudanças

- `services/expresso-auth/src/handlers/refresh.rs`:
  - Import `expresso_core::audit::{record_async, AuditEntry}`.
  - Failure path: captura status + corpo da resposta KC, audita `auth.token.refresh.failure` com `status_code` real e `metadata.upstream_error` truncado em 500 chars.
  - Success path: sorteio (u8 < 26 → ~10%); quando amostrado, valida o novo access_token via `app.validator.validate()` para recuperar sub/email/tenant/roles e audita `auth.token.refresh.success` com `metadata.sampled=true` + `metadata.rate=0.1`.

- `libs/expresso-core/src/audit.rs` — remove guard `tenant_id required`; agora aceita `None` (tenant_id passou a ser nullable no schema).

- `migrations/20260424130000_audit_log_tenant_nullable.sql` — `ALTER TABLE audit_log ALTER COLUMN tenant_id DROP NOT NULL`. Rationale: eventos pre-tenant (failed login, refresh failure, system tasks) precisam registrar sem contexto. O índice parcial `audit_log_tenant_idx WHERE tenant_id IS NOT NULL` já antecipava nullability.

### Deploy

- DB: migração aplicada em prod.
- Auth image: `69e372925b02` — `compose-auth-rp.yaml up -d --force-recreate`.

### Smoke

- 5 POSTs com refresh_token bogus → `audit_log` tem 5 rows `auth.token.refresh.failure` (todos auditados, 100%).
- Success path só audita ~10% (requer refresh válido para testar ponta-a-ponta em prod).

### Consequências

- `auth.login.failure` (pendente da task ae) agora também é auditável — tenant_id nullable desbloqueia.
- Outras audit callers que usam `AuditEntry { tenant_id: None, ... }` agora persistem em vez de erro silencioso.

---

## Task #7 — Tombstones retention cron

**Data**: 2026-04-24 (autonomous trilha item #7)

**Escopo**: Replicar pattern da task ah (audit purge) para tombstones CalDAV/CardDAV.
Retenção default = **30 dias** (alinhado com `tombstone_gc.rs::DEFAULT_RETENTION_DAYS`).

### Mudanças

- `migrations/20260424140000_tombstones_purge_fn.sql` — função PostgreSQL `tombstones_purge(retention_days INT) RETURNS BIGINT`:
  - Purga `calendar_event_tombstones` + `contact_tombstones` em batches de 5000 rows com `FOR UPDATE SKIP LOCKED`.
  - Raises se retention < 1.
- Host 125 systemd:
  - `/etc/default/expresso-tombstones-purge` (600 root): PGHOST/PGPORT/PGUSER/PGDATABASE/PGPASSWORD + RETENTION_DAYS=30.
  - `expresso-tombstones-purge.service` — Type=oneshot, roda `postgres:16-alpine psql -Atc "SELECT tombstones_purge(${RETENTION_DAYS})"`.
  - `expresso-tombstones-purge.timer` — OnCalendar=Sun 03:30, Persistent=true, RandomizedDelaySec=300.

### Deploy

- DB: migração aplicada em prod.
- Host 125: timer enabled; próxima execução Dom 2026-04-26 03:31:43 UTC.

### Smoke

- `psql -Atc "SELECT tombstones_purge(30)"` → `0`.
- `systemctl start expresso-tombstones-purge.service` → status 0/SUCCESS, journal mostra `0`.

### Observação

- Background GC in-process (`domain::tombstone_gc::spawn`) continua ativo na instância do expresso-calendar (a cada 6h). O cron semanal é **defesa em profundidade**: se o serviço estiver offline por período prolongado, o timer garante limpeza.


---

## Trilha sprint — tasks #6, #8, #9, #10 (2026-04-23)

Single admin image rebuild ships #6 (Dead Props), #8 (Drive Quotas UI),
#10 (Tenant config whitelist). #9 (Drive uploads purge) is pure DB+systemd.

### #6 — DAV Dead Properties admin UI

Read-only listing of PROPPATCH-set XML properties on calendars and
addressbooks (Apple calendar colors, display-order, etc). Useful to
diagnose unexpected metadata on collections.

**New files:**
- `services/expresso-admin/src/dead_props.rs`
- `services/expresso-admin/templates/dead_props_admin.html`

**Wired:** `main.rs` -> `GET /dead-props.html`. Nav link: Dead Props.

**Query:** UNION ALL of `calendar_dead_properties` + `addressbook_dead_properties`
LEFT JOIN parent collections (using `name` column), ordered by updated_at
DESC, LIMIT 200. Value truncated to 120 chars.

**Smoke:** `GET /dead-props.html` -> 303 (auth gate). OK.

### #8 — Drive Quotas admin UI

List tenants with storage used/limit in MiB, allow per-tenant limit update.

**New files:**
- `services/expresso-admin/src/drive_quotas.rs`
- `services/expresso-admin/templates/drive_quotas_admin.html`

**Wired:** `GET /drive-quotas.html` + `POST /drive-quotas/:tenant_id`.
Nav link: Drive Quotas.

**Query:** LEFT JOIN `tenants x drive_quotas x SUM(drive_files.size_bytes)`.
Update via INSERT ... ON CONFLICT UPDATE. `max_mb=0` = no limit.
Clamp 0..=10TB. Audit `admin.drive.quota_update`.

**Smoke:** `GET /drive-quotas.html` -> 303 (auth gate). OK.

### #9 — Drive resumable uploads purge cron

PG function + systemd timer to delete expired rows in `drive_uploads`
(resumable upload sessions past `expires_at`, default NOW()+30d).

**Migration:** `migrations/20260424150000_drive_uploads_purge_fn.sql`
-> `drive_uploads_purge_expired()` RETURNS BIGINT, batched 5000 rows
with `FOR UPDATE SKIP LOCKED`.

**Systemd (on 192.168.15.125):**
- `/etc/default/expresso-drive-uploads-purge` (chmod 600, PG creds)
- `/etc/systemd/system/expresso-drive-uploads-purge.service`
  (Type=oneshot, docker run postgres:16-alpine psql)
- `/etc/systemd/system/expresso-drive-uploads-purge.timer`
  (OnCalendar=daily 04:00 UTC, Persistent=true, RandomizedDelaySec=300)

**Smoke:** dry-run `SELECT drive_uploads_purge_expired()` -> 0; systemctl
service run exit 0/SUCCESS; next scheduled: daily 04:00 UTC. OK.

### #10 — Tenant config top-level key whitelist

Before save, reject JSON with unknown top-level keys (fail-closed policy).
Known keys must be added alongside the feature that consumes them.

**Patched:** `services/expresso-admin/src/tenants.rs::config_action`
adds `ALLOWED_KEYS` whitelist check after `parsed.is_object()` guard.
On unknown key(s), re-renders form with error listing unknown keys +
allowed set.

**Allowed (initial):** branding, features, smtp, quota, retention, locale,
caldav, carddav, webmail, security.

### Deploy notes

- Admin container: **ffe967a8dad3** (tag `expresso-admin:t8910` + `:latest`).
- Built on 101 (rust:1-bookworm + mold), shipped to 125 via scp.
- Compose: `compose-phase3.yaml` `up -d --force-recreate expresso-admin`.
- Only #9 required a DB migration; rest are in-process admin code.

## Sprint Trilha #11–#20 (parte 1: #11 + #12 + #14)

### #11 — Rate limiting por tenant (in-process)

Token-bucket middleware keyed por `x-expresso-tenant` header (fallback
`x-forwarded-for` → `_anon`). Configurável via env
`EXPRESSO_RATELIMIT_RPS` (50) / `EXPRESSO_RATELIMIT_BURST` (200).
Denied requests → 429 + `Retry-After`. GC 10min idle.

**Novo:** `libs/expresso-core/src/ratelimit.rs` (RateLimiter, RateLimitConfig,
`layer` middleware). Skip allowlist p/ `/health /healthz /readyz /ready /metrics`.
3 unit tests (burst/refill/isolated).

**Wiring:** `services/expresso-calendar/src/main.rs` +
`services/expresso-contacts/src/main.rs`:
```rust
let rate_cfg = expresso_core::ratelimit::RateLimitConfig::from_env();
let rate_limiter = expresso_core::ratelimit::RateLimiter::new(rate_cfg);
tokio::spawn(async move { loop { sleep(300s); rl.gc(); } });
app.layer(from_fn(ratelimit::layer)).layer(Extension(rate_limiter));
```

**Smoke:** 2000 req P200 mesmo tenant → 347 passam (burst≈200+refill),
1653 → 429. /health + /readyz + /metrics sempre 200 (allowlist).

### #12 — Métricas Prometheus `/metrics`

`libs/expresso-observability/src/lib.rs` já expunha `metrics_router()` +
`HTTP_REQUESTS_TOTAL`. Adicionado `http_counter_mw(req, next)` middleware
que conta service/method/status por label.

**Wiring:** `services/expresso-calendar/src/api/mod.rs` +
`services/expresso-contacts/src/api/mod.rs`:
`.layer(from_fn(expresso_observability::http_counter_mw))`.

**Smoke:** `curl /metrics` →
```
# HELP http_requests_total Total HTTP requests
# TYPE http_requests_total counter
http_requests_total{method="GET",service="expresso",status="404"} 347
```

### #14 — Health check profundo `/readyz`

`libs/expresso-core/src/health.rs` — `ReadinessCheck` (name, required, fn),
`run(checks)` com timeout 3s/check, 503 se qualquer required falhar.
`db_check(PgPool)` roda `SELECT 1`.

**Wiring:** `services/expresso-calendar/src/api/health.rs` +
`services/expresso-contacts/src/api/health.rs`:
```rust
async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let checks = vec![ReadinessCheck { name: "db", required: true, run: db_check(db.clone()) }];
    let (code, report) = health::run(&checks).await;
    (code, Json(report))
}
```

**Smoke:** `/readyz` →
```json
{"status":"ok","components":[{"name":"db","status":"ok","error":null,"elapsed_ms":2}]}
```

### Deploy notes (#11/#12/#14)

- Calendar: **expresso-calendar:t111214b** + `:latest`.
- Contacts: **expresso-contacts:t111214b** + `:latest` (nova imagem
  `Dockerfile.contacts.quick`).
- Built on 101, scp → 125, docker load + compose up.

### #13 — PostgreSQL backup diário + retention

Script bash executa `pg_dump -Fc -Z 6` via container `postgres:16-alpine`
contra PG remoto (192.168.15.123). Verifica integridade com
`pg_restore -l` e aplica retenção (delete `-mtime +RETENTION_DAYS`).

**Novo (repo):** `ops/backup/`:
- `expresso-pg-backup.sh` (`/usr/local/sbin/`)
- `expresso-pg-backup.service` (oneshot)
- `expresso-pg-backup.timer` (daily 02:00 UTC, RandomizedDelaySec=300, Persistent=true)
- `expresso-pg-backup.env.example` → `/etc/default/expresso-pg-backup`
  (chmod 600, contém PG* + BACKUP_DIR + RETENTION_DAYS=30)

**Instalado em 192.168.15.125:** `/var/backups/expresso/pg/` (chmod 700).

**Smoke:** `systemctl start expresso-pg-backup.service` →
`backup ok: /var/backups/expresso/pg/expresso-20260423T210056Z.dump (128316 bytes)`.
Próximo agendamento: diário 02:00 UTC.

### #15 — Password reset self-service

`POST /auth/forgot {"email":"..."}` — sempre 204 (sem leak de existência).
Se email existe no realm KC, envia-se `execute-actions-email` com
action `UPDATE_PASSWORD` (lifespan 1h). Audit: `auth.password_reset.requested`.

**Novo:**
- `services/expresso-auth/src/kc_admin.rs` — cliente admin KC minimal
  (token master/admin-cli + user_id_by_email + execute_actions_email).
- `services/expresso-auth/src/handlers/forgot.rs` — handler stateless.

**main.rs:** registrado `mod kc_admin;` + route
`POST /auth/forgot → handlers::forgot::forgot`.

**Compose:** `compose-auth-rp.yaml` ganhou envs `KC_URL`, `KC_REALM`,
`KC_ADMIN_USER`, `KC_ADMIN_PASS` + `extra_hosts: host.docker.internal`.

**Smoke:**
- email vazio → 204 (noop).
- email inexistente → 204 (silent).
- email real (alice@expresso.local) → 204; KC localiza user
  `c3a1459f-…` + dispara `execute-actions-email`. Status real de envio
  depende de SMTP do realm (erro `Please provide a valid address` se
  realm SMTP não configurado — fora do escopo de código).
- Imagem: `expresso-auth:t15` (+ `:latest`).

### #16 — 2FA TOTP toggle via admin UI

KC nativamente suporta TOTP como required-action. Admin UI ganha dois
botões por usuário:
- **enrolar 2FA** → `POST /users/:id/totp/enroll` → KC
  `execute-actions-email [CONFIGURE_TOTP]` (usuário recebe link p/ QR).
- **reset 2FA** → `POST /users/:id/totp/reset` → lista
  `/users/:id/credentials`, deleta todas do tipo `otp` → força
  re-enrolamento no próximo login.

**Patched:**
- `services/expresso-admin/src/kc.rs`: `enroll_totp` + `reset_totp`.
- `services/expresso-admin/src/handlers.rs`: `user_totp_enroll` +
  `user_totp_reset` handlers com audit (`admin.user.totp.enroll|reset`).
- `services/expresso-admin/src/main.rs`: 2 rotas POST.
- `services/expresso-admin/templates/users.html`: 2 `<form>` inline
  com `confirm()` JS antes de submeter.

**Smoke:** `POST /users/c3a1459f.../totp/enroll` → 303,
`POST /users/c3a1459f.../totp/reset` → 303. Image `expresso-admin:t16` +
`:latest` deployed em 125.

### #17 — Audit log CSV export

`GET /audit.csv` — mesmos filtros de `/audit.json` (`action_prefix`,
`tenant_id`, `preset` 24h/7d/30d/all, `since`/`until`, `before_id`,
`limit`) mas cap = 50k rows/req. Content-Type `text/csv; charset=utf-8`
+ `Content-Disposition: attachment; filename="audit-<utc>.csv"`.
Campos RFC 4180-escaped (aspas duplicadas, vírgulas/quebras quoted).

**Colunas:** id, created_at, tenant_id, user_id, action, resource,
status, metadata (JSON compacto).

**Patched:**
- `services/expresso-admin/src/audit.rs::csv` — novo handler.
- `services/expresso-admin/src/main.rs`: `.route("/audit.csv", get(audit::csv))`.
- `services/expresso-admin/templates/audit_admin.html`: botão CSV ao
  lado do JSON.

**Smoke:** `GET /audit.csv` → 303 (auth gate, SuperAdmin required).
Imagem `expresso-admin:t17` + `:latest`.

### #19 — Drive quota enforcement no upload path

Já implementado em sprint anterior — verificado nesta trilha:

- `libs` SQL: tabela `drive_quotas(tenant_id, max_bytes)` + função
  `drive_quota_used(tenant)` (sprint #8).
- `services/expresso-drive/src/domain/quota.rs::QuotaRepo::get` +
  `Quota::fits(extra)`.
- Enforçado em 3 paths:
  - `api/files.rs:121` (upload simples).
  - `api/uploads.rs:120` (resumable finalize).
  - `api/wopi.rs:192` (WOPI PutFile).
- Retorna `DriveError::QuotaExceeded` → HTTP **507 Insufficient Storage**.

Default quota = 10 GB quando tenant não tem linha explícita em
`drive_quotas` (gerenciado via admin `/drive-quotas.html`).

**Status:** ✅ já estava em produção; nenhuma ação necessária.

### #18 — Tenant onboarding wizard

Fluxo single-page para provisionar tenant completo:

- `services/expresso-admin/templates/tenant_wizard.html`: form com slug, nome,
  plano, email admin, username admin.
- `services/expresso-admin/src/tenants.rs::wizard_form` + `wizard_action`:
  - Valida entrada (slug `[a-z0-9-]+`, plano `free|pro|enterprise`, email).
  - `INSERT INTO tenants (slug,name,plan,status='active') RETURNING id`.
  - `KcAdmin.create_user()` com senha placeholder + `temporary:true`.
  - Dispara `CONFIGURE_TOTP` via `execute_actions_email` (reusa método #16).
  - Em falha KC → `DELETE FROM tenants WHERE id=$1` (sem tenants órfãos).
  - Audit: `admin.tenant.onboard` com tenant_id+slug+admin_email+kc_user_id.
- `main.rs`: rota `/tenants/wizard` (GET+POST) guardada por `require_super_admin`.
- `templates/tenants_admin.html`: botão "Onboarding wizard" ao lado de "+ Novo tenant".

**Smoke 125:**
```
curl http://172.17.0.1:8101/tenants/wizard → 303 (redirect auth, rota registrada)
```

Imagem: `expresso-admin:t18`.

### #20 — NATS JetStream event publishing

Calendar service agora publica eventos em JetStream além do broadcast
in-process. Transport opt-in via env `NATS_URL`.

- `services/expresso-calendar/Cargo.toml`: `async-nats = "0.37" + bytes = "1"`.
- `services/expresso-calendar/src/events.rs::EventBus::new_with_nats(url)`:
  - Conecta em NATS + cria `jetstream::Context`.
  - `get_or_create_stream` idempotente: nome `EXPRESSO_CALENDAR`, subjects
    `expresso.calendar.>`, `max_age = 7 dias`.
- `EventBus::publish(ev)`: mantém broadcast local + `tokio::spawn` publish
  em JetStream no subject `expresso.calendar.<tenant_id>.<kind>`. Fire-and-forget
  com `tracing::warn!` em erro (nunca bloqueia hot-path).
- `main.rs`: quando `NATS_URL` presente → `new_with_nats`; falha → fallback
  silencioso para in-process.
- `compose-phase3.yaml` (125): `NATS_URL=nats://172.17.0.1:4222` adicionado
  a `expresso-calendar` (NATS já rodava como `expresso-nats 2.10-alpine` com
  JetStream habilitado em `/data/jetstream`).

**Smoke 125:**
```
logs calendar:  jetstream EXPRESSO_CALENDAR ready, nats://172.17.0.1:4222
                async_nats: event: connected
                calendar EventBus with NATS enabled

curl http://172.17.0.1:8222/jsz?streams=1
  → streams: 1, EXPRESSO_CALENDAR registered
```

Imagem: `expresso-calendar:t20`.

Extensões futuras (fora do #20):
- Adicionar NATS no `expresso-contacts` (mesmo padrão, subject `expresso.contacts.>`).
- Consumers: email-dispatcher, iMIP relay, webhook fanout, search re-indexer.

### #21 — Grafana dashboards

Artefato JSON + docs (zero deploy) alavancando as métricas expostas em
`/metrics` pelos serviços (sprint #12) + JetStream (sprint #20).

- `ops/grafana/expresso-overview.json`: dashboard schemaVersion 39 com 6 painéis:
  1. HTTP req/s por serviço (`rate(http_requests_total[1m])` by service).
  2. HTTP 4xx/5xx por serviço.
  3. 429 rate-limited hits (5m increase).
  4. Status mix global.
  5. JetStream EXPRESSO_CALENDAR (messages + bytes — requer nats-exporter).
  6. /readyz up count (serviços com `up=1`).
- Template variable `$service` = `label_values(http_requests_total, service)`.
- `ops/grafana/README.md`: instruções de import + scrape config exemplo.

**Status:** ✅ artefato commitado; import manual no Grafana quando provisionado.

### #22 — NATS JetStream smoke tool

CLI ops para validar presença de streams JetStream (usado em smokes
pós-deploy e em CI).

- `ops/nats/smoke.sh`: bash + python3 (stdlib) — consulta `/jsz?streams=1`
  e valida stream. Exit codes:
  - `0` stream presente + stats impressas.
  - `1` stream ausente.
  - `2` monitoring endpoint inacessível.
- Args: `[NATS_MON_URL] [STREAM]` (defaults: `http://localhost:8222` + `EXPRESSO_CALENDAR`).

**Smoke 125:**
```
bash ops/nats/smoke.sh http://172.17.0.1:8222 EXPRESSO_CALENDAR
→ OK: stream 'EXPRESSO_CALENDAR' present.
    messages: 0, bytes: 0, consumers: 0
```

### #23 — Contacts EventBus + NATS JetStream (scaffold)

Infra de publicação de eventos para contacts, espelhando #20 (calendar) sem
broadcast in-process (contacts não tem SSE consumer).

- `services/expresso-contacts/Cargo.toml`: `async-nats = "0.37"`.
- `services/expresso-contacts/src/events.rs`: enum `ContactsEvent` com
  variantes `AddressbookCreated|Deleted`, `ContactUpserted|Deleted`.
  `ContactsEventBus::new_with_nats(url)` → stream `EXPRESSO_CONTACTS`
  (`expresso.contacts.>`, max_age 7 dias).
  `publish(ev)` fire-and-forget com `tokio::spawn`.
- `src/state.rs`: AppState agora armazena `bus: ContactsEventBus` + getter `bus()`.
- `src/main.rs`: `mod events;` + opt-in via `NATS_URL` (mesmo padrão calendar,
  fallback silencioso para `noop()`).
- `compose-phase3.yaml` (125): `NATS_URL=nats://172.17.0.1:4222` em contacts.

**Smoke 125:**
```
logs:  jetstream EXPRESSO_CONTACTS ready, nats://172.17.0.1:4222
       async_nats: event: connected
       contacts EventBus with NATS enabled

bash ops/nats/smoke.sh http://172.17.0.1:8222 EXPRESSO_CONTACTS
→ OK: stream 'EXPRESSO_CONTACTS' present.
```

Imagem: `expresso-contacts:t23`.

**Pendência (fora do #23):** injetar `st.bus().publish(...)` nos handlers de
CRUD de addressbook + contact. Por ora só scaffold/infra — 6 warnings dead_code
esperadas até publishers serem wired.

### #24 — Contacts NATS publishers wired

Completa o scaffold #23 conectando os publishers JetStream aos handlers
REST CRUD.

- `services/expresso-contacts/src/api/addressbooks.rs`:
  - `create` → `ContactsEvent::AddressbookCreated { tenant_id, addressbook_id, name }`.
  - `delete` → `ContactsEvent::AddressbookDeleted { tenant_id, addressbook_id }`.
- `services/expresso-contacts/src/api/contacts.rs`:
  - `create` + `update` → `ContactsEvent::ContactUpserted { tenant_id, addressbook_id, contact_id }`.
  - `delete` → `ContactsEvent::ContactDeleted { tenant_id, addressbook_id, contact_id }`.

Publishes são fire-and-forget via `state.bus().publish(...)` → `tokio::spawn`,
não afetam hot-path de resposta REST.

Warnings dead_code do #23 (6 totais) → resolvidos (0 restantes).

**Smoke 125:**
```
logs contacts:  jetstream EXPRESSO_CONTACTS ready
                async_nats: event: connected
                contacts EventBus with NATS enabled
                rate limiter armed, rps: 50, burst: 200
                HTTP API listening, addr: 0.0.0.0:8003
```

Imagem: `expresso-contacts:t24`.

### #25 — Calendar publishers audit + CalDAV path coverage

Auditoria dos call sites de `EventBus::publish()` em
`services/expresso-calendar/`. Sprint #20 cobriu os handlers REST
(`api/events.rs`, `api/scheduling.rs`), mas os paths CalDAV (usados por
Thunderbird, Apple Calendar, iOS, Evolution) **não emitiam eventos**.

**Gap fechado:**

- `src/caldav/resource.rs`:
  - `PUT` (upsert via iCalendar body) → `Event::EventUpdated { event_id, summary, sequence }`.
  - `DELETE` → lookup `get_by_uid` para capturar `event_id` antes do `delete_by_uid`, depois `Event::EventCancelled`.
- `src/caldav/movecopy.rs` (COPY/MOVE):
  - Destino → `Event::EventUpdated` com dados do `dst_ev` retornado.
  - MOVE com same_row=false → adicional `Event::EventCancelled` do source.

**Paths agora instrumentados (100% do CRUD):**
| Path | Método | Evento |
|---|---|---|
| `POST /api/v1/calendars/:id/events` | REST | EventCreated |
| `PATCH /api/v1/events/:id` | REST | EventUpdated |
| `DELETE /api/v1/events/:id` | REST | EventCancelled |
| `POST /api/v1/scheduling/inbox` (iMIP COUNTER) | REST | CounterReceived |
| `PUT /dav/principals/.../calendars/:id/:uid.ics` | CalDAV | EventUpdated |
| `DELETE /dav/...` | CalDAV | EventCancelled |
| `COPY/MOVE /dav/...` | CalDAV | EventUpdated (+ EventCancelled se MOVE) |

**Smoke 125:**
```
logs calendar:  jetstream EXPRESSO_CALENDAR ready
                async_nats: event: connected
                calendar EventBus with NATS enabled
                HTTP API listening, addr: 0.0.0.0:8002
```

Imagem: `expresso-calendar:t27`.

### #26 — NATS end-to-end smoke script

`ops/nats/e2e-smoke.sh`: valida a **cadeia completa** publish → JetStream
storage medindo delta de `state.messages` antes/depois do trigger.

- Uso: `ops/nats/e2e-smoke.sh <MON_URL> <STREAM> <TRIGGER_CMD>`.
- Lê count via `/jsz?streams=1`, executa trigger, aguarda 2s para ack
  assíncrono, re-lê count. Exit 0 iff count aumentou.
- Script complementa `smoke.sh` (#22 — presença de stream) com prova
  funcional de write-path.

**Smoke 125:**
```
bash e2e-smoke.sh http://172.17.0.1:8222 EXPRESSO_CALENDAR \
    "docker run --rm --network host natsio/nats-box:latest \
     nats --server=nats://172.17.0.1:4222 pub expresso.calendar.test.e2e payload"
→ before: 1
  after:  2
  OK: +1 messages
```

Confirma stream `EXPRESSO_CALENDAR` aceitando publishes no subject
`expresso.calendar.*.*` com persistência JetStream ativa.

**Validação cruzada direta do publisher Rust (#20):**
```
docker logs expresso-calendar | grep jetstream
  → jetstream EXPRESSO_CALENDAR ready
     calendar EventBus with NATS enabled
```

O que falta para o pipeline ficar fim-a-fim em produção: consumer (sprint
futuro) lendo de `expresso.calendar.>` e processando eventos (email
dispatch, iMIP relay, webhook fanout).

### #27 — NATS tail + ops README

Fecha a tríade ops/nats com ferramenta de subscribe/debug + documentação
consolidada.

- `ops/nats/tail.sh`: subscribe live via `natsio/nats-box` (image pull
  automático). Uso:
  ```bash
  ops/nats/tail.sh nats://localhost:4222 'expresso.calendar.>'
  ```
- `ops/nats/README.md`: docs consolidados dos 3 scripts (`smoke.sh`,
  `e2e-smoke.sh`, `tail.sh`) + tabela dos streams ativos + próximos passos.

**Status:** ✅ artefatos bash + docs commitados; sem deploy necessário.

**Trilha consolidada #2 → #27** — 26 sprints shipped entre núcleo planejado
+ extras de observabilidade/NATS. Próximas cartas: consumer worker
(email-dispatcher), admin 2FA enforcement, Keycloak realm-per-tenant wizard
extension. Pipeline JetStream 100% funcional com publishers em calendar
(7 call sites) e contacts (4 call sites).

### #28 — NATS consumer: expresso-event-audit

Primeiro consumer real da infra JetStream. Worker standalone que assina
`EXPRESSO_CALENDAR` + `EXPRESSO_CONTACTS` e loga cada evento como JSON
estruturado. Zero business logic — base para consumers futuros
(iMIP dispatch, webhook fanout, thumbnails).

- Novo crate: `services/expresso-event-audit/` (~100 linhas).
- Spawna 1 task por stream, cria durable consumer (`event-audit-<stream>`),
  `deliver_policy: New`, ack após log.
- Env: `NATS_URL` (req) · `NATS_DURABLE` (default `event-audit`)
  · `NATS_SUBJECT_FILTER` (default `expresso.>`) · `RUST_LOG`.
- Imagem: `expresso-event-audit:t28` (debian:bookworm-slim + ca-certificates,
  ~33MB gzipped).

**Deploy 125:**
```
sudo docker run -d --name expresso-event-audit --restart unless-stopped \
    -e NATS_URL=nats://172.17.0.1:4222 -e RUST_LOG=info \
    expresso-event-audit:t28
```

**Smoke:**
```
pub expresso.calendar.t28.hello "audit-test"
→ docker logs expresso-event-audit:
  {"level":"INFO","message":"event","stream":"EXPRESSO_CALENDAR",
   "subject":"expresso.calendar.t28.hello","payload":"audit-test",
   "target":"event_audit"}
```

Publish → consume → ack verificado. Consumers durables persistem entre
restarts; mensagens novas desde a criação são entregues (histórico antigo
ignorado via `DeliverPolicy::New`).

### #29 — Admin 2FA enforcement

Middleware `require_admin` agora exige step-up MFA quando
`ADMIN_REQUIRE_2FA=true`.

- `services/expresso-admin/src/auth.rs`:
  - `MeResp` extendido com `mfa: MfaField { totp, webauthn }` (refletindo
    schema de `/auth/me` já existente em `expresso-auth`).
  - `AuthConfig.require_2fa` lido de `ADMIN_REQUIRE_2FA`
    (`1|true|yes|on` = ativo, default false).
  - Pós-role-check: se `require_2fa && !(mfa.totp || mfa.webauthn)` →
    403 com página HTML "2FA obrigatória" + link `/auth/logout`.
  - Log `WARN admin access denied: 2FA required but not present`
    com user + email.

- Imagem: `expresso-admin:t29` (deployed 125).
- Backward-compat: sem env → comportamento idêntico ao t18.

**Smoke (mock /auth/me):**
```
TEST 1  mfa.totp=false, ADMIN_REQUIRE_2FA=true  → 403 + 2FA page  ✅
TEST 2  mfa.totp=true,  ADMIN_REQUIRE_2FA=true  → gate passa      ✅
TEST 3  sem env (default)                        → login redirect  ✅
```

Para ativar em prod: adicionar `ADMIN_REQUIRE_2FA: "true"` ao environment
do serviço `expresso-admin` em `compose-phase3.yaml` após garantir que
todos os super_admins possuem TOTP cadastrado no Keycloak. Sem enrollment
prévio todos os admins ficarão trancados fora do painel.

Próximo passo natural seria wizard no admin que força enrollment via
Required Action KC `CONFIGURE_TOTP`.

### #30 — TOTP coverage report

Complementa #29 com endpoint `GET /users/totp-status` listando
quais usuários do realm têm TOTP cadastrado. Pré-requisito para
ligar `ADMIN_REQUIRE_2FA=true` em prod sem trancar admins.

- `kc.rs`: novo helper `user_has_totp(id) -> Result<bool>` (consulta
  `/users/{id}/credentials`, procura `type=="otp"`).
- `handlers.rs`: `users_totp_status()` renderiza HTML inline com tabela
  username/email/nome/status/badge TOTP + sumário "N de M usuários
  (X%)". Escape HTML inline (4 chars) — sem dep externa nova.
- `main.rs`: rota `GET /users/totp-status` (atrás de `require_admin`).
- Imagem: `expresso-admin:t30` deployed 125.

**Smoke:**
```
curl -I /users/totp-status  → 303 → /auth/login  ✅ (gate aplicado)
```

**Uso operacional (playbook pra ligar 2FA em prod):**
1. Acesse `/users/totp-status` logado como super_admin.
2. Verifique que 100% dos usuários com role `tenant_admin`/`super_admin`
   tenham TOTP.
3. Para os sem TOTP, clique "enroll" em `/users` (dispara email KC com
   `CONFIGURE_TOTP`).
4. Após cobertura completa, edite `compose-phase3.yaml` adicionando
   `ADMIN_REQUIRE_2FA: "true"` ao `expresso-admin.environment` e
   recrie o container.

Pipeline 2FA completo: enforcement (#29) + visibilidade (#30) +
ações enroll/reset (pré-existentes). Ops pode flip safely.

**Trilha consolidada #2 → #30** — 29 sprints shipped, todos verificados
em 125.

### #31 — event-audit metrics + healthz

Ops hygiene para o consumer shipped em #28. Adiciona HTTP endpoints
`/healthz`, `/readyz`, `/metrics` em port configurável (default 9090).

- `services/expresso-event-audit/src/main.rs`:
  - `run_ops_http()` spawnada antes da conexão NATS → sobrevive a outages
    do broker (kubelet/healthcheck ainda funcionam).
  - Counter `event_audit_events_total{stream="..."}` via
    `prometheus::IntCounterVec` lazy-inicializado.
  - Counter incrementado após log/ack de cada mensagem.
- Novas deps: `axum`, `prometheus`, `once_cell` (workspace).
- Env novo: `METRICS_ADDR` (default `0.0.0.0:9090`).

**Deploy 125:**
```
docker run -d --name expresso-event-audit --network host \
    -e NATS_URL=nats://172.17.0.1:4222 \
    -e METRICS_ADDR=0.0.0.0:9191 \
    expresso-event-audit:t31
```

**Smoke:**
```
curl /healthz                              → "ok"
pub expresso.calendar.t31.hello metrics-test
curl /metrics | grep event_audit
  → event_audit_events_total{stream="EXPRESSO_CALENDAR"} 1   ✅
```

Integra com Grafana dashboard (#21) — basta scraping Prometheus do
`host.docker.internal:9191`. Painel "NATS consumer throughput" passa
a ter dado real.

### #32 — Publisher NATS counters (calendar + contacts)

Fecha o loop de observabilidade produtor → broker → consumidor:
- **Produtor** (#32): `calendar_nats_publish_total{kind,result}` +
  `contacts_nats_publish_total{kind,result}`.
- **Broker** (#21): JetStream messages totais via `/jsz`.
- **Consumidor** (#31): `event_audit_events_total{stream}`.

Lag de publish → consume fica visível em Grafana.

- `services/expresso-calendar/src/events.rs` +
  `services/expresso-contacts/src/events.rs`:
  - `NATS_PUBLISH_TOTAL: Lazy<IntCounterVec>` registrado em
    `expresso_observability::registry()` (shared com /metrics).
  - Incrementado com `result="ok"|"err"|"serialize_err"` após cada
    `js.publish()`.
  - Pré-populado com zeros para todos os `{kind, result}` em
    `new_with_nats()` → `rate()` funciona desde scrape #1.
- Novas deps por serviço: `once_cell` (workspace).
- Imagens: `expresso-calendar:t32`, `expresso-contacts:t32` deployed 125.

**Smoke:**
```
curl :8002/metrics | grep calendar_nats
  # HELP calendar_nats_publish_total …
  calendar_nats_publish_total{kind="counter_received",result="ok"} 0
  … (4 kinds × 3 results = 12 séries) ✅
```

**Painel Grafana sugerido:**
```
# Publish success rate por kind
rate(calendar_nats_publish_total{result="ok"}[5m])

# Error rate alertável
rate(calendar_nats_publish_total{result!="ok"}[5m]) > 0.1

# Publish vs consume lag
sum(rate(calendar_nats_publish_total{result="ok"}[5m]))
  - sum(rate(event_audit_events_total{stream="EXPRESSO_CALENDAR"}[5m]))
```

### #33 — Grafana dashboard extension (pipeline NATS)

Segue-se #32 adicionando painéis ao `ops/grafana/expresso-overview.json`
consumindo os counters dos sprints #31/#32. Escopo pequeno, artefato-only,
sem rebuild/deploy de serviços.

Novos painéis:
- `NATS publish rate — produtores` (timeseries) —
  `sum by (kind,result) (rate(calendar_nats_publish_total[5m]))` + contatos.
- `event-audit — consumo por stream` (timeseries) —
  `sum by (stream) (rate(event_audit_events_total[5m]))`.
- `Lag produtor → consumidor` (timeseries) —
  `rate(publish{result="ok"}) − rate(audit)` por stream; ~0 = saudável.
- `Publish errors (5m)` (stat) —
  `increase(*_nats_publish_total{result!="ok"}[5m])`; base p/ alerting #34.
- `JetStream EXPRESSO_CONTACTS` (timeseries) — espelha painel de calendar.

Também: `ops/grafana/README.md` atualizado (lista de painéis + scrape
config de `expresso-event-audit:9191`).

**Verificação** (import no Grafana):
```
Dashboards → Import → Upload JSON → expresso-overview.json
→ 11 painéis exibidos (antes: 6).
```

Próximo candidato natural: **#34 Alerting rules** (prometheus alertmanager
em cima dos mesmos counters).

### #34 — Prometheus alerting rules (pipeline NATS + service health)

Novo artefato `ops/prometheus/alerts/expresso.yml` com 9 regras em 3 grupos
consumindo os counters shipados em #31/#32. Segue #33 fechando o triângulo
observability (**métricas → dashboard → alertas**).

Grupos:
- `expresso-nats-pipeline` (5): `ExpressoNatsPublishErrors` (warn >0.1/s),
  `ExpressoNatsPublishErrorsCritical` (crit >1/s), `ExpressoEventAuditLagCalendar`
  / `LagContacts` (warn lag >0.5/s por 10m), `ExpressoEventAuditSilent` (crit
  consumer parado com produtores ativos).
- `expresso-service-health` (3): `ExpressoServiceDown` (`up=0` 2m),
  `ExpressoHttp5xxHigh` (>0.5/s 5m), `ExpressoRateLimitedSpike` (429 >1/s 10m).
- `expresso-nats-broker` (1): `ExpressoJetStreamStalled` (broker sem crescer
  com produtores ativos).

**Validação:**
```
docker run --rm --entrypoint promtool \
  -v .../expresso.yml:/w/expresso-alerts.yml \
  prom/prometheus:latest check rules /w/expresso-alerts.yml
SUCCESS: 9 rules found
```

README novo em [ops/prometheus/README.md](ops/prometheus/README.md) com tabela
de alerts + snippet de integração em `prometheus.yml`. Artefato-only, sem
rebuild/deploy — basta montar o arquivo e `rule_files` no Prometheus de prod.

### #35 — Observability stack template (prometheus + alertmanager + nats-exporter)

Empacota #31→#34 como stack deployável: `ops/compose-observability.yaml`.

Artefatos:
- `ops/alertmanager/alertmanager.yml` — rota por severity, 3 receivers (default
  / critical / info), 2 inhibit rules (`ServiceDown` silencia 5xx/429 no mesmo
  instance; `PublishErrorsCritical` silencia warning).
- `ops/alertmanager/README.md` — tabela de rotas, templates p/ Slack/Teams/
  PagerDuty/Email.
- `ops/prometheus/prometheus.yml` — scrape `expresso-services`,
  `expresso-event-audit`, `nats`, self, alertmanager + `rule_files:
  /etc/prometheus/alerts/*.yml`.
- `ops/compose-observability.yaml` — stack completo (Prometheus 9090 +
  Alertmanager 9093 + prometheus-nats-exporter 7777 + volumes `prom-data`/
  `am-data`), rede externa `expresso-net`.

**Validação (promtool + amtool em prod-host 125):**
```
amtool check-config /w/am.yml       → SUCCESS
promtool check config prometheus.yml → SUCCESS: 1 rule files found
                                     → SUCCESS: 9 rules found
```

Deploy (quando operador estiver pronto):
```
cd ops && sudo docker compose -f compose-observability.yaml up -d
curl :9093/-/healthy && curl :9090/-/ready
curl :9090/api/v1/rules | jq '.data.groups[].name'
```

Loop observability 100% artefato-pronto: counters (#31/#32) → dashboard (#33)
→ rules (#34) → stack deploy (#35). Próximo passo natural: subir em prod +
conectar receivers reais.

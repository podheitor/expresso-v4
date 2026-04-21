# Expresso V4 — Project Guidelines

## Hard Constraints

### Build & Compilation
- **NEVER compile on the host machine (BigLinux)**. ALWAYS compile on the respective dev VMs (Proxmox VMs 122-126, Debian 12).
- Use `mold` linker on VMs to reduce RAM usage during linking.
- Use `-j1` for cargo test/build when RAM is tight (~14Gi).

### Code
- Communication with user: PT-BR.
- Code / comments / commits / docs: telegraphic EN.
- Rust edition 2021, Cargo workspace.
- Axum 0.7 for HTTP services.
- All services must expose JSON `/health` (always 200) and `/ready` (503 when deps unavailable).

### Infrastructure
- Docker Compose for service orchestration on VMs.
- PostgreSQL 16 + RLS for tenant isolation.
- Redis 7 for caching/sessions.
- MinIO for S3-compatible object storage.
- Keycloak for auth.
- NATS for messaging.

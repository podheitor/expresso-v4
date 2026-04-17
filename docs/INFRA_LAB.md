# Infra Lab — Proxmox

> Ambiente de desenvolvimento e staging no Proxmox 192.168.194.101

## Credenciais

- **Host**: `192.168.194.101:8006`
- **Usuário API**: `root@pam`
- **Nó**: `proxmoxlab`
- **Credenciais**: ver `CONNECTIONS.md` no Bacula project

## Recursos Disponíveis

| Recurso | Total | Usado | Livre |
|---------|-------|-------|-------|
| RAM | 125.7 GB | 61.3 GB | **64.4 GB** |
| Storage STG-1 | ~600 GB | ~2 GB | **~597 GB** |
| Storage local-lvm | ~762 GB | <1 GB | **~762 GB** |

## VMs Planejadas para Expresso V4

| VMID | Nome | Função | vCPU | RAM | Storage |
|------|------|--------|------|-----|---------|
| 122 | expresso-dev | Dev + build environment | 8 | 16 GB | 100 GB SSD (local-lvm) |
| 123 | expresso-db | PostgreSQL 16 + Redis 7 | 4 | 8 GB | 150 GB (STG-1) |
| 124 | expresso-storage | MinIO object storage | 4 | 8 GB | 500 GB (STG-1) |
| 125 | expresso-services | Keycloak + NATS + mail | 8 | 16 GB | 100 GB (local-lvm) |
| 126 | expresso-obs | Grafana + Prometheus + Loki | 4 | 8 GB | 50 GB (local-lvm) |

**Total estimado**: 28 vCPU, 56 GB RAM, 900 GB storage — dentro do disponível

## OS: Debian 13 "Trixie"

Debian 13 Trixie é o sistema operacional padrão para TODAS as VMs do projeto Expresso V4.

```bash
# Download ISO Debian 13 (quando disponível) via QEMU command
# Enquanto isso, usar Debian 12 via netinstall e upgrade para Trixie

# Via repositório testing:
# /etc/apt/sources.list
deb http://deb.debian.org/debian/ trixie main contrib non-free non-free-firmware
deb-src http://deb.debian.org/debian/ trixie main contrib non-free non-free-firmware
deb http://security.debian.org/debian-security trixie-security main contrib
```

## Script de Provisionamento (Debian 13)

```bash
#!/usr/bin/env bash
# provision-expresso-dev.sh — setup dev environment Debian 13

set -euo pipefail

# Update system
apt-get update && apt-get dist-upgrade -y

# Build essentials
apt-get install -y \
    build-essential curl git wget unzip \
    pkg-config libssl-dev libpq-dev \
    protobuf-compiler cmake clang

# Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup component add clippy rustfmt
cargo install cargo-audit cargo-watch sqlx-cli

# Node.js 22 LTS (pnpm)
curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
apt-get install -y nodejs
corepack enable && corepack prepare pnpm@latest --activate

# Docker (para deps locais)
curl -fsSL https://download.docker.com/linux/debian/gpg | gpg --dearmor -o /usr/share/keyrings/docker.gpg
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/docker.gpg] \
    https://download.docker.com/linux/debian trixie stable" > /etc/apt/sources.list.d/docker.list
apt-get update && apt-get install -y docker-ce docker-ce-cli containerd.io docker-compose-plugin

# PostgreSQL 16 client tools
apt-get install -y postgresql-client-16

# Ferramentas de dev
apt-get install -y \
    htop iotop jq ripgrep fd-find bat eza \
    vim-nox tmux mosh direnv

echo "✅ Dev environment ready"
```

## Docker Compose — Stack Local Dev

```yaml
# compose.yaml — stack local para desenvolvimento
services:
  postgres:
    image: postgres:16-bookworm
    environment:
      POSTGRES_DB: expresso
      POSTGRES_USER: expresso
      POSTGRES_PASSWORD: dev_secret_change_in_prod
    volumes:
      - pgdata:/var/lib/postgresql/data
    ports:
      - "5432:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U expresso"]
      interval: 5s
      timeout: 5s
      retries: 5

  redis:
    image: redis:7-bookworm
    command: redis-server --appendonly yes
    volumes:
      - redisdata:/data
    ports:
      - "6379:6379"

  minio:
    image: quay.io/minio/minio:latest
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: expresso
      MINIO_ROOT_PASSWORD: dev_secret_change_in_prod
    volumes:
      - miniodata:/data
    ports:
      - "9000:9000"
      - "9001:9001"

  nats:
    image: nats:2.10-alpine
    command: -js -m 8222
    ports:
      - "4222:4222"
      - "8222:8222"

  mailpit:
    image: axllent/mailpit:latest
    ports:
      - "1025:1025"   # SMTP dev
      - "8025:8025"   # Web UI
    environment:
      MP_MAX_MESSAGES: 5000

volumes:
  pgdata:
  redisdata:
  miniodata:
```

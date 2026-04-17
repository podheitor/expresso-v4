.PHONY: help dev up down build test lint fmt check clean

# Default target
help:
@echo "Expresso V4 — Makefile"
@echo ""
@echo "  make up         Start all infrastructure (Docker)"
@echo "  make down       Stop infrastructure"
@echo "  make dev        Start infrastructure + watch expresso-mail"
@echo "  make build      Build all Rust services"
@echo "  make test       Run all tests"
@echo "  make lint       Run cargo clippy + eslint"
@echo "  make fmt        Format code (rustfmt + prettier)"
@echo "  make check      Security audit (cargo-audit)"
@echo "  make clean      Remove build artifacts"
@echo "  make migrate    Run database migrations"
@echo "  make seed       Seed development data"

up:
docker compose -f deploy/docker/compose.yaml up -d
@echo "Waiting for services..."
@sleep 5
@docker compose -f deploy/docker/compose.yaml ps

down:
docker compose -f deploy/docker/compose.yaml down

dev: up
RUST_LOG=debug cargo watch -x 'run -p expresso-mail'

build:
cargo build --workspace

build-release:
cargo build --workspace --release

test:
cargo test --workspace -- --nocapture

lint:
cargo clippy --workspace --all-targets --all-features -- -D warnings

fmt:
cargo fmt --all
@if command -v pnpm >/dev/null; then pnpm -r lint --fix; fi

check: lint
cargo audit

migrate:
sqlx migrate run --source migrations

seed:
cargo run -p expresso-admin -- seed-dev

clean:
cargo clean
find . -name "*.log" -delete

# Docker shortcuts
logs:
docker compose -f deploy/docker/compose.yaml logs -f

ps:
docker compose -f deploy/docker/compose.yaml ps

restart-%:
docker compose -f deploy/docker/compose.yaml restart $*

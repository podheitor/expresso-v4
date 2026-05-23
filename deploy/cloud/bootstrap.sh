#!/usr/bin/env bash
# bootstrap.sh — Deploy end-to-end do Expresso V4 em 4 VMs na nuvem.
#
# Uso:
#   bash deploy/cloud/bootstrap.sh [--check] [--step STEP]
#
# Opções:
#   --check      Valida pré-requisitos sem executar nada
#   --step STEP  Executa só um passo (data|app|meet|obs|dns|tls|migrate|smoke)
#
# Pré-requisitos locais:
#   - ansible >= 2.15  (pip install ansible)
#   - ansible collections: community.docker
#     ansible-galaxy collection install community.docker
#   - SSH configurado para as 4 VMs (chave em ~/.ssh/id_ed25519)
#   - deploy/cloud/ansible/inventory.ini preenchido com IPs reais
#   - deploy/cloud/env/{app,data,meet,obs}.env preenchidos
#
# Ordem de execução:
#   1. data  — Postgres, Redis, MinIO, NATS
#   2. app   — Serviços Rust, Keycloak, Nginx, TLS, migrations
#   3. meet  — expresso-meet (WebRTC)
#   4. obs   — Prometheus, Grafana, Alertmanager
#   5. smoke — testa health de todos os serviços

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ANSIBLE_DIR="$SCRIPT_DIR/ansible"
ENV_DIR="$SCRIPT_DIR/env"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
log()  { echo -e "${GREEN}[$(date +%H:%M:%S)] $*${NC}"; }
warn() { echo -e "${YELLOW}[$(date +%H:%M:%S)] WARN: $*${NC}"; }
die()  { echo -e "${RED}[$(date +%H:%M:%S)] ERROR: $*${NC}" >&2; exit 1; }

# ── Parse args ───────────────────────────────────────────────────────────────
CHECK_ONLY=false
ONLY_STEP=""
while [[ $# -gt 0 ]]; do
  case $1 in
    --check)      CHECK_ONLY=true; shift ;;
    --step)       ONLY_STEP="$2"; shift 2 ;;
    *) die "Argumento desconhecido: $1" ;;
  esac
done

# ── Pré-requisitos ───────────────────────────────────────────────────────────
check_prereqs() {
  log "Verificando pré-requisitos..."

  command -v ansible-playbook >/dev/null 2>&1 \
    || die "ansible-playbook não encontrado. Instale: pip install ansible"

  ansible --version | grep -q "^ansible \[core 2\.[1-9][5-9]" \
    || warn "Ansible < 2.15 detectado. Recomendado >= 2.15."

  ansible-galaxy collection list community.docker 2>/dev/null | grep -q community.docker \
    || die "Coleção community.docker ausente. Execute: ansible-galaxy collection install community.docker"

  [[ -f "$ANSIBLE_DIR/inventory.ini" ]] \
    || die "inventory.ini não encontrado em $ANSIBLE_DIR"

  grep -q "<APP_IP>" "$ANSIBLE_DIR/inventory.ini" \
    && die "inventory.ini ainda tem placeholders (<APP_IP> etc). Preencha com IPs reais."

  for vm in app data meet obs; do
    [[ -f "$ENV_DIR/${vm}.env" ]] \
      || die "Arquivo $ENV_DIR/${vm}.env não encontrado. Copie de env.example e preencha."
    grep -q "POSTGRES_PASSWORD=$\|POSTGRES_PASSWORD=change" "$ENV_DIR/${vm}.env" 2>/dev/null \
      && warn "$vm.env: POSTGRES_PASSWORD parece não preenchida."
  done

  log "Pré-requisitos OK."
}

check_prereqs
$CHECK_ONLY && { log "Modo --check: nenhum deploy executado."; exit 0; }

run_step() {
  local step="$1"
  [[ -n "$ONLY_STEP" && "$ONLY_STEP" != "$step" ]] && return 0
  log "=== Passo: $step ==="
}

# ── Passo: data ──────────────────────────────────────────────────────────────
run_step data && {
  log "Subindo VM data (Postgres + MinIO + NATS + Redis)..."
  ansible-playbook -i "$ANSIBLE_DIR/inventory.ini" "$ANSIBLE_DIR/playbook.yml" \
    --limit data \
    --tags all \
    -e "@$ENV_DIR/data.env"
  log "VM data: OK"
}

# ── Passo: app ───────────────────────────────────────────────────────────────
run_step app && {
  log "Subindo VM app (serviços Rust + Keycloak + Nginx + TLS + migrations)..."
  ansible-playbook -i "$ANSIBLE_DIR/inventory.ini" "$ANSIBLE_DIR/playbook.yml" \
    --limit app \
    --tags all \
    -e "@$ENV_DIR/app.env"
  log "VM app: OK"
}

# ── Passo: meet ──────────────────────────────────────────────────────────────
run_step meet && {
  log "Subindo VM meet (expresso-meet WebRTC)..."
  ansible-playbook -i "$ANSIBLE_DIR/inventory.ini" "$ANSIBLE_DIR/playbook.yml" \
    --limit meet \
    --tags all \
    -e "@$ENV_DIR/meet.env"
  log "VM meet: OK"
}

# ── Passo: obs ───────────────────────────────────────────────────────────────
run_step obs && {
  log "Subindo VM obs (Prometheus + Grafana + Alertmanager)..."
  ansible-playbook -i "$ANSIBLE_DIR/inventory.ini" "$ANSIBLE_DIR/playbook.yml" \
    --limit obs \
    --tags all \
    -e "@$ENV_DIR/obs.env"
  log "VM obs: OK"
}

# ── Passo: smoke ─────────────────────────────────────────────────────────────
run_step smoke && {
  # Lê domínio do inventory
  DOMAIN=$(grep '^domain=' "$ANSIBLE_DIR/inventory.ini" | cut -d= -f2 | tr -d ' ')
  [[ -z "$DOMAIN" ]] && die "domain= não encontrado no inventory.ini"

  log "Smoke tests em https://${DOMAIN}..."

  FAIL=0
  check_url() {
    local label="$1" url="$2" expected="${3:-200}"
    local status
    status=$(curl -sf -o /dev/null -w "%{http_code}" --max-time 10 "$url" || echo "000")
    if [[ "$status" == "$expected" ]]; then
      echo -e "  ${GREEN}OK${NC}  $label ($status)"
    else
      echo -e "  ${RED}FAIL${NC} $label (esperado $expected, got $status)"
      ((FAIL++))
    fi
  }

  check_url "Web UI"          "https://${DOMAIN}/"
  check_url "Auth health"     "https://${DOMAIN}/auth/health"
  check_url "Mail health"     "https://mail.${DOMAIN}/health"
  check_url "Meet health"     "https://meet.${DOMAIN}/health"
  check_url "Admin health"    "https://admin.${DOMAIN}/health"
  check_url "Grafana"         "https://grafana.${DOMAIN}/api/health"
  check_url "Keycloak discovery" "https://auth.${DOMAIN}/realms/expresso/.well-known/openid-configuration"
  check_url "IMAP (993 TCP)"  "" # skip — não é HTTP

  if [[ $FAIL -eq 0 ]]; then
    log "Todos os smoke tests passaram."
  else
    die "$FAIL smoke test(s) falharam. Verifique os logs acima."
  fi
}

log ""
log "Deploy concluído."
log "  Web UI:   https://${DOMAIN:-<DOMAIN>}"
log "  Grafana:  https://grafana.${DOMAIN:-<DOMAIN>}"
log "  Keycloak: https://auth.${DOMAIN:-<DOMAIN>}"

#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# Expresso v4 — sobe o ambiente de DEMONSTRAÇÃO num comando.
#
#   bash scripts/demo-up.sh              # detecta o IP da LAN automaticamente
#   HOST_IP=192.168.15.125 bash scripts/demo-up.sh   # força um IP
#
# O HOST_IP é o IP da MÁQUINA QUE RODA ISTO — é o endereço que outras máquinas
# (e o Keycloak) vão usar. Acesse o webmail de qualquer lugar da rede em
# http://<HOST_IP>:3000. Login de demonstração: patricia / patricia.
#
# NÃO use em produção — os segredos abaixo são fracos de propósito.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail
cd "$(dirname "$0")/.."

# ── 0. Pré-requisitos ────────────────────────────────────────────────────────
command -v docker >/dev/null 2>&1 || { echo "ERRO: docker não encontrado. Instale o Docker nesta máquina." >&2; exit 1; }
docker compose version >/dev/null 2>&1 || { echo "ERRO: 'docker compose' (plugin v2) não disponível." >&2; exit 1; }
# Ferramentas que os scripts de seed usam no host (não dentro dos containers).
for t in curl python3 uuidgen; do
  command -v "$t" >/dev/null 2>&1 || { echo "ERRO: '$t' não encontrado (necessário para os seeds)." >&2; exit 1; }
done

# ── 1. IP do host (sem inventar: detecta ou usa o que você passar) ───────────
if [[ -z "${HOST_IP:-}" ]]; then
  HOST_IP=$(ip route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="src"){print $(i+1); exit}}')
  HOST_IP="${HOST_IP:-localhost}"
fi
echo "==> HOST_IP = $HOST_IP  (acesse o webmail em http://$HOST_IP:3000)"

# ── 2. .env de demonstração (segredos fracos; sobrescreve só se ausente) ─────
if [[ ! -f .env ]]; then
  cat > .env <<ENV
POSTGRES_USER=expresso
POSTGRES_PASSWORD=demo-pg-pass
S3_ACCESS_KEY=expresso
S3_SECRET_KEY=demo-s3-secret
S3_BUCKET=expresso
KEYCLOAK_ADMIN=admin
KEYCLOAK_ADMIN_PASSWORD=demo-admin-pass
KEYCLOAK_REALM=expresso
KEYCLOAK_CLIENT_ID=expresso-backend
KEYCLOAK_CLIENT_SECRET=demo-kc-secret
KEYCLOAK_HOSTNAME=$HOST_IP
GRAFANA_ADMIN_PASSWORD=demo-grafana
MAIL_DOMAIN=mail.expresso.local
RUST_LOG=info,expresso=debug
ENV
  echo "==> .env de demo criado (KEYCLOAK_HOSTNAME=$HOST_IP)"
else
  echo "==> .env já existe — mantido (apague para regenerar)"
fi
set -a; . ./.env; set +a

# ── 3. Sobe a stack (primeira vez compila as imagens Rust — pode levar minutos) ─
echo "==> docker compose up -d  (a 1ª vez compila tudo; aguarde)"
docker compose up -d --build

# ── 4. Espera os serviços ficarem healthy (poll real, não sleep cego) ────────
wait_healthy() {
  local svc="$1" tries="${2:-60}"
  echo -n "    aguardando $svc "
  for _ in $(seq "$tries"); do
    local st
    st=$(docker compose ps --format '{{.Health}}' "$svc" 2>/dev/null || true)
    case "$st" in
      healthy) echo " healthy"; return 0 ;;
      *) echo -n "."; sleep 3 ;;
    esac
  done
  echo " TIMEOUT (siga mesmo assim; veja: docker compose logs $svc)"; return 1
}
wait_healthy expresso-mail   || true   # mail roda TODAS as migrations no boot (sqlx::migrate!)
wait_healthy expresso-auth   || true
wait_healthy expresso-web    120 || true

# ── 5. Schema: o boot do expresso-mail já aplicou as 109 migrations ──────────
echo "==> migrations aplicadas pelo boot do expresso-mail (sqlx migrate)."

# ── 6. Keycloak: cria realm + cliente + usuário de demo (patricia/patricia) ──
echo "==> seed do Keycloak (realm + patricia/patricia)"
KC_URL="http://$HOST_IP:8080" KC_ADMIN="${KEYCLOAK_ADMIN:-admin}" KC_ADMIN_PASS="${KEYCLOAK_ADMIN_PASSWORD:-demo-admin-pass}" \
  bash deploy/keycloak/seed-realm.sh || echo "   (seed-realm falhou — veja se o Keycloak já subiu; pode reexecutar)"

# ── 7. Dados de demo (agenda, contatos, drive, mail) sob a patricia ──────────
echo "==> seed de dados de demonstração"
HOST="$HOST_IP" bash scripts/seed-demo.sh || echo "   (seed-demo falhou — reexecute após os serviços ficarem healthy)"

# ── 8. Resumo ────────────────────────────────────────────────────────────────
cat <<DONE

────────────────────────────────────────────────────────────────────
 Expresso v4 — ambiente de DEMO no ar
────────────────────────────────────────────────────────────────────
  Webmail .......... http://$HOST_IP:3000     (login: patricia / patricia)
  Painel Admin ..... http://$HOST_IP:8101
  Keycloak ......... http://$HOST_IP:8080     (admin / ${KEYCLOAK_ADMIN_PASSWORD:-demo-admin-pass})
  Grafana .......... http://$HOST_IP:3001     (admin / ${GRAFANA_ADMIN_PASSWORD:-demo-grafana})

  Status:    docker compose ps
  Logs:      docker compose logs -f expresso-web
  Derrubar:  docker compose down        (apaga dados:  docker compose down -v)
────────────────────────────────────────────────────────────────────
DONE

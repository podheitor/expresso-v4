#!/usr/bin/env bash
# Roda todos os perfis SLA contra o stack local. Requer TOKEN e serviços healthy.
# Uso: TOKEN=<jwt> bash tests/load/run-all.sh [smoke|sla|stress]
set -euo pipefail

MODE="${1:-sla}"
TOKEN="${TOKEN:-}"

if [[ -z "$TOKEN" ]]; then
  echo "ERROR: variável TOKEN não definida. Exporte um JWT válido antes de executar." >&2
  exit 1
fi

declare -A SERVICES=(
  [auth]="http://localhost:8100"
  [mail]="http://localhost:8001"
  [calendar]="http://localhost:8002"
  [drive]="http://localhost:8004"
  [search]="http://localhost:8007"
)

PASS=0
FAIL=0

for svc in "${!SERVICES[@]}"; do
  script="tests/load/${svc}-sla.js"
  if [[ ! -f "$script" ]]; then
    echo "SKIP $svc — $script não encontrado"
    continue
  fi

  echo ""
  echo "=== $svc ($MODE) ==="
  if BASE_URL="${SERVICES[$svc]}" TOKEN="$TOKEN" k6 run "$script"; then
    echo "PASS: $svc"
    ((PASS++))
  else
    echo "FAIL: $svc"
    ((FAIL++))
  fi
done

echo ""
echo "Resultado: ${PASS} PASS / ${FAIL} FAIL"
[[ "$FAIL" -eq 0 ]]

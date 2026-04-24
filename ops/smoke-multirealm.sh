#!/bin/bash
# Smoke test for multi-realm JWT validation in prod.
# Usage: PILOT_REALM=<uuid> PILOT_PASS=<pwd> ./smoke-multirealm.sh [host]
#
# Env defaults assume prod 125 context. Override for other envs.
#   KC_URL                (default http://127.0.0.1:8080)
#   AUTH_RP_URL           (default http://127.0.0.1:8012)
#   PROM_URL              (default http://127.0.0.1:9090)
#   PILOT_REALM           (required)
#   PILOT_CLIENT_ID       (default expresso-dav)
#   PILOT_CLIENT_SECRET   (required)
#   PILOT_USER            (default admin@pilot.expresso.local)
#   PILOT_PASS            (required)
#   TENANT_HOST           (default pilot.expresso.local)
#
# Exit codes: 0=ok, 1=token_fail, 2=auth_me_fail, 3=metric_missing
set -euo pipefail

KC_URL="${KC_URL:-http://127.0.0.1:8080}"
AUTH_RP_URL="${AUTH_RP_URL:-http://127.0.0.1:8012}"
PROM_URL="${PROM_URL:-http://127.0.0.1:9090}"
PILOT_CLIENT_ID="${PILOT_CLIENT_ID:-expresso-dav}"
PILOT_USER="${PILOT_USER:-admin@pilot.expresso.local}"
TENANT_HOST="${TENANT_HOST:-pilot.expresso.local}"
: "${PILOT_REALM:?PILOT_REALM required}"
: "${PILOT_CLIENT_SECRET:?PILOT_CLIENT_SECRET required}"
: "${PILOT_PASS:?PILOT_PASS required}"

echo "[1/3] issue JWT from realm=$PILOT_REALM"
RESP=$(curl -sS -X POST "$KC_URL/realms/$PILOT_REALM/protocol/openid-connect/token" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data-urlencode 'grant_type=password' \
  --data-urlencode "client_id=$PILOT_CLIENT_ID" \
  --data-urlencode "client_secret=$PILOT_CLIENT_SECRET" \
  --data-urlencode "username=$PILOT_USER" \
  --data-urlencode "password=$PILOT_PASS")
TOKEN=$(echo "$RESP" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("access_token","NONE"))')
[[ "$TOKEN" == "NONE" ]] && { echo "FAIL token: ${RESP:0:200}"; exit 1; }
echo "  ok token_len=${#TOKEN}"

echo "[2/3] GET /auth/me with Host=$TENANT_HOST"
CODE=$(curl -sS -o /tmp/smoke-me.json -w "%{http_code}" \
  -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" \
  "$AUTH_RP_URL/auth/me")
[[ "$CODE" != "200" ]] && { echo "FAIL /auth/me code=$CODE body=$(head -c 200 /tmp/smoke-me.json)"; exit 2; }
TENANT_CLAIM=$(python3 -c 'import json;print(json.load(open("/tmp/smoke-me.json")).get("tenant_id",""))')
[[ "$TENANT_CLAIM" != "$PILOT_REALM" ]] && { echo "FAIL tenant_id mismatch: got=$TENANT_CLAIM want=$PILOT_REALM"; exit 2; }
echo "  ok tenant_id=$TENANT_CLAIM"

echo "[3/3] check prometheus metric auth_validation_total{realm=$PILOT_REALM,result=ok}"
Q=$(python3 -c "import urllib.parse;print(urllib.parse.quote(f'auth_validation_total{{realm=\"$PILOT_REALM\",result=\"ok\"}}'))")
COUNT=$(curl -sS "$PROM_URL/api/v1/query?query=$Q" | python3 -c 'import sys,json;r=json.load(sys.stdin)["data"]["result"];print(r[0]["value"][1] if r else "MISSING")')
[[ "$COUNT" == "MISSING" ]] && { echo "FAIL metric absent"; exit 3; }
echo "  ok count=$COUNT"
echo "SMOKE PASS"

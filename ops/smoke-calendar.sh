#!/bin/bash
set -uo pipefail
KC_URL="${KC_URL:-http://127.0.0.1:8080}"
CAL_URL="${CAL_URL:-http://127.0.0.1:8002}"
PILOT_CLIENT_ID="${PILOT_CLIENT_ID:-expresso-dav}"
PILOT_USER="${PILOT_USER:-admin@pilot.expresso.local}"
TENANT_HOST="${TENANT_HOST:-pilot.expresso.local}"
: "${PILOT_REALM:?}"; : "${PILOT_CLIENT_SECRET:?}"; : "${PILOT_PASS:?}"
echo "[1/2] JWT realm=$PILOT_REALM"
RESP=$(curl -sS -X POST "$KC_URL/realms/$PILOT_REALM/protocol/openid-connect/token" \
  --data-urlencode grant_type=password \
  --data-urlencode "client_id=$PILOT_CLIENT_ID" \
  --data-urlencode "client_secret=$PILOT_CLIENT_SECRET" \
  --data-urlencode "username=$PILOT_USER" \
  --data-urlencode "password=$PILOT_PASS")
TOKEN=$(echo "$RESP" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("access_token","NONE"))')
[[ "$TOKEN" == "NONE" ]] && { echo "FAIL token: ${RESP:0:200}"; exit 1; }
echo "  ok token_len=${#TOKEN}"
echo "[2/2] GET $CAL_URL/api/v1/calendars Host=$TENANT_HOST"
CODE=$(curl -sS -o /tmp/cal-resp.json -w "%{http_code}" \
  -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" \
  "$CAL_URL/api/v1/calendars")
echo "  http=$CODE body=$(head -c 300 /tmp/cal-resp.json)"
[[ "$CODE" == "200" ]] && echo "SMOKE PASS" || { echo "SMOKE FAIL"; exit 2; }

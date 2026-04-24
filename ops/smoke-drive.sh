#!/bin/bash
set -uo pipefail
KC_URL="${KC_URL:-http://127.0.0.1:8080}"
DRIVE_URL="${DRIVE_URL:-http://127.0.0.1:8004}"
PILOT_CLIENT_ID="${PILOT_CLIENT_ID:-expresso-dav}"
: "${PILOT_REALM:?}"; : "${PILOT_CLIENT_SECRET:?}"; : "${PILOT_PASS:?}"; : "${PILOT_USER:?}"; : "${TENANT_HOST:?}"
RESP=$(curl -sS -X POST "$KC_URL/realms/$PILOT_REALM/protocol/openid-connect/token" \
  --data-urlencode grant_type=password --data-urlencode "client_id=$PILOT_CLIENT_ID" \
  --data-urlencode "client_secret=$PILOT_CLIENT_SECRET" \
  --data-urlencode "username=$PILOT_USER" --data-urlencode "password=$PILOT_PASS")
TOKEN=$(echo "$RESP" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("access_token","NONE"))')
[[ "$TOKEN" == "NONE" ]] && { echo "FAIL token"; exit 1; }
echo "JWT token_len=${#TOKEN}"
CODE=$(curl -sS -o /tmp/dr.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$DRIVE_URL/api/v1/drive/files")
echo "http=$CODE body=$(head -c 300 /tmp/dr.json)"
[[ "$CODE" == "200" || "$CODE" == "404" ]] && echo "SMOKE PASS (auth ok)" || { echo "SMOKE FAIL"; exit 2; }

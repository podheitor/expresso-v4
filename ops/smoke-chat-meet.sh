#!/bin/bash
set -uo pipefail
KC_URL="${KC_URL:-http://127.0.0.1:8080}"
CHAT_URL="${CHAT_URL:-http://127.0.0.1:8010}"
MEET_URL="${MEET_URL:-http://127.0.0.1:8011}"
PILOT_CLIENT_ID="${PILOT_CLIENT_ID:-expresso-dav}"
: "${PILOT_REALM:?}"; : "${PILOT_CLIENT_SECRET:?}"; : "${PILOT_PASS:?}"; : "${PILOT_USER:?}"; : "${TENANT_HOST:?}"
RESP=$(curl -sS -X POST "$KC_URL/realms/$PILOT_REALM/protocol/openid-connect/token" \
  --data-urlencode grant_type=password --data-urlencode "client_id=$PILOT_CLIENT_ID" \
  --data-urlencode "client_secret=$PILOT_CLIENT_SECRET" \
  --data-urlencode "username=$PILOT_USER" --data-urlencode "password=$PILOT_PASS")
TOKEN=$(echo "$RESP" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("access_token","NONE"))')
[[ "$TOKEN" == "NONE" ]] && { echo "FAIL token: ${RESP:0:200}"; exit 1; }
echo "JWT token_len=${#TOKEN}"
CODE=$(curl -sS -o /tmp/c.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$CHAT_URL/api/v1/channels")
echo "chat http=$CODE body=$(head -c 200 /tmp/c.json)"
CH_OK=$([[ "$CODE" == "200" || "$CODE" == "404" ]] && echo 1 || echo 0)
CODE=$(curl -sS -o /tmp/m.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$MEET_URL/api/v1/meetings")
echo "meet http=$CODE body=$(head -c 200 /tmp/m.json)"
ME_OK=$([[ "$CODE" == "200" || "$CODE" == "404" ]] && echo 1 || echo 0)
[[ $CH_OK == 1 && $ME_OK == 1 ]] && echo "SMOKE PASS" || { echo "SMOKE FAIL"; exit 2; }

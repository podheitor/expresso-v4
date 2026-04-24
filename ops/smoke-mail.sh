#!/bin/bash
set -uo pipefail
KC_URL="${KC_URL:-http://127.0.0.1:8080}"
MAIL_URL="${MAIL_URL:-http://127.0.0.1:8001}"
PILOT_CLIENT_ID="${PILOT_CLIENT_ID:-expresso-dav}"
: "${PILOT_REALM:?}"; : "${PILOT_CLIENT_SECRET:?}"; : "${PILOT_PASS:?}"; : "${PILOT_USER:?}"; : "${TENANT_HOST:?}"
RESP=$(curl -sS -X POST "$KC_URL/realms/$PILOT_REALM/protocol/openid-connect/token" \
  --data-urlencode grant_type=password --data-urlencode "client_id=$PILOT_CLIENT_ID" \
  --data-urlencode "client_secret=$PILOT_CLIENT_SECRET" \
  --data-urlencode "username=$PILOT_USER" --data-urlencode "password=$PILOT_PASS")
TOKEN=$(echo "$RESP" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("access_token","NONE"))')
[[ "$TOKEN" == "NONE" ]] && { echo "FAIL token"; exit 1; }
echo "JWT token_len=${#TOKEN}"
CODE=$(curl -sS -o /tmp/mf.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$MAIL_URL/api/v1/mail/folders")
echo "folders http=$CODE body=$(head -c 200 /tmp/mf.json)"
[[ "$CODE" == "200" ]] || { echo "SMOKE FAIL folders"; exit 2; }

# Thread probe — pick a thread_id from INBOX listing (if any) and re-fetch by thread
CODE_L=$(curl -sS -o /tmp/ml.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$MAIL_URL/api/v1/mail/messages?folder=INBOX&limit=50")
echo "messages http=$CODE_L"
[[ "$CODE_L" == "200" ]] || { echo "SMOKE FAIL messages"; exit 2; }

TID=$(python3 -c 'import json
rows=json.load(open("/tmp/ml.json"))
print(next((r["thread_id"] for r in rows if r.get("thread_id")), ""))')

if [[ -n "$TID" ]]; then
  CODE_T=$(curl -sS -o /tmp/mt.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$MAIL_URL/api/v1/mail/threads/$TID")
  N=$(python3 -c 'import json;print(len(json.load(open("/tmp/mt.json"))))')
  echo "thread http=$CODE_T tid=$TID count=$N"
  [[ "$CODE_T" == "200" && "$N" -ge 1 ]] || { echo "SMOKE FAIL thread"; exit 2; }
else
  echo "thread skipped: no threaded messages in INBOX (empty or single)"
fi

echo "SMOKE PASS"

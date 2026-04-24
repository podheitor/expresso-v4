#!/bin/bash
# Smoke test calendar + contacts multi-realm (tenant=%i via env file).
# Env: PILOT_REALM, PILOT_CLIENT_SECRET, PILOT_PASS, PILOT_USER, TENANT_HOST
# Optional: KC_URL, CAL_URL, CON_URL, PILOT_CLIENT_ID, PUSHGATEWAY_URL, SMOKE_JOB
set -uo pipefail
KC_URL="${KC_URL:-http://127.0.0.1:8080}"
CAL_URL="${CAL_URL:-http://127.0.0.1:8002}"
CON_URL="${CON_URL:-http://127.0.0.1:8003}"
PILOT_CLIENT_ID="${PILOT_CLIENT_ID:-expresso-dav}"
PUSHGATEWAY_URL="${PUSHGATEWAY_URL:-}"
SMOKE_JOB="${SMOKE_JOB:-smoke_dav}"
: "${PILOT_REALM:?}"; : "${PILOT_CLIENT_SECRET:?}"; : "${PILOT_PASS:?}"
: "${PILOT_USER:?}"; : "${TENANT_HOST:?}"

push_metric() {
  local svc="$1" success="$2"
  [[ -z "$PUSHGATEWAY_URL" ]] && return 0
  local ts; ts=$(date +%s)
  curl -sS --data-binary @- "$PUSHGATEWAY_URL/metrics/job/$SMOKE_JOB/tenant/$PILOT_REALM/service/$svc" >/dev/null 2>&1 <<PUSH || true
# TYPE expresso_smoke_dav_success gauge
expresso_smoke_dav_success $success
# TYPE expresso_smoke_dav_last_run_timestamp_seconds gauge
expresso_smoke_dav_last_run_timestamp_seconds $ts
PUSH
}

echo "[1/7] JWT realm=$PILOT_REALM"
RESP=$(curl -sS -X POST "$KC_URL/realms/$PILOT_REALM/protocol/openid-connect/token" \
  --data-urlencode grant_type=password --data-urlencode "client_id=$PILOT_CLIENT_ID" \
  --data-urlencode "client_secret=$PILOT_CLIENT_SECRET" \
  --data-urlencode "username=$PILOT_USER" --data-urlencode "password=$PILOT_PASS")
TOKEN=$(echo "$RESP" | python3 -c 'import sys,json;print(json.load(sys.stdin).get("access_token","NONE"))')
[[ "$TOKEN" == "NONE" ]] && { echo "FAIL token: ${RESP:0:200}"; push_metric calendar 0; push_metric contacts 0; exit 1; }
echo "  ok token_len=${#TOKEN}"

RC=0
echo "[2/7] GET $CAL_URL/api/v1/calendars Host=$TENANT_HOST"
C=$(curl -sS -o /tmp/sd-cal.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$CAL_URL/api/v1/calendars")
if [[ "$C" == "200" ]]; then echo "  calendar PASS"; push_metric calendar 1; else echo "  calendar FAIL http=$C"; push_metric calendar 0; RC=2; fi

echo "[3/7] GET $CON_URL/api/v1/addressbooks Host=$TENANT_HOST"
C=$(curl -sS -o /tmp/sd-con.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$CON_URL/api/v1/addressbooks")
if [[ "$C" == "200" ]]; then echo "  contacts PASS"; push_metric contacts 1; else echo "  contacts FAIL http=$C"; push_metric contacts 0; RC=3; fi


DRIVE_URL="${DRIVE_URL:-http://127.0.0.1:8004}"
echo "[4/7] GET $DRIVE_URL/api/v1/drive/files Host=$TENANT_HOST"
C=$(curl -sS -o /tmp/sd-drv.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$DRIVE_URL/api/v1/drive/files")
if [[ "$C" == "200" ]]; then echo "  drive PASS"; push_metric drive 1; else echo "  drive FAIL http=$C"; push_metric drive 0; RC=4; fi


MAIL_URL="${MAIL_URL:-http://127.0.0.1:8001}"
echo "[5/7] GET $MAIL_URL/api/v1/mail/folders Host=$TENANT_HOST"
C=$(curl -sS -o /tmp/sd-mail.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$MAIL_URL/api/v1/mail/folders")
if [[ "$C" == "200" ]]; then echo "  mail PASS"; push_metric mail 1; else echo "  mail FAIL http=$C"; push_metric mail 0; RC=5; fi



CHAT_URL="${CHAT_URL:-http://127.0.0.1:8010}"
echo "[6/7] GET $CHAT_URL/api/v1/channels Host=$TENANT_HOST"
C=$(curl -sS -o /tmp/sd-chat.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$CHAT_URL/api/v1/channels")
if [[ "$C" == "200" ]]; then echo "  chat PASS"; push_metric chat 1; else echo "  chat FAIL http=$C"; push_metric chat 0; RC=6; fi


MEET_URL="${MEET_URL:-http://127.0.0.1:8011}"
echo "[7/7] GET $MEET_URL/api/v1/meetings Host=$TENANT_HOST"
C=$(curl -sS -o /tmp/sd-meet.json -w "%{http_code}" -H "Host: $TENANT_HOST" -H "Authorization: Bearer $TOKEN" "$MEET_URL/api/v1/meetings")
if [[ "$C" == "200" ]]; then echo "  meet PASS"; push_metric meet 1; else echo "  meet FAIL http=$C"; push_metric meet 0; RC=7; fi

[[ $RC -eq 0 ]] && echo "SMOKE PASS" || echo "SMOKE FAIL rc=$RC"
exit $RC

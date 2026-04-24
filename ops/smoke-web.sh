#!/bin/bash
# Smoke test for web UI multi-tenant login flow (Sprint #46).
#
# Probes the /login page + /auth/login redirect for each tenant host and asserts:
#   1. /login returns 200 with tenant-aware HTML (contains href="/auth/login")
#   2. /auth/login returns 303 with Location → Keycloak realm matching host
#   3. redirect_uri in Location matches https://<host>/auth/callback
#
# Required env: (defaults OK for single-node prod)
#   WEB_HOST_IP        reverse-proxy IP (default 127.0.0.1)
#   PILOT_HOST         tenant 1 host  (default pilot.expresso.local)
#   PILOT_REALM        tenant 1 realm UUID
#   PILOT2_HOST        tenant 2 host  (default pilot2.expresso.local)
#   PILOT2_REALM       tenant 2 realm UUID
#   FALLBACK_HOST      host NOT in AUTH__TENANT_HOSTS (default expresso.local)
#   FALLBACK_REALM     fallback realm slug/uuid (default expresso)
# Optional:
#   PUSHGATEWAY_URL SMOKE_JOB (=smoke_web)
# Exit codes: 0=ok, 1=/login fail, 2=/auth/login fail, 3=redirect_uri mismatch, 4=realm mismatch
set -uo pipefail

WEB_HOST_IP="${WEB_HOST_IP:-127.0.0.1}"
PILOT_HOST="${PILOT_HOST:-pilot.expresso.local}"
PILOT2_HOST="${PILOT2_HOST:-pilot2.expresso.local}"
FALLBACK_HOST="${FALLBACK_HOST:-expresso.local}"
FALLBACK_REALM="${FALLBACK_REALM:-expresso}"
PUSHGATEWAY_URL="${PUSHGATEWAY_URL:-}"
SMOKE_JOB="${SMOKE_JOB:-smoke_web}"
: "${PILOT_REALM:?PILOT_REALM required}"
: "${PILOT2_REALM:?PILOT2_REALM required}"

push_metric() {
  local success="$1" tenant="$2"
  [[ -z "$PUSHGATEWAY_URL" ]] && return 0
  local ts; ts=$(date +%s)
  curl -sS --data-binary @- "$PUSHGATEWAY_URL/metrics/job/$SMOKE_JOB/tenant/$tenant" >/dev/null 2>&1 <<PUSH || true
# TYPE expresso_smoke_web_success gauge
expresso_smoke_web_success $success
# TYPE expresso_smoke_web_last_run_timestamp_seconds gauge
expresso_smoke_web_last_run_timestamp_seconds $ts
PUSH
}

RESULT=0
LAST_TENANT="multi"
on_exit() {
  local ec=$?
  if [[ $ec -eq 0 ]]; then push_metric 1 "$LAST_TENANT"; else push_metric 0 "$LAST_TENANT"; fi
}
trap on_exit EXIT

probe_tenant() {
  local host="$1" expected_realm="$2" label="$3"
  LAST_TENANT="$label"
  echo "[$label] host=$host expected_realm=$expected_realm"

  # Probe 1: /login renders
  local body_code
  body_code=$(curl -sk -o /tmp/smoke-web-login.html -w "%{http_code}" \
    --resolve "${host}:443:${WEB_HOST_IP}" "https://${host}/login")
  if [[ "$body_code" != "200" ]]; then
    echo "  FAIL /login code=$body_code"; return 1
  fi
  if ! grep -qE 'href="/auth/login[?"]' /tmp/smoke-web-login.html; then
    echo "  FAIL /login missing relative /auth/login link"; return 1
  fi
  echo "  ok /login renders"

  # Probe 2: /auth/login → 303
  local loc
  loc=$(curl -sk -o /dev/null -D - -w "%{http_code}" \
    --resolve "${host}:443:${WEB_HOST_IP}" "https://${host}/auth/login" \
    | awk 'tolower($1)=="location:"{print $2}' | tr -d '\r')
  if [[ -z "$loc" ]]; then
    echo "  FAIL /auth/login no Location header"; return 2
  fi

  # Probe 3: realm in Location path
  if [[ "$loc" != *"/realms/${expected_realm}/"* ]]; then
    echo "  FAIL realm mismatch: got=$loc want=*realms/${expected_realm}/*"; return 4
  fi

  # Probe 4: redirect_uri host matches
  if [[ "$loc" != *"redirect_uri=https%3A%2F%2F${host}%2Fauth%2Fcallback"* ]]; then
    echo "  FAIL redirect_uri mismatch: $loc"; return 3
  fi

  echo "  ok /auth/login → realm=$expected_realm redirect_uri=https://$host/auth/callback"
}

probe_tenant "$PILOT_HOST"    "$PILOT_REALM"    "pilot"    || exit $?
probe_tenant "$PILOT2_HOST"   "$PILOT2_REALM"   "pilot2"   || exit $?
probe_tenant "$FALLBACK_HOST" "$FALLBACK_REALM" "fallback" || exit $?

echo "SMOKE WEB PASS"

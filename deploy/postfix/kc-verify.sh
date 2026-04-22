#!/bin/bash
# Dovecot checkpassword → Keycloak password grant
# FD 3: user\0pass\0timestamp\0
set -u
: "${KC_URL:?KC_URL required}"
: "${KC_REALM:?KC_REALM required}"
: "${KC_CLIENT_ID:?KC_CLIENT_ID required}"
: "${KC_CLIENT_SECRET:?KC_CLIENT_SECRET required}"

# Read NUL-delimited fields directly (avoids $() which strips nulls)
IFS= read -rd '' user <&3 || true
IFS= read -rd '' pass <&3 || true

username="${user%@*}"

code=$(curl -s -o /dev/null -w '%{http_code}' -m 5 \
  -X POST "${KC_URL}/realms/${KC_REALM}/protocol/openid-connect/token" \
  --data-urlencode "client_id=${KC_CLIENT_ID}" \
  --data-urlencode "client_secret=${KC_CLIENT_SECRET}" \
  --data-urlencode "grant_type=password" \
  --data-urlencode "username=${username}" \
  --data-urlencode "password=${pass}")

if [[ "$code" == "200" ]]; then
  export USER="$user"
  exec "$@"
fi
if [[ "$code" =~ ^5 ]]; then exit 111; fi
exit 1

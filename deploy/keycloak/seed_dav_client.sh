#!/usr/bin/env bash
# Seed / update the `expresso-dav` confidential client in the Keycloak
# `expresso` realm. Enables Direct Access Grants (password flow) so CalDAV
# and CardDAV services can validate HTTP Basic credentials.
#
# Usage:
#   KC_URL=http://expresso-keycloak:8080 \
#   KC_ADMIN_USER=admin KC_ADMIN_PASS=... \
#   KC_REALM=expresso   KC_CLIENT=expresso-dav \
#     ./seed_dav_client.sh
#
# Prints the final `<client-id>:<secret>` pair on stdout.

set -euo pipefail

KC_URL="${KC_URL:-http://localhost:8080}"
KC_REALM="${KC_REALM:-expresso}"
KC_CLIENT="${KC_CLIENT:-expresso-dav}"
KC_ADMIN_USER="${KC_ADMIN_USER:-admin}"
KC_ADMIN_PASS="${KC_ADMIN_PASS:?KC_ADMIN_PASS required}"

err() { echo "ERROR: $*" >&2; exit 1; }

# ── 1) Admin token (master realm, admin-cli) ────────────────
TOKEN=$(curl -sS -X POST "${KC_URL}/realms/master/protocol/openid-connect/token" \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=password" \
  -d "client_id=admin-cli" \
  -d "username=${KC_ADMIN_USER}" \
  --data-urlencode "password=${KC_ADMIN_PASS}" \
  | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
[[ -n "$TOKEN" ]] || err "admin token empty; check KC_URL/KC_ADMIN_USER/KC_ADMIN_PASS"

auth() { curl -sS -H "Authorization: Bearer $TOKEN" "$@"; }

# ── 2) Check existence ──────────────────────────────────────
LIST=$(auth "${KC_URL}/admin/realms/${KC_REALM}/clients?clientId=${KC_CLIENT}")
EXISTING_ID=$(echo "$LIST" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)

CLIENT_JSON=$(cat <<JSON
{
  "clientId": "${KC_CLIENT}",
  "name": "Expresso CalDAV/CardDAV",
  "description": "Confidential client for DAV services (HTTP Basic → password grant).",
  "enabled": true,
  "protocol": "openid-connect",
  "publicClient": false,
  "standardFlowEnabled": false,
  "implicitFlowEnabled": false,
  "directAccessGrantsEnabled": true,
  "serviceAccountsEnabled": false,
  "authorizationServicesEnabled": false,
  "clientAuthenticatorType": "client-secret",
  "redirectUris": [],
  "webOrigins": [],
  "attributes": {
    "access.token.lifespan": "300"
  }
}
JSON
)

if [[ -z "$EXISTING_ID" ]]; then
  echo "Creating client ${KC_CLIENT}..." >&2
  auth -X POST "${KC_URL}/admin/realms/${KC_REALM}/clients" \
    -H "Content-Type: application/json" \
    -d "$CLIENT_JSON" >/dev/null
  LIST=$(auth "${KC_URL}/admin/realms/${KC_REALM}/clients?clientId=${KC_CLIENT}")
  EXISTING_ID=$(echo "$LIST" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
  [[ -n "$EXISTING_ID" ]] || err "client creation failed"
else
  echo "Updating client ${KC_CLIENT} (id=${EXISTING_ID})..." >&2
  auth -X PUT "${KC_URL}/admin/realms/${KC_REALM}/clients/${EXISTING_ID}" \
    -H "Content-Type: application/json" \
    -d "$CLIENT_JSON" >/dev/null
fi

# ── 3) Retrieve (or regenerate) secret ──────────────────────
SECRET=$(auth "${KC_URL}/admin/realms/${KC_REALM}/clients/${EXISTING_ID}/client-secret" \
  | sed -n 's/.*"value":"\([^"]*\)".*/\1/p')

if [[ -z "$SECRET" ]]; then
  SECRET=$(auth -X POST "${KC_URL}/admin/realms/${KC_REALM}/clients/${EXISTING_ID}/client-secret" \
    | sed -n 's/.*"value":"\([^"]*\)".*/\1/p')
fi
[[ -n "$SECRET" ]] || err "no client secret returned"

echo "${KC_CLIENT}:${SECRET}"

#!/usr/bin/env bash
# Seed SuperAdmin user → Keycloak realm + DB (idempotent).
# → creates KC user with SuperAdmin role mapped, syncs tenants+users table so
#   users.id == KC sub (enables RLS/session identity). Safe to re-run.
# Prereqs: Keycloak 25+ @ $KC_URL, admin creds, Postgres reachable via $DB_*
# env (or skip DB sync by unsetting DB_HOST).
set -euo pipefail

KC_URL="${KC_URL:-http://192.168.15.125:8080}"
KC_ADMIN="${KC_ADMIN:-admin}"
KC_ADMIN_PASS="${KC_ADMIN_PASS:?set KC_ADMIN_PASS}"
REALM="${REALM:-expresso}"

SA_EMAIL="${SA_EMAIL:-admin@expresso.local}"
SA_USERNAME="${SA_USERNAME:-$SA_EMAIL}"
SA_PASS="${SA_PASS:?set SA_PASS}"
SA_FIRST="${SA_FIRST:-Super}"
SA_LAST="${SA_LAST:-Admin}"
# Tenant that SuperAdmin belongs to (for tenant_id claim). Defaults to `default`
# tenant id used in dev seed (matches migrations/20260417000001_core_schema.sql).
SA_TENANT_ID="${SA_TENANT_ID:-91f1b947-f495-4071-bee4-d87d705e7698}"
SA_TENANT_SLUG="${SA_TENANT_SLUG:-default}"
SA_TENANT_NAME="${SA_TENANT_NAME:-Default}"

DB_HOST="${DB_HOST:-}"
DB_PORT="${DB_PORT:-5432}"
DB_USER="${DB_USER:-expresso}"
DB_PASS="${DB_PASS:-}"
DB_NAME="${DB_NAME:-expresso}"

_token() {
  curl -sf -X POST "$KC_URL/realms/master/protocol/openid-connect/token" \
    -d grant_type=password -d client_id=admin-cli \
    -d "username=$KC_ADMIN" -d "password=$KC_ADMIN_PASS" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["access_token"])'
}

TOKEN=$(_token)
H=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json")

# 1. ensure SuperAdmin realm role (seed-realm.sh may have done this; idempotent)
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/roles" \
  -d '{"name":"SuperAdmin","description":"Expresso SuperAdmin role"}' || true

# 2. upsert user (create; if exists, lookup id + reset pwd/attrs)
USER_JSON=$(cat <<JSON
{"username":"$SA_USERNAME","email":"$SA_EMAIL","enabled":true,"emailVerified":true,
 "firstName":"$SA_FIRST","lastName":"$SA_LAST",
 "attributes":{"tenant_id":["$SA_TENANT_ID"]},
 "credentials":[{"type":"password","value":"$SA_PASS","temporary":false}]}
JSON
)
CREATE_STATUS=$(curl -s -o /dev/null -w '%{http_code}' "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/users" -d "$USER_JSON")
echo "KC create user: $CREATE_STATUS (201=new, 409=exists)"

SA_EMAIL_ENC=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1],safe=''))" "$SA_EMAIL")
SA_USERNAME_ENC=$(python3 -c "import urllib.parse,sys;print(urllib.parse.quote(sys.argv[1],safe=''))" "$SA_USERNAME")
# Try email first (new installs); fallback to username (legacy users created w/o email attr).
USERID=$(curl -sf "${H[@]}" "$KC_URL/admin/realms/$REALM/users?email=$SA_EMAIL_ENC&exact=true" \
  | python3 -c 'import sys,json;u=json.load(sys.stdin);print(u[0]["id"] if u else "")')
if [[ -z "$USERID" ]]; then
  USERID=$(curl -sf "${H[@]}" "$KC_URL/admin/realms/$REALM/users?username=$SA_USERNAME_ENC&exact=true" \
    | python3 -c 'import sys,json;u=json.load(sys.stdin);print(u[0]["id"] if u else "")')
fi
[[ -z "$USERID" ]] && { echo "ERR: failed to locate KC user $SA_EMAIL"; exit 1; }
echo "KC user id: $USERID"

# 2b. ensure full profile (email + names + tenant_id attr) — legacy users may
# have been created with only username; this PUT completes the profile so that
# direct-access grants work (KC rejects login with "Account is not fully set up"
# otherwise).
UPDATE_JSON=$(cat <<JSON
{"email":"$SA_EMAIL","emailVerified":true,"enabled":true,
 "firstName":"$SA_FIRST","lastName":"$SA_LAST",
 "requiredActions":[],
 "attributes":{"tenant_id":["$SA_TENANT_ID"]}}
JSON
)
curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/users/$USERID" -d "$UPDATE_JSON" || true
curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/users/$USERID/reset-password" \
  -d "{\"type\":\"password\",\"value\":\"$SA_PASS\",\"temporary\":false}" || true

# 3. assign SuperAdmin role (idempotent)
ROLE=$(curl -sf "${H[@]}" "$KC_URL/admin/realms/$REALM/roles/SuperAdmin")
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/users/$USERID/role-mappings/realm" \
  -d "[$ROLE]" || true

# 4. DB sync (optional — skip if DB_HOST unset)
if [[ -z "$DB_HOST" ]]; then
  echo "SKIP: DB sync (set DB_HOST/DB_PASS to enable)"
  echo "OK: KC SuperAdmin seeded (id=$USERID email=$SA_EMAIL)"
  exit 0
fi
[[ -z "$DB_PASS" ]] && { echo "ERR: DB_HOST set but DB_PASS missing"; exit 1; }

# Use local psql if available, otherwise fall back to dockerized postgres:16-alpine.
if command -v psql >/dev/null 2>&1; then
  export PGPASSWORD="$DB_PASS"
  PSQL=(psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -v ON_ERROR_STOP=1 -Atq)
elif command -v docker >/dev/null 2>&1; then
  PSQL=(docker run --rm -i -e "PGPASSWORD=$DB_PASS" postgres:16-alpine
        psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -v ON_ERROR_STOP=1 -Atq)
else
  echo "ERR: neither psql nor docker available for DB sync"; exit 1
fi

# 4a. upsert tenant
"${PSQL[@]}" -c "
INSERT INTO tenants (id, slug, name, plan, status)
VALUES ('$SA_TENANT_ID', '$SA_TENANT_SLUG', '$SA_TENANT_NAME', 'enterprise', 'active')
ON CONFLICT (id) DO UPDATE SET
  slug = EXCLUDED.slug,
  name = EXCLUDED.name,
  status = 'active',
  updated_at = now();
" >/dev/null
echo "DB tenant upserted: $SA_TENANT_ID"

# 4b. upsert user — users.id MUST equal KC sub for session identity.
# When existing row has same email but different id, attempt re-link:
# switch FK-referencing rows to new id inside a single tx; if FK missing
# or tables not present yet, ON CONFLICT paths still succeed.
EXISTING_ID=$("${PSQL[@]}" -c "SELECT id::text FROM users WHERE email = '$SA_EMAIL';" || true)
EXISTING_ID="${EXISTING_ID:-}"
if [[ -n "$EXISTING_ID" && "$EXISTING_ID" != "$USERID" ]]; then
  cat >&2 <<WARN
WARN: DB users.id ($EXISTING_ID) != KC sub ($USERID) for $SA_EMAIL.
      RBAC via /auth/me roles continua funcionando; session identity diverge.
      Para re-link em deploy fresh: DELETE FROM users WHERE email='$SA_EMAIL'
      (cascata em mailboxes/calendars/etc), depois rerun este script.
      Prosseguindo com ON CONFLICT DO UPDATE (mantém id antigo).
WARN
fi

"${PSQL[@]}" -c "
INSERT INTO users (id, tenant_id, email, display_name, role, is_active)
VALUES ('$USERID', '$SA_TENANT_ID', '$SA_EMAIL', '$SA_FIRST $SA_LAST', 'super_admin', true)
ON CONFLICT (tenant_id, email) DO UPDATE SET
  display_name = EXCLUDED.display_name,
  role         = 'super_admin',
  is_active    = true,
  updated_at   = now();
" >/dev/null
echo "DB user upserted: id=$USERID email=$SA_EMAIL tenant=$SA_TENANT_ID"
echo "OK: SuperAdmin fully seeded (KC + DB)"

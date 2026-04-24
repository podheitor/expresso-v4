#!/bin/bash
# Add new tenant to multi-realm rollout (Sprint #45 + #46).
#
# Usage:
#   ops/tenant-add.sh <tenant-slug> <realm-uuid> [host-fqdn]
#
# Example:
#   ops/tenant-add.sh pilot3 3b11c7a2-xxxx-xxxx-xxxx-xxxxxxxxxxxx pilot3.expresso.local
#
# Prereqs (manual):
#   1. Keycloak realm created (master admin), UUID captured.
#   2. Users provisioned. Client `expresso-dav` (confidential, direct-access) with audience mapper.
#   3. DNS / /etc/hosts: host-fqdn → reverse-proxy IP.
#
# What this script does (prints; does NOT execute):
#   - Env snippets for AUTH__TENANT_HOSTS / smoke-multirealm / smoke-web.
#   - kcadm commands to provision `expresso-web` public client in new realm
#     with redirect_uri https://<host>/auth/callback (Sprint #46 requirement).
#   - systemd timer commands.
#
# Still manual AFTER running this:
#   - Apply env updates on prod 125 (compose-phase3.yaml, compose-mail.yaml, compose-chat-meet.yaml, expresso-mail.env, compose-auth-rp.yaml)
#   - `docker compose up -d --force-recreate` each affected stack (rolling)
#   - Install /etc/expresso/smoke-multirealm-<tenant>.env + smoke-web.env with real secrets
#   - Enable systemd timers
#   - Validate: ops/smoke-multirealm.sh + ops/smoke-web.sh PASS
set -euo pipefail
TENANT="${1:?tenant slug (e.g. pilot3)}"
REALM="${2:?realm UUID}"
HOST="${3:-${TENANT}.expresso.local}"

cat <<EOF
=========================================================================
TENANT ONBOARDING: ${TENANT}  realm=${REALM}  host=${HOST}
=========================================================================

--- 1) AUTH__TENANT_HOSTS append (comma-separated) ---
${HOST}:${REALM}

--- 2) Keycloak: provision \`expresso-web\` public client in realm ${REALM} ---
# Run on prod 125 (kcadm inside keycloak container):
docker exec -it expresso-keycloak /opt/keycloak/bin/kcadm.sh config credentials \\
  --server http://localhost:8080 --realm master --user admin --password "\$KC_ADMIN_PASS"

docker exec -it expresso-keycloak /opt/keycloak/bin/kcadm.sh create clients \\
  -r ${REALM} \\
  -s clientId=expresso-web \\
  -s enabled=true \\
  -s publicClient=true \\
  -s standardFlowEnabled=true \\
  -s directAccessGrantsEnabled=false \\
  -s 'redirectUris=["https://${HOST}/auth/callback"]' \\
  -s 'webOrigins=["https://${HOST}"]' \\
  -s 'attributes={"pkce.code.challenge.method":"S256","post.logout.redirect.uris":"https://${HOST}/"}'

# Verify:
docker exec -it expresso-keycloak /opt/keycloak/bin/kcadm.sh get clients \\
  -r ${REALM} -q clientId=expresso-web --fields clientId,redirectUris,webOrigins

--- 3) smoke-multirealm-${TENANT}.env template ---
PILOT_REALM=${REALM}
PILOT_CLIENT_ID=expresso-dav
PILOT_CLIENT_SECRET=<REPLACE_WITH_CLIENT_SECRET>
PILOT_USER=admin@${HOST}
PILOT_PASS=<REPLACE_WITH_ADMIN_PASSWORD>
TENANT_HOST=${HOST}
PUSHGATEWAY_URL=http://127.0.0.1:9091

--- 4) smoke-web.env append (global — one file, all tenants) ---
# /etc/expresso/smoke-web.env must include at minimum PILOT_REALM + PILOT2_REALM.
# New tenant: add a FALLBACK_HOST / FALLBACK_REALM rotation OR extend smoke-web.sh probe_tenant calls.

--- 5) systemd enable ---
sudo install -m 0600 /tmp/smoke-multirealm-${TENANT}.env /etc/expresso/
sudo systemctl enable --now expresso-smoke-dav@${TENANT}.timer
sudo systemctl enable --now expresso-smoke-web.timer

--- 6) validation ---
sudo env \$(sudo cat /etc/expresso/smoke-multirealm-${TENANT}.env | xargs) bash /opt/expresso/smoke-dav.sh
# expect: SMOKE PASS (7/7 probes)

sudo env \$(sudo cat /etc/expresso/smoke-web.env | xargs) bash /opt/expresso/smoke-web.sh
# expect: SMOKE WEB PASS

--- 7) services requiring AUTH__TENANT_HOSTS update ---
compose-phase3.yaml    → expresso-calendar, expresso-contacts, expresso-drive
compose-mail.yaml      → expresso-mail (OR expresso-mail.env)
compose-chat-meet.yaml → expresso-chat, expresso-meet
compose-auth-rp.yaml   → expresso-auth (UI login — Sprint #46)

After editing each file, run:
  sudo docker compose -f <file> up -d --force-recreate <service>
Verify log line: "multi-realm validator ready ... hosts: N" (N = total tenants)
Verify log line: "tenant provider cache ready" (auth-rp)
EOF

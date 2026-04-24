#!/bin/bash
# Add new tenant to multi-realm rollout.
#
# Usage:
#   ops/tenant-add.sh <tenant-slug> <realm-uuid> [host-fqdn]
#
# Example:
#   ops/tenant-add.sh pilot3 3b11c7a2-xxxx-xxxx-xxxx-xxxxxxxxxxxx pilot3.expresso.local
#
# Prereqs (manual, not automated):
#   1. Keycloak realm created (master admin), UUID captured.
#   2. Users + client `expresso-dav` (confidential, direct-access) provisioned.
#      Add audience mapper emitting `aud=account` for API compat.
#   3. DNS entry / /etc/hosts: host-fqdn → reverse-proxy IP.
#
# What this script does:
#   - Prints diff-ready env snippets to append to each compose/env file on prod.
#   - Generates smoke-multirealm-<tenant>.env template for systemd timer instance.
#   - Emits systemd `systemctl enable --now expresso-smoke-dav@<tenant>.timer` command.
#
# Still manual AFTER running this:
#   - Apply env updates on prod 125 (compose-phase3.yaml, compose-mail.yaml, compose-chat-meet.yaml, expresso-mail.env)
#   - `docker compose up -d --force-recreate` each affected stack (rolling)
#   - Install /etc/expresso/smoke-multirealm-<tenant>.env with real secret
#   - Enable systemd timer
#   - Run `sudo env $(sudo cat /etc/expresso/smoke-multirealm-<tenant>.env | xargs) bash /opt/expresso/smoke-dav.sh` → expect SMOKE PASS
set -euo pipefail
TENANT="${1:?tenant slug (e.g. pilot3)}"
REALM="${2:?realm UUID}"
HOST="${3:-${TENANT}.expresso.local}"

cat <<EOF
=== AUTH__TENANT_HOSTS append (comma-separated) ===
${HOST}:${REALM}

=== smoke-multirealm-${TENANT}.env template ===
PILOT_REALM=${REALM}
PILOT_CLIENT_ID=expresso-dav
PILOT_CLIENT_SECRET=<REPLACE_WITH_CLIENT_SECRET>
PILOT_USER=admin@${HOST}
PILOT_PASS=<REPLACE_WITH_ADMIN_PASSWORD>
TENANT_HOST=${HOST}
PUSHGATEWAY_URL=http://127.0.0.1:9091

=== systemd enable ===
sudo install -m 0600 /tmp/smoke-multirealm-${TENANT}.env /etc/expresso/
sudo systemctl enable --now expresso-smoke-dav@${TENANT}.timer

=== validation ===
sudo env \$(sudo cat /etc/expresso/smoke-multirealm-${TENANT}.env | xargs) bash /opt/expresso/smoke-dav.sh
# expect: SMOKE PASS (7/7 probes)

=== services requiring AUTH__TENANT_HOSTS update ===
compose-phase3.yaml    → expresso-calendar, expresso-contacts, expresso-drive
compose-mail.yaml      → expresso-mail (OR expresso-mail.env)
compose-chat-meet.yaml → expresso-chat, expresso-meet

After editing each file, run:
  sudo docker compose -f <file> up -d --force-recreate <service>
Verify log line: "multi-realm validator ready ... hosts: N" (N = total tenants)
EOF

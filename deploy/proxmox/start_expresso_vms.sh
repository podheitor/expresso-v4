#!/usr/bin/env bash
set -euo pipefail

PROXMOX_HOST="${PROXMOX_HOST:-192.168.194.101}"
PROXMOX_USER="${PROXMOX_USER:-root}"
PROXMOX_PASS="${PROXMOX_PASS:-}"
PROXMOX_SSH_OPTS="-o StrictHostKeyChecking=no -o ConnectTimeout=8"
WAIT_SECONDS="${WAIT_SECONDS:-20}"

if [[ -z "${PROXMOX_PASS}" ]]; then
  echo "error: set PROXMOX_PASS environment variable" >&2
  exit 1
fi

remote_script=$(cat <<'REMOTE'
set -euo pipefail
mapfile -t ids < <(qm list | awk 'NR>1 && $2 ~ /^expresso-/ {print $1}')
for id in "${ids[@]}"; do
  qm start "$id" >/dev/null 2>&1 || true
  echo "started-or-running:${id}"
done
sleep "${WAIT_SECONDS}"
for id in "${ids[@]}"; do
  qm status "$id"
done
REMOTE
)

sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" "WAIT_SECONDS='${WAIT_SECONDS}' bash -s" <<< "$remote_script"

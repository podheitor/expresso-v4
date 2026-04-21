#!/usr/bin/env bash
set -euo pipefail

PROXMOX_HOST="${PROXMOX_HOST:-192.168.194.101}"
PROXMOX_USER="${PROXMOX_USER:-root}"
PROXMOX_PASS="${PROXMOX_PASS:-}"
VM_USER="${VM_USER:-debian}"
VM_PASS="${VM_PASS:-}"
PROXMOX_SSH_OPTS="-o StrictHostKeyChecking=no -o ConnectTimeout=8"

if [[ -z "${PROXMOX_PASS}" ]]; then
  echo "error: set PROXMOX_PASS environment variable" >&2
  exit 1
fi

if [[ -z "${VM_PASS}" ]]; then
  echo "error: set VM_PASS environment variable" >&2
  exit 1
fi

ips=(
  192.168.15.122
  192.168.15.123
  192.168.15.124
  192.168.15.125
  192.168.15.126
)

for ip in "${ips[@]}"; do
  echo "[base] ${ip}"
  sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" \
    "sshpass -p '${VM_PASS}' ssh -o StrictHostKeyChecking=no -o ConnectTimeout=5 ${VM_USER}@${ip} \"echo ${VM_PASS} | sudo -S bash -lc 'apt-get update -y >/dev/null && apt-get install -y qemu-guest-agent ca-certificates curl git >/dev/null && systemctl restart qemu-guest-agent'\""
done

echo "done"

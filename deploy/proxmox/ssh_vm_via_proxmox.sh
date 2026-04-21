#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "usage: $0 <vm-ip> <command...>" >&2
  exit 1
fi

VM_IP="$1"
shift
CMD="$*"

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

sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" \
  "sshpass -p '${VM_PASS}' ssh -o StrictHostKeyChecking=no -o ConnectTimeout=5 ${VM_USER}@${VM_IP} \"${CMD}\""

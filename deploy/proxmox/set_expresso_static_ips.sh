#!/usr/bin/env bash
set -euo pipefail

PROXMOX_HOST="${PROXMOX_HOST:-192.168.194.101}"
PROXMOX_USER="${PROXMOX_USER:-root}"
PROXMOX_PASS="${PROXMOX_PASS:-}"
PROXMOX_SSH_OPTS="-o StrictHostKeyChecking=no -o ConnectTimeout=8"
GATEWAY="${GATEWAY:-192.168.15.1}"
CIDR="${CIDR:-24}"

if [[ -z "${PROXMOX_PASS}" ]]; then
  echo "error: set PROXMOX_PASS environment variable" >&2
  exit 1
fi

# VMID:IP mapping (adjust if needed)
declare -A vm_ips=(
  [122]="192.168.15.122"
  [123]="192.168.15.123"
  [124]="192.168.15.124"
  [125]="192.168.15.125"
  [126]="192.168.15.126"
)

for vmid in "${!vm_ips[@]}"; do
  ip="${vm_ips[$vmid]}"
  echo "setting vm ${vmid} -> ${ip}/${CIDR} gw ${GATEWAY}"
  sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" \
    "qm set ${vmid} --ipconfig0 ip=${ip}/${CIDR},gw=${GATEWAY} >/dev/null && qm cloudinit update ${vmid}"
done

echo "done: reboot VMs or run deploy/proxmox/start_expresso_vms.sh"

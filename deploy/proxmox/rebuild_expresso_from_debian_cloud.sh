#!/usr/bin/env bash
set -euo pipefail

PROXMOX_HOST="${PROXMOX_HOST:-192.168.194.101}"
PROXMOX_USER="${PROXMOX_USER:-root}"
PROXMOX_PASS="${PROXMOX_PASS:-}"
PROXMOX_SSH_OPTS="-o StrictHostKeyChecking=no -o ConnectTimeout=8"
DEBIAN_IMAGE_URL="${DEBIAN_IMAGE_URL:-https://cloud.debian.org/images/cloud/bookworm/latest/debian-12-genericcloud-amd64.qcow2}"
DEBIAN_IMAGE_PATH="${DEBIAN_IMAGE_PATH:-/var/lib/vz/template/iso/debian-12-genericcloud-amd64.qcow2}"
CI_USER="${CI_USER:-debian}"
CI_PASSWORD="${CI_PASSWORD:-}"
GATEWAY="${GATEWAY:-192.168.15.1}"
CIDR="${CIDR:-24}"

if [[ -z "${PROXMOX_PASS}" ]]; then
  echo "error: set PROXMOX_PASS environment variable" >&2
  exit 1
fi

if [[ -z "${CI_PASSWORD}" ]]; then
  echo "error: set CI_PASSWORD environment variable" >&2
  exit 1
fi

# VMID:IP:DISK_SIZE
entries=(
  "123:192.168.15.123:132G"
  "124:192.168.15.124:482G"
  "125:192.168.15.125:82G"
  "126:192.168.15.126:32G"
)

sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" \
  "test -f '${DEBIAN_IMAGE_PATH}' || wget -q -O '${DEBIAN_IMAGE_PATH}' '${DEBIAN_IMAGE_URL}'"

for row in "${entries[@]}"; do
  IFS=':' read -r vmid ip disk_size <<<"$row"
  echo "[rebuild] vm=${vmid} ip=${ip} disk=${disk_size}"

  sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" "
    set -e
    qm stop ${vmid} --skiplock >/dev/null 2>&1 || true
    qm set ${vmid} --delete scsi0 >/dev/null 2>&1 || true
    qm set ${vmid} --scsi0 local-lvm:0,import-from=${DEBIAN_IMAGE_PATH} >/dev/null
    qm resize ${vmid} scsi0 ${disk_size} >/dev/null
    qm set ${vmid} --boot order=scsi0 >/dev/null
    qm config ${vmid} | grep -q '^ide2:' || qm set ${vmid} --ide2 local-lvm:cloudinit >/dev/null
    qm set ${vmid} --agent enabled=1 >/dev/null
    qm set ${vmid} --serial0 socket --vga serial0 >/dev/null
    qm set ${vmid} --ciuser '${CI_USER}' --cipassword '${CI_PASSWORD}' >/dev/null
    qm set ${vmid} --ipconfig0 ip=${ip}/${CIDR},gw=${GATEWAY} >/dev/null
    qm cloudinit update ${vmid} >/dev/null
    qm start ${vmid} >/dev/null
  "
done

echo "waiting for boot..."
sleep 35

sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" '
for id in 122 123 124 125 126; do
  echo "===== ${id} ====="
  qm status "$id"
  qm config "$id" | egrep "^(name|boot|scsi0|ipconfig0|ciuser|agent):"
done
'

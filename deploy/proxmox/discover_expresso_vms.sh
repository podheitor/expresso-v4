#!/usr/bin/env bash
set -euo pipefail

PROXMOX_HOST="${PROXMOX_HOST:-192.168.194.101}"
PROXMOX_USER="${PROXMOX_USER:-root}"
PROXMOX_PASS="${PROXMOX_PASS:-}"
PROXMOX_SSH_OPTS="-o StrictHostKeyChecking=no -o ConnectTimeout=8"

if [[ -z "${PROXMOX_PASS}" ]]; then
  echo "error: set PROXMOX_PASS environment variable" >&2
  exit 1
fi

remote_script='set -euo pipefail

echo "VMID|NAME|STATUS|MAC|IP_AGENT|IP_ARP"

mapfile -t rows < <(qm list | awk "NR>1 && \$2 ~ /^expresso-/ {print \$1\"|\"\$2}")

for row in "${rows[@]}"; do
  vmid="${row%%|*}"
  name="${row#*|}"

  status=$(qm status "$vmid" | awk "{print \$2}")

  netline=$(qm config "$vmid" | sed -n "s/^net0: //p")
  mac=$(printf "%s" "$netline" | sed -n "s/.*virtio=\([^,]*\).*/\1/p" | tr "A-F" "a-f")

  ip_agent=""
  if qm guest cmd "$vmid" network-get-interfaces >/tmp/qga-${vmid}.json 2>/dev/null; then
    if command -v jq >/dev/null 2>&1; then
      ip_agent=$(jq -r "[.[]?.\"ip-addresses\"[]? | select(.\"ip-address-type\"==\"ipv4\") | .\"ip-address\"] | map(select(startswith(\"127.\")|not)) | .[0] // \"\"" /tmp/qga-${vmid}.json)
    fi
  fi

  ip_arp=""
  if [[ -n "$mac" ]]; then
    ip_arp=$(ip neigh show dev vmbr0 | awk -v m="$mac" "tolower(\$0) ~ m {print \$1; exit}")
  fi

  echo "${vmid}|${name}|${status}|${mac}|${ip_agent}|${ip_arp}"
done
'

sshpass -p "$PROXMOX_PASS" ssh $PROXMOX_SSH_OPTS "${PROXMOX_USER}@${PROXMOX_HOST}" "$remote_script"

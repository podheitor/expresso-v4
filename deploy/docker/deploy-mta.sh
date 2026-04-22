#!/bin/bash
# Deploy MTA stack (Postfix + milter + expresso-mail LMTP) to VM
# Usage: ./deploy-mta.sh [--build-only|--deploy-only]
# Assumes: ~/expresso-v4/ on VM already has deploy/ subdir; source is rsynced fresh
set -euo pipefail

VM_HOST="${VM_HOST:-192.168.15.125}"
VM_USER="${VM_USER:-debian}"
JUMP_HOST="${JUMP_HOST:-192.168.194.101}"
JUMP_USER="${JUMP_USER:-root}"
MODE="${1:-all}"

say() { echo -e "\033[1;36m[deploy-mta]\033[0m $*"; }

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ "$MODE" != "--deploy-only" ]]; then
  say "Packaging source for build…"
  cd "$REPO_ROOT"
  tar --exclude=target --exclude=node_modules --exclude=.git \
      -czf /tmp/expresso-mta-src.tar.gz \
      Cargo.toml Cargo.lock \
      services/expresso-mail services/expresso-milter \
      libs/ migrations/ \
      Dockerfile.mail Dockerfile.milter deploy/postfix deploy/docker/compose-mta.yaml
  say "→ $(du -h /tmp/expresso-mta-src.tar.gz | cut -f1)"

  say "Shipping via jump host…"
  sshpass -p "${JUMP_PASS:?JUMP_PASS env}" scp -o StrictHostKeyChecking=no \
      /tmp/expresso-mta-src.tar.gz "${JUMP_USER}@${JUMP_HOST}:/tmp/"
  sshpass -p "${JUMP_PASS}" ssh -o StrictHostKeyChecking=no "${JUMP_USER}@${JUMP_HOST}" \
      "sshpass -p '${VM_PASS:?VM_PASS env}' scp -o StrictHostKeyChecking=no /tmp/expresso-mta-src.tar.gz ${VM_USER}@${VM_HOST}:/tmp/"

  say "Extracting + building on VM…"
  sshpass -p "${JUMP_PASS}" ssh "${JUMP_USER}@${JUMP_HOST}" \
    "sshpass -p '${VM_PASS}' ssh -o StrictHostKeyChecking=no ${VM_USER}@${VM_HOST} '
      set -e
      mkdir -p ~/expresso-v4-mta && cd ~/expresso-v4-mta && tar -xzf /tmp/expresso-mta-src.tar.gz
      cd ~/expresso-v4-mta
      docker build -t expresso-mail:mta    -f Dockerfile.mail    .
      docker build -t expresso-milter:latest -f Dockerfile.milter .
      docker build -t expresso-postfix:latest deploy/postfix
    '"
fi

if [[ "$MODE" != "--build-only" ]]; then
  say "Copying compose-mta.yaml…"
  sshpass -p "${JUMP_PASS}" ssh "${JUMP_USER}@${JUMP_HOST}" \
    "sshpass -p '${VM_PASS}' scp -o StrictHostKeyChecking=no ~/expresso-v4-mta/deploy/docker/compose-mta.yaml ${VM_USER}@${VM_HOST}:~/expresso-v4/deploy/docker/"

  say "Deploying stack…"
  sshpass -p "${JUMP_PASS}" ssh "${JUMP_USER}@${JUMP_HOST}" \
    "sshpass -p '${VM_PASS}' ssh ${VM_USER}@${VM_HOST} '
      cd ~/expresso-v4/deploy/docker
      docker compose -f compose-mta.yaml up -d
      sleep 3
      docker ps --format \"{{.Names}}\t{{.Status}}\" | grep -E \"postfix|milter|mail\"
    '"
fi

say "Done."

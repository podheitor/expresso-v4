#!/usr/bin/env bash
# NATS JetStream tail tool — Expresso v4.
#
# Subscribes to a subject pattern and prints messages as they arrive.
# Useful for live debugging of event publishers + validating event payloads.
#
# Usage: ops/nats/tail.sh [NATS_URL] [SUBJECT]
#   NATS_URL   default: nats://localhost:4222
#   SUBJECT    default: expresso.>
#
# Uses natsio/nats-box:latest (pulled automatically) so no local deps needed.
#
# Ctrl-C to stop.

set -euo pipefail

NATS_URL="${1:-${NATS_URL:-nats://localhost:4222}}"
SUBJECT="${2:-${NATS_SUBJECT:-expresso.>}}"

echo ">> tailing ${SUBJECT} on ${NATS_URL}"
echo ">> (Ctrl-C to stop)"
echo

exec docker run --rm -i --network host natsio/nats-box:latest \
    nats --server="$NATS_URL" sub "$SUBJECT"

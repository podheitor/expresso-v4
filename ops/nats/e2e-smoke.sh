#!/usr/bin/env bash
# End-to-end NATS JetStream smoke — Expresso v4.
#
# Proves chain: service write -> JetStream publish.
#
# Usage: ops/nats/e2e-smoke.sh <MON_URL> <STREAM> <TRIGGER_CMD>
#   1. Reads current 'messages' count of STREAM.
#   2. Executes TRIGGER_CMD (should perform a write that publishes).
#   3. Re-reads count. Exit 0 iff count increased.

set -euo pipefail

MON_URL="${1:-http://localhost:8222}"
STREAM="${2:-EXPRESSO_CALENDAR}"
TRIGGER="${3:-}"

if [[ -z "$TRIGGER" ]]; then
    echo "usage: $0 <MON_URL> <STREAM> <TRIGGER_CMD>" >&2
    exit 2
fi

count() {
    curl -fsS "${MON_URL}/jsz?streams=1" | python3 -c "
import json, os, sys
d = json.load(sys.stdin)
stream = os.environ['STREAM_NAME']
for a in d.get('account_details', []):
    for s in a.get('stream_detail', []):
        if s['name'] == stream:
            print(s['state']['messages'])
            sys.exit(0)
sys.exit(1)
"
}

export STREAM_NAME="$STREAM"

BEFORE="$(count)" || { echo "ERROR: stream '$STREAM' missing" >&2; exit 1; }
echo ">> before: $BEFORE messages"

echo ">> trigger: $TRIGGER"
eval "$TRIGGER"

# Allow async publish to settle.
sleep 2

AFTER="$(count)"
echo ">> after:  $AFTER messages"

if [[ "$AFTER" -gt "$BEFORE" ]]; then
    echo "OK: +$((AFTER - BEFORE)) messages"
    exit 0
else
    echo "FAIL: count did not increase" >&2
    exit 1
fi

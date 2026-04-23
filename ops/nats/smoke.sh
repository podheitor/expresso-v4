#!/usr/bin/env bash
# NATS JetStream smoke tool — Expresso v4.
#
# Usage: ops/nats/smoke.sh [NATS_MON_URL] [STREAM]
#   NATS_MON_URL   monitoring endpoint (default: http://localhost:8222)
#   STREAM         JetStream stream name  (default: EXPRESSO_CALENDAR)
#
# Behavior:
#   1. Queries /jsz?streams=1 and verifies the named stream exists.
#   2. Prints stream stats (messages, bytes, consumers).
#   3. Exits 0 on success, 1 missing stream, 2 unreachable endpoint.
#
# Requires: curl + python3.

set -euo pipefail

MON_URL="${1:-${NATS_MON_URL:-http://localhost:8222}}"
STREAM="${2:-${EXPRESSO_STREAM:-EXPRESSO_CALENDAR}}"

echo ">> GET ${MON_URL}/jsz?streams=1"
if ! JSON="$(curl -fsS "${MON_URL}/jsz?streams=1")"; then
    echo "ERROR: NATS monitoring endpoint unreachable at ${MON_URL}" >&2
    exit 2
fi

export EXPRESSO_NATS_JSON="$JSON" EXPRESSO_STREAM_CHECK="$STREAM"
python3 <<'PY'
import json, os, sys
data = json.loads(os.environ["EXPRESSO_NATS_JSON"])
stream = os.environ["EXPRESSO_STREAM_CHECK"]
found = None
for acc in data.get("account_details", []):
    for s in acc.get("stream_detail", []):
        if s.get("name") == stream:
            found = s
            break
if not found:
    print(f"FAIL: stream '{stream}' NOT found in JetStream.", file=sys.stderr)
    sys.exit(1)
st = found.get("state", {})
print(f"OK: stream '{stream}' present.")
print(f"  messages : {st.get('messages', 0)}")
print(f"  bytes    : {st.get('bytes', 0)}")
print(f"  consumers: {found.get('consumer_count', 0)}")
PY

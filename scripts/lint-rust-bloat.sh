#!/usr/bin/env bash
# lint-rust-bloat.sh — guard against the "analytics endpoints sprint" flood that
# once grew single .rs files to 350k lines of near-duplicate generated handlers,
# each wired to one nonsense REST route.
#
# Blocking checks:
#   1. No .rs file exceeds MAX_LINES.
#   2. No file registers more than MAX_ROUTES axum `.route(` calls (a real API
#      surface is dozens, not thousands — thousands means generated bloat).
#
# Usage: ./scripts/lint-rust-bloat.sh
set -euo pipefail

# Thresholds sized to allow the largest legitimate API file (calendar events.rs
# ~9k lines / 98 routes) while still catching the generated flood (350k / 15k).
MAX_LINES="${MAX_LINES:-12000}"
MAX_ROUTES="${MAX_ROUTES:-300}"
fail=0

while IFS= read -r f; do
    n=$(wc -l < "$f")
    if (( n > MAX_LINES )); then
        printf '✘ %s: %d lines (max %d)\n' "$f" "$n" "$MAX_LINES"
        fail=1
    fi
    r=$(grep -cE '^\s*\.route\(' "$f" || true)
    if (( r > MAX_ROUTES )); then
        printf '✘ %s: %d .route() registrations (max %d) — generated-handler bloat?\n' "$f" "$r" "$MAX_ROUTES"
        fail=1
    fi
done < <(find services libs -name '*.rs' -not -path '*/target/*')

(( fail == 0 )) && echo "✔ rust sources clean (≤${MAX_LINES} lines, ≤${MAX_ROUTES} routes/file)"
exit "$fail"

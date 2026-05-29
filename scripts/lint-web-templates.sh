#!/usr/bin/env bash
# lint-web-templates.sh — guard expresso-web templates against the dead-code
# bloat that once grew them to ~15k lines each: hundreds of unreferenced
# `window.X = function(){...}` globals appended sprint-by-sprint.
#
# Two blocking checks:
#   1. No template exceeds MAX_LINES.
#   2. No `window.X = function` whose `X` is never called (`X(`) anywhere.
#
# Usage: ./scripts/lint-web-templates.sh
set -euo pipefail

TPL_DIR="services/expresso-web/templates"
SRC_ROOT="services/expresso-web"
MAX_LINES="${MAX_LINES:-3000}"
fail=0

[[ -d "$TPL_DIR" ]] || { echo "no $TPL_DIR — skipping"; exit 0; }

# ── 1. line-count budget ──────────────────────────────────────────────────────
while IFS= read -r f; do
    n=$(wc -l < "$f")
    if (( n > MAX_LINES )); then
        printf '✘ %s: %d lines (max %d)\n' "$f" "$n" "$MAX_LINES"
        fail=1
    fi
done < <(find "$TPL_DIR" -name '*.html')

# ── 2. unreferenced window.* function globals ─────────────────────────────────
# Names invoked as `X(` anywhere under the web service (onclick, JS, etc.),
# excluding the definition line itself.
called=$(grep -rhoE '[^A-Za-z0-9_.]([A-Za-z0-9_]+)\(' "$SRC_ROOT" \
            --include='*.html' --include='*.js' --include='*.ts' \
         | grep -oE '[A-Za-z0-9_]+\(' | sed 's/($//;s/(//' | sort -u)

dead=0
while IFS= read -r name; do
    [[ -z "$name" ]] && continue
    if ! grep -qxF "$name" <<<"$called"; then
        (( dead++ ))
        (( dead <= 10 )) && printf '✘ dead global: window.%s (defined, never called)\n' "$name"
    fi
done < <(grep -rhoE '^window\.([A-Za-z0-9_]+)\s*=\s*function' "$TPL_DIR"/*.html \
            | sed -E 's/^window\.([A-Za-z0-9_]+).*/\1/' | sort -u)

if (( dead > 0 )); then
    printf '✘ %d unreferenced window.* function globals in templates\n' "$dead"
    fail=1
fi

if (( fail == 0 )); then
    echo "✔ web templates clean (≤${MAX_LINES} lines, no dead globals)"
fi
exit "$fail"

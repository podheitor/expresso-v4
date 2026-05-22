#!/usr/bin/env bash
# Expresso v4 — quality gate wrapper.
#
# Modes:
#   --ci    blocking gate (fmt, clippy, tests, deny, machete, lizard, jscpd)
#   --full  informational additions (typos, tarpaulin, outdated, geiger, scc, careful)
#   --fix   autofix (fmt, clippy --fix, typos --write-changes)
#
# Usage: ./scripts/quality-check.sh [--ci|--full|--fix]
set -euo pipefail

MODE="${1:---ci}"
PASS=0
FAIL=0

# ── helpers ──────────────────────────────────────────────────────────────────

green() { printf '\033[32m✔ %s\033[0m\n' "$*"; }
red()   { printf '\033[31m✘ %s\033[0m\n' "$*"; }
blue()  { printf '\033[34m» %s\033[0m\n' "$*"; }

run() {
    local label="$1"; shift
    blue "$label"
    if "$@"; then
        green "$label"
        (( PASS++ )) || true
    else
        red "$label"
        (( FAIL++ )) || true
        [[ "$MODE" == "--full" ]] || return 0   # full = informational, keep going
    fi
}

require() {
    local cmd="$1"
    if ! command -v "$cmd" &>/dev/null; then
        printf '\033[33m⚠ %s not found — skipping (install: %s)\033[0m\n' "$cmd" "${2:-see CLAUDE.md}"
        return 1
    fi
    return 0
}

# ── CI gate ───────────────────────────────────────────────────────────────────

ci_gate() {
    run "cargo fmt --check"           cargo fmt --check
    run "cargo clippy -D warnings"    cargo clippy --workspace --all-targets --all-features -- -D warnings
    run "cargo test"                  cargo test --workspace
    run "cargo deny check"            cargo deny check bans sources advisories licenses

    if require cargo-machete "cargo install cargo-machete"; then
        run "cargo-machete"           cargo-machete
    fi

    if require lizard "pipx install lizard"; then
        # CCN threshold 25 per function; -w = exit non-zero on violations
        run "lizard CCN≤25"           lizard src/ -l rust -C 25 -w 2>/dev/null || \
            find . -path '*/src/*.rs' -not -path '*/target/*' -print0 \
                | xargs -0 lizard -l rust -C 25 -w
    fi

    if require jscpd "npm install -g jscpd"; then
        run "jscpd dup<5%"            jscpd \
            --min-lines 50 --min-tokens 100 --threshold 5 \
            --reporters console --silent \
            services/ libs/
    fi
}

# ── Full (informational) ──────────────────────────────────────────────────────

full_gate() {
    ci_gate

    if require typos "cargo install typos-cli"; then
        run "typos"                   typos services/ libs/
    fi
    if require cargo-tarpaulin "cargo install cargo-tarpaulin"; then
        run "tarpaulin --lib"         cargo tarpaulin --lib --skip-clean --out Stdout
    fi
    if require cargo-careful "cargo +nightly install cargo-careful"; then
        run "cargo careful test"      cargo +nightly careful test --lib --workspace 2>/dev/null || true
    fi
    if require cargo-outdated "cargo install cargo-outdated"; then
        run "cargo outdated"          cargo outdated
    fi
    if require cargo-geiger "cargo install cargo-geiger"; then
        run "cargo geiger"            cargo geiger --quiet 2>/dev/null || true
    fi
    if require scc "go install github.com/boyter/scc/v3@latest 2>/dev/null"; then
        run "scc complexity"          scc --by-file --sort complexity services/ libs/
    fi
}

# ── Fix ───────────────────────────────────────────────────────────────────────

fix_gate() {
    blue "Autofixing…"
    cargo fmt
    cargo clippy --workspace --all-targets --all-features --fix --allow-dirty --allow-staged
    if require typos "cargo install typos-cli"; then
        typos --write-changes services/ libs/ || true
    fi
    green "Fix done — re-run --ci to verify"
}

# ── Dispatch ─────────────────────────────────────────────────────────────────

case "$MODE" in
    --ci)   ci_gate   ;;
    --full) full_gate ;;
    --fix)  fix_gate  ;;
    *)
        printf 'Usage: %s [--ci|--full|--fix]\n' "$0" >&2
        exit 1
        ;;
esac

printf '\n%s passed, %s failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]

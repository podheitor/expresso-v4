# User Preferences

## Language
- Communicate with me in concise English (chat replies included).
- Code, identifiers, comments, commits, and internal technical notes stay in English.
- User-facing UI text and end-user documentation are English (note: the existing Expresso v4 UI strings are Portuguese — match the surrounding language when editing existing templates; new standalone surfaces default to English unless the file's siblings are Portuguese).

## Workflow (what diverges from defaults)
- Target 200–300 line files; justify going over 500.
- Target 4–20 logical-line functions.
- Avoid vague names: `data`, `manager`, `helper`, `util`, `process` — prefer grep-friendly specifics.
- Type hints on every Python signature. Explicit types on Rust public APIs.
- Warnings = failures. Prefer the project's one-command validation when it exists.

## Default context
Trabalho principal é uma suite alternativa ao Microsoft Office 365. Detalhes de stack ficam no `CLAUDE.md` de cada projeto. Caso o projeto não tenha um, assumir esse perfil.

## When uncertain
- Pick the simpler approach.
- Never invent APIs — read the code or docs first.
- Ask before acting when the blast radius is wide, the requirement is ambiguous, or multiple valid paths exist.

## Git / commits
- Never add `Co-Authored-By: Claude ...` (or any other AI-assistant attribution) to commit messages, PR descriptions, or tags. Author them as if written solely by the user.
- The same rule applies to `🤖 Generated with …` footers and equivalent markers — omit them.

## Rust quality gate (reusable across projects)

Every Rust project I maintain should ship with a `scripts/quality-check.sh` that wraps the tools below. Modes: `--ci` (exact gate CI runs, blocking) / `--full` (adds slower informational checks) / `--fix` (autofix what's autofixable).

### Blocking gate (runs in `--ci`)

**Correctness:**
- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings` — pedantic + restriction lints configured in `Cargo.toml`
- `cargo test` (or `cargo nextest run` — ~3–4× faster, drop-in)

**Dependency hygiene:**
- `cargo deny check` with a `deny.toml` (advisories + licenses + bans + sources)
- `cargo-machete` — unused dependencies

**Complexity & duplication** (the two most-forgotten gates):
- `lizard src/ -l rust -C <CCN_THRESHOLD> -w` — per-function cyclomatic complexity. Exits non-zero if any function exceeds threshold. Default target: **CCN ≤ 25**; on legacy code start at the current max, tighten by 5 each refactor. Install: `pipx install lizard`.
- `jscpd --min-lines 50 --min-tokens 100 --threshold 5 --reporters console --silent src/` — duplicate-code detector. Fails if >5% of code is copy-paste in blocks ≥50 lines / ≥100 tokens (tuned to avoid GTK boilerplate false-positives). Install: `npm install -g jscpd`.

### Informational (`--full` only — slower, noisier)

- `typos src/` — spellcheck comments + strings. Install: `cargo install typos-cli`.
- `cargo tarpaulin --lib` — coverage.
- `cargo +nightly careful test` — extra UB detection (needs nightly toolchain).
- `cargo outdated --exit-code 1` — dependency freshness.
- `cargo geiger` — `unsafe` block counter (watch for silent growth in projects with FFI).
- `scc src/ --by-file --sort complexity` — LOC + CC summary per file (complements lizard's per-function view).
- `cargo mutants` — mutation testing (hours to run, noturno). Answers "do our tests actually catch bugs?".

### Project-specific lints

Add custom scripts for domain concerns that cargo lints can't express — and register them in `quality-check.sh` alongside the generic gates. Examples I've used:
- `lint-i18n.sh` — verifies every `i18n()` call-site is in `po/POTFILES.in` and no user-facing string is bare.
- `lint-a11y.sh` — checks icon-only buttons have `accessible::Property::Label` + tooltip.

### One-liner install (fresh dev box)

```bash
cargo install cargo-deny cargo-machete cargo-nextest typos-cli cargo-tarpaulin \
              cargo-careful cargo-outdated cargo-geiger cargo-mutants
pipx install lizard
npm install -g jscpd
```

### Clippy lints worth enabling in `Cargo.toml`

Every Rust project's `[lints.clippy]` table should at minimum include:

```toml
pedantic = { level = "warn", priority = -1 }
# Restriction lints (opt-in, high value):
dbg_macro = "warn"
todo = "warn"
rc_buffer = "warn"
rc_mutex = "warn"
verbose_file_reads = "warn"
unseparated_literal_suffix = "warn"
# FFI/unsafe-heavy codebases:
not_unsafe_ptr_arg_deref = "deny"
ptr_as_ptr = "warn"
```

And in `[lints.rust]`:

```toml
unsafe_op_in_unsafe_fn = "warn"
dead_code = "warn"
unused_imports = "warn"
unused_variables = "warn"
unused_mut = "warn"
unreachable_code = "warn"
unreachable_patterns = "warn"
trivial_numeric_casts = "warn"
```

## Rust security gate (reusable across projects)

Every Rust project I ship publicly should have the security layer below on top of the quality gate. Both the local `scripts/quality-check.sh --full` and GitHub Actions cover it; PRs block on the CI side, local runs are for dev feedback.

### Gates that run in CI (blocking)

On GitHub, split across three workflows:

- **`ci.yml`** — quality gate (fmt, clippy, tests, deny, machete, lizard, jscpd, custom lints).
- **`codeql.yml`** — `github/codeql-action` with `queries: +security-extended,security-and-quality`. Runs on push/PR + weekly cron. Free for public repos. Beta for Rust but picks up real CWEs.
- **`security.yml`** — three jobs in parallel:
  - `cargo audit` (RustSec DB, daily cron). Complements `cargo-deny advisories` because audit's DB is updated more aggressively.
  - `gitleaks/gitleaks-action@v2` with `fetch-depth: 0` so every historical commit is scanned.
  - `cargo +nightly miri test --lib <module::>...` on a curated list of FFI-free modules. The whole `--lib` typically can't run under Miri; pick pure-logic modules (keybindings, URL encoding, JSON parsers, etc.) and expand the list as FFI is removed.
- **`dependabot.yml`** — weekly cargo + github-actions updates; groups minor/patch updates to avoid PR noise.

### Gates for `scripts/quality-check.sh --full` (informational / nightly)

Expensive enough that they don't run per-PR, but a developer should run them before a release cut:

- **AddressSanitizer**: `RUSTFLAGS="-Zsanitizer=address" cargo +nightly test --lib --target <host> -Zbuild-std -- --test-threads=1`. The only gate that detects UB through `unsafe` FFI calls (libmpv, EGL, GL, dlsym) — Miri can't simulate C libraries, cargo-careful only catches stdlib issues.
- **`cargo fuzz` smoke**: build every target + `-max_total_time=5` per target. Catches harness regressions; real fuzzing happens overnight with `-max_total_time=14400`.
- **`cargo audit`** (redundant with the CI job but useful locally).
- **`gitleaks detect --no-banner --redact`** (defense-in-depth against accidental commits).
- Existing `--full` checks: `typos`, `cargo tarpaulin`, `cargo outdated`, `cargo geiger`, `cargo careful`, `scc`.

### Fuzzing setup (project side)

Standard layout for any project that parses untrusted input (media files, network protocols, config files):

1. `Cargo.toml` main crate: `tempfile = { version = "3", optional = true }` + `[features] fuzzing = ["dep:tempfile"]`.
2. `src/fuzz_entry.rs` feature-gated module with `pub fn fuzz_<target>(data: &[u8])` wrappers around internal parsers. For file-path APIs, write `data` to a `tempfile::NamedTempFile` and invoke the parser. For string APIs, `str::from_utf8` early-return on invalid UTF-8.
3. `fuzz/Cargo.toml` with empty `[workspace]` (isolates from the parent) + `big-media-player = { path = "..", features = ["fuzzing"] }`.
4. `fuzz/fuzz_targets/<target>.rs` — three lines each, just `fuzz_target!(|data: &[u8]| big_media_player::fuzz_entry::fuzz_<target>(data))`.
5. Internal parsers stay private — mark them `pub(crate)` so `fuzz_entry` can call them without leaking API surface.

### Pre-commit hook (optional)

`.pre-commit-config.yaml` with gitleaks + `cargo fmt --check` + `cargo clippy -D warnings`. Installed via `pip install pre-commit && pre-commit install`. CI already catches everything this would; the hook is just faster feedback.

### One-liner install (fresh dev box)

```bash
cargo install cargo-audit --locked
cargo install cargo-fuzz --locked
rustup toolchain install nightly
rustup +nightly component add miri rust-src
# gitleaks: see https://github.com/gitleaks/gitleaks (brew / apt / binary)
```

### Threat model doc

Every project with network I/O or untrusted-input parsers should ship `docs/SECURITY.md` covering (a) what runs where, (b) how to report vulnerabilities (prefer GitHub Security Advisories over public issues), (c) a bullet threat model naming each untrusted input and which gate covers it. Doesn't need to be long — one page is enough and is what auditors look for first.

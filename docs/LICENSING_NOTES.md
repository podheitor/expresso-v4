# Licensing notes — third-party copyleft dependencies

**Status:** informational. Expresso v4 is licensed under **GNU AGPL-3.0** (see
the repository `LICENSE` file). The copyleft dependencies below are therefore
**license-compatible by construction** — no conflict, no legal-review blocker.
The `deny.toml` exceptions exist only because the project's own workspace crates
don't yet carry SPDX `license` fields, so cargo-deny can't auto-derive that the
copyleft is compatible; the exceptions make that explicit rather than silent.

## `sieve-rs` 0.7.2 — AGPL-3.0-only

- **Used by:** `expresso-mail` (Sieve filter evaluation).
- **Compatibility:** the project is AGPL-3.0, so linking AGPL-3.0 `sieve-rs` is
  the natural, fully-compatible case — both halves carry the same network
  copyleft obligations. Nothing to mitigate.
- **Provenance:** `sieve-rs` is the Stalwart Labs Sieve interpreter
  (github.com/stalwartlabs/sieve). It is dual-licensed (AGPL-3.0 + a commercial
  option via `licensing@stalw.art`); we use the AGPL build, which matches our
  own license.

### Usage scope (audited 2026-05-30)

Recorded for maintenance reasons (not licensing) — the surface is small if we
ever need to swap engines for *technical* reasons:

- `services/expresso-mail/src/sieve.rs` — the only file that imports `sieve::*`
  (`Compiler`, `Runtime`, `Input`, `Event`, `Recipient`). Exposes a local
  `evaluate(script, raw) -> Vec<FilterAction>` and a `FilterAction` enum that
  the rest of the service depends on. ~115 LOC + ~200 LOC of tests.
- `services/expresso-mail/src/api/sieve.rs::validate_script` — one extra call:
  `sieve::Compiler::new().compile(bytes)` to validate a script on PUT.

`ingest.rs` and the REST handlers consume only `crate::sieve::{evaluate,
FilterAction}`, so the engine sits behind a stable wrapper seam. Subset actually
exercised: RFC 5228 `keep`, `fileinto`, `reject`, `discard`, `redirect`, plus
`header :contains`. `include`, `notify`, `duplicate`, `mailboxexists` /
`listcontains` are stubbed. Malformed input degrades to `Keep` (never panics —
covered by fuzz target `fuzz_sieve`).

## `indymilter` 0.3.0 — GPL-3.0-or-later

- **Used by:** `expresso-milter`.
- **Compatibility:** GPL-3.0 is compatible with the project's AGPL-3.0 license
  (AGPL-3.0 §13 explicitly permits combining AGPL and GPLv3 works). No issue.

## Permissive license added to the allow-list

- `webpki-roots` (CDLA-Permissive-2.0) — transitive via reqwest/aws-sdk/lettre.
  CDLA-Permissive-2.0 is a permissive data-license; added to `deny.toml`
  `licenses.allow` directly.

## Security advisories ignored in `deny.toml`

Several RUSTSEC advisories are `ignore`d because no upgrade is reachable from
our direct dependencies (they are pinned by `async-nats`, `aws-sdk`, and the
`hickory`/`opentelemetry` trees). See the inline comments in `deny.toml` for
the per-advisory rationale; none sit on this suite's primary attack surface.
Re-evaluate when the parent crates publish releases that pull patched
versions (`rustls-webpki ≥0.103.13`, `protobuf ≥3.7.2`, `hickory-proto ≥0.26.1`).

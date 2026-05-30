# Licensing notes — third-party copyleft dependencies

**Status:** flagged for legal review. The entries below are allowed in
`deny.toml` via scoped `exceptions` so CI reflects an explicit, reviewed
decision rather than a silent failure. They are **not** a legal sign-off.

Expresso v4 ships a proprietary (`LicenseRef-PodHeitor-Proprietary`) build.
Two transitive/direct dependencies carry strong copyleft licenses:

## `sieve-rs` 0.7.2 — AGPL-3.0-only (highest concern)

- **Used by:** `expresso-mail` (Sieve filter evaluation).
- **Why it matters:** AGPL-3.0 is the strongest copyleft — its §13 network
  clause can extend source-disclosure obligations to users interacting with
  the service over a network, not just to those who receive a binary. For a
  hosted multi-tenant mail service this is the material risk.
- **Options for legal/eng to weigh:**
  1. Confirm whether the AGPL boundary is acceptable for the deployment model
     (e.g. self-hosted-only customers vs. SaaS).
  2. Replace with a non-AGPL Sieve implementation, or reimplement the subset
     of RFC 5228 actually used.
  3. Obtain a commercial license from the `sieve-rs` author. **Confirmed
     available:** `sieve-rs` is the Stalwart Labs Sieve interpreter
     (github.com/stalwartlabs/sieve), dual-licensed — a commercial license that
     releases you from AGPLv3 obligations is offered via `licensing@stalw.art`.
     This is the lowest-engineering path if the price is acceptable; option 2
     (reimplement the contained subset) is the fallback if not.

### Usage scope (audited 2026-05-30) — relevant to option 2

The AGPL surface is **contained to two functions in two files**; every other
caller goes through our own wrapper, not the `sieve` crate:

- `services/expresso-mail/src/sieve.rs` — the only file that imports `sieve::*`
  (`Compiler`, `Runtime`, `Input`, `Event`, `Recipient`). It exposes a local
  `evaluate(script, raw) -> Vec<FilterAction>` and a `FilterAction` enum that
  the rest of the service depends on. ~115 LOC + ~200 LOC of tests.
- `services/expresso-mail/src/api/sieve.rs::validate_script` — one extra direct
  call: `sieve::Compiler::new().compile(bytes)` to validate a script on PUT.

`ingest.rs` and the REST handlers consume only `crate::sieve::{evaluate,
FilterAction}` — they are already insulated from the AGPL crate. So a
replacement only has to preserve that wrapper's public contract.

**Subset actually exercised** (what a reimplementation must cover): the RFC 5228
actions `keep`, `fileinto`, `reject`, `discard`, `redirect`, plus `header
:contains` tests (per the wrapper's tests). The wrapper already **stubs out**
`include`, `notify`, `duplicate`, and `mailboxexists`/`listcontains` (returns
unsupported/false), so those are out of scope. Compile errors and malformed
input degrade to `Keep` (never panic — covered by fuzz target `fuzz_sieve`).

**Cost read:** because the blast radius is one wrapper with a fixed public API
and a thorough test suite already in place, swapping the engine (to a
permissively-licensed Sieve crate, or a hand-rolled parser for the ~5-action
subset) is a **localized change behind a stable seam**, not a cross-cutting
refactor. The test suite in `sieve.rs` doubles as the acceptance spec for any
replacement.

## `indymilter` 0.3.0 — GPL-3.0-or-later (lower concern)

- **Used by:** `expresso-milter`.
- **Mitigating factor:** a milter communicates with the MTA over a local
  socket as a **separate process**; it is not linked into the proprietary
  binaries. Process separation is the standard argument that GPL obligations
  do not propagate across the IPC boundary — but this should be confirmed by
  counsel for the specific distribution model.

## Permissive license added to the allow-list

- `webpki-roots` (CDLA-Permissive-2.0) — transitive via reqwest/aws-sdk/lettre.
  CDLA-Permissive-2.0 is a permissive data-license; added to `deny.toml`
  `licenses.allow` directly (no review needed).

## Security advisories ignored in `deny.toml`

Several RUSTSEC advisories are `ignore`d because no upgrade is reachable from
our direct dependencies (they are pinned by `async-nats`, `aws-sdk`, and the
`hickory`/`opentelemetry` trees). See the inline comments in `deny.toml` for
the per-advisory rationale; none sit on this suite's primary attack surface.
Re-evaluate when the parent crates publish releases that pull patched
versions (`rustls-webpki ≥0.103.13`, `protobuf ≥3.7.2`, `hickory-proto ≥0.26.1`).

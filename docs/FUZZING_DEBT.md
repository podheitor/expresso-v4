# Fuzzing harness — lib targets added (pending green-CI confirmation)

**Status (2026-05-30):** the three services now have `src/lib.rs` exposing
`pub async fn run()` plus the module tree, and a `[lib]` target in each
`Cargo.toml`; `main.rs` is a thin shim calling `expresso_<svc>::run()`. The
fuzz crate should now link `expresso_{mail,calendar,contacts}::fuzz_entry`.
The `fuzz-smoke` job is kept `continue-on-error: true` for one CI run to
confirm the harness builds green; once observed green, drop that line to make
it a merge gate.

---

The `fuzz/` crate defines targets that call into the services:

```rust
expresso_mail::fuzz_entry::fuzz_imip(data);
expresso_calendar::fuzz_entry::fuzz_ical(data);
expresso_contacts::fuzz_entry::fuzz_vcard(data);
```

But `expresso-mail`, `expresso-calendar`, and `expresso-contacts` are
**binary-only crates** (`[[bin]]`, `src/main.rs`, no `[lib]`/`src/lib.rs`).
cargo cannot use a bin crate as a library dependency, so it emits:

```
warning: ignoring invalid dependency `expresso-mail` which is missing a lib target
error[E0433]: cannot find module or crate `expresso_mail` in this scope
```

i.e. the fuzz harness has **never compiled**. The `fuzz-smoke` CI job is
marked `continue-on-error: true` until this is fixed.

## To re-enable fuzzing

Give each of the three services a library target alongside the binary:

1. Add `src/lib.rs` exposing the modules the fuzz entry needs
   (`pub mod fuzz_entry;` gated on `#[cfg(feature = "fuzzing")]`, plus the
   `pub(crate)` parsers it wraps — `imip`, `sieve`, the iCal/vCard parsers).
2. Add a `[lib]` section (or rely on the default `src/lib.rs`) and keep the
   existing `[[bin]]` pointing at `src/main.rs`; have `main.rs` use the lib
   crate (`use expresso_mail::...`).
3. Verify `cd fuzz && cargo +nightly fuzz build` links all four targets.
4. Drop `continue-on-error` from the `fuzz-smoke` job.

The `fuzz_entry` modules and the `fuzzing` Cargo feature already exist in each
service; only the missing lib target blocks them.

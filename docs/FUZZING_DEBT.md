# Fuzzing harness — blocked on lib targets

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

# Fuzzing harness — fixed, now a blocking CI gate

**Status (2026-05-30):** the harness builds and runs; the `fuzz-smoke` job is a
blocking merge gate (verified green on commit 240e310d, `continue-on-error`
removed). It had **four** stacked blockers, fixed in order:

1. **Missing lib targets** — the three services were bin-only, so the fuzz crate
   couldn't link `expresso_{mail,calendar,contacts}::fuzz_entry`. Fix: each
   service got `src/lib.rs` (module tree + `pub async fn run()`) and a `[lib]`
   target; `main.rs` is a thin shim calling `expresso_<svc>::run()`.
2. **gitleaks false positive** — `let key = "MAIL_TEST_UNSET_19981"` test
   env-var NAMES tripped the default `generic-api-key` rule when they moved
   bin→lib. Fix: narrow `.gitleaks.toml` allowlist for the `_TEST_` convention.
3. **`cargo install cargo-fuzz --locked`** — the pinned `Cargo.lock` carries a
   `rustix` that fails on recent nightly (`rustc_attrs` reserved-attr errors).
   Fix: drop `--locked`; add `rust-src` component.
4. **cargo-fuzz invocation** — `fuzz/Cargo.toml` lacked
   `[package.metadata] cargo-fuzz = true`, and the CI step ran with
   `working-directory: fuzz` (cargo-fuzz then looked for `fuzz/fuzz/Cargo.toml`).
   Fix: add the marker; run `cargo +nightly fuzz build` from the **repo root**.

Verified on the build host (.105, nightly 1.98): `cargo fuzz build` from root
compiles all 4 targets under ASan + build-std (green, ~7m), and
`cargo fuzz run fuzz_vcard -max_total_time=5` does 295k runs, no crash.

The `fuzz-smoke` job is now a blocking merge gate (`continue-on-error` removed
on commit 240e310d after build+run passed green on the runner).

**.105 gotchas hit this round:** `~/.bashrc` exports `RUSTC_WRAPPER=sccache`
(binary missing) and it re-sources inside every `setsid bash -c`, so `unset`
before the heredoc doesn't stick — must `export RUSTC_WRAPPER=` *inside* the
build shell. The host's nightly was stale (2026-05-08) and couldn't even build
cargo-fuzz; `rustup toolchain install nightly` to 1.98 fixed it. And `git reset`
restores the tracked `.cargo/config.toml` mold flag — re-neutralize every time.

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

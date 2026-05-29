# Cyclomatic-complexity debt (lizard CCN)

The target is **CCN ≤ 25** (per `CLAUDE.md`). The debloat baseline restore left
21 functions above that. Per the project policy for legacy code — *"start at the
current max, tighten by 5 each refactor"* — the CI `lizard` gate is currently
currently set to **`-C 59`** and should keep ratcheting down: 59 → 54 → … → 25.

**Progress (2026-05-29):** gate lowered 72 → 64 → 59.
- `cmd_fetch` 72 → **48** (commit 2a0d9181): extracted the FETCH data-item
  parser into `fetch_plan()`.
- `handle_tls` 72 → 64 (`finish_smtp_auth()`) → **57** (`finalize_data_message()`
  — the end-of-DATA DKIM-sign+ingest+respond+reset).
All verified behaviour-preserving (573 mail tests pass).

Next gate-blocker is `handle` in `services/expresso-mail/src/smtp/session.rs:65`
(CCN 59 — the plaintext SMTP command loop, structurally similar to handle_tls).
Extract its DATA-finalize / AUTH branches the same way to reach 54.

Run `find services libs -name '*.rs' -not -path '*/target/*' | xargs lizard -l rust -C 25 -w`
to see the current offenders. Highest remaining:

| CCN | Function | File |
|----:|----------|------|
| 59 | `handle` | services/expresso-mail/src/smtp/session.rs:65 (next gate-blocker) |
| 57 | `handle_tls` | services/expresso-mail/src/smtp/submission.rs:220 (was 72) |
| 49 | `overrides_stats` | services/expresso-calendar/src/api/events.rs:6674 |
| 48 | `cmd_fetch` | services/expresso-mail/src/imap/session.rs (was 72) |
| 46 | `session_loop` | services/expresso-mail/src/smtp/session.rs:309 |
| 43 | `exdates_stats` | services/expresso-calendar/src/api/events.rs:5532 |
| 41 | `handle` | services/expresso-mail/src/lmtp.rs:59 |
| 40 | `exdates_preview_stats` | services/expresso-calendar/src/api/events.rs:9195 |
| 37 | `patch_recurrence_id_override_block` | services/expresso-calendar/src/api/events.rs:9404 |
| 36 | `patch_overrides_by_range` | services/expresso-calendar/src/api/events.rs:8488 |
| 33 | `main` | services/expresso-milter/src/main.rs:51 |
| 33 | `build_ical` | libs/expresso-imip/src/lib.rs:71 |
| 31 | `touch_overrides_by_range` | services/expresso-calendar/src/api/events.rs:8313 |
| 31 | `upload` | services/expresso-drive/src/api/files.rs:353 |
| 31 | `run` | services/expresso-mail/src/imap/session.rs:94 |
| 31 | `dispatch` | services/expresso-mail/src/imap/session.rs:297 |
| 31 | `process` | services/expresso-mail/src/ingest.rs:30 |
| 30 | `main` | services/expresso-mail/src/main.rs:68 |
| 28 | `main` | services/expresso-drive/src/main.rs:95 |
| 27 | `callback` | services/expresso-auth/src/handlers/callback.rs:42 |

(`local_to_ical_utc` in expresso-web reports a huge NLOC but CCN 1 — it is a
giant flat data table, not control-flow complexity; not a refactor target.)

Verify the gate locally with the CI toolchain (rustc 1.96):
`find services libs -name '*.rs' -not -path '*/target/*' | xargs lizard -l rust -C 64 -L 3000 -w`

## Refactor approach

The worst offenders are protocol state machines (IMAP `FETCH`, SMTP submission)
where complexity is somewhat inherent. Prefer extracting independent sub-handlers
(per-FETCH-item, per-SMTP-verb) into named functions rather than mechanical
splitting. After each batch, lower the `-C` value in `.github/workflows/ci.yml`.

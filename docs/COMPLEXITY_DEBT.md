# Cyclomatic-complexity debt (lizard CCN)

The target is **CCN ≤ 25** (per `CLAUDE.md`). The debloat baseline restore left
21 functions above that. Per the project policy for legacy code — *"start at the
current max, tighten by 5 each refactor"* — the CI `lizard` gate is currently
currently set to **`-C 48`** and should keep ratcheting down: 48 → 43 → … → 25.

**Calendar analytics (2026-05-29):** `overrides_stats` 49 → **32** by extracting
the window + presence `retain` filters into `retain_overrides_window()` /
`retain_overrides_presence()` (567 calendar tests pass). Gate now 48 (max is
`cmd_fetch` 48). Remaining cluster: `exdates_stats` (43), `exdates_preview_stats`
(40) — same filter-extraction applies; and `cmd_fetch` (48) could shed its
per-row response-builder loop next.


**Progress (2026-05-29):** gate lowered 72 → 64 → 59 → 57 → 50 → 49. The entire
mail SMTP/IMAP cluster is now decomposed:
- `cmd_fetch` 72 → **48** (`fetch_plan()`).
- `handle_tls` 72 → **45** (`finish_smtp_auth()`, `finalize_data_message()`,
  `handle_mail_from()`, `handle_rcpt_to()`).
- `handle` (smtp/session.rs) 59 → **43** and `session_loop` 46 → **31**: share
  `finalize_inbound_message()`, `handle_inbound_mail_from()`,
  `handle_inbound_rcpt_to()`.
All verified behaviour-preserving (573 mail tests pass at each step).

Next gate-blocker is the **calendar analytics cluster** in
`services/expresso-calendar/src/api/events.rs`: `overrides_stats` (49),
`exdates_stats` (43), `exdates_preview_stats` (40), `patch_*_override*` (36-37).
These share a stats-aggregation shape; extract the per-row tally + sort/clamp
into helpers to reach 44 then below.

Run `find services libs -name '*.rs' -not -path '*/target/*' | xargs lizard -l rust -C 25 -w`
to see the current offenders. Highest remaining:

| CCN | Function | File |
|----:|----------|------|
| 49 | `overrides_stats` | services/expresso-calendar/src/api/events.rs:6674 (next gate-blocker) |
| 48 | `cmd_fetch` | services/expresso-mail/src/imap/session.rs (was 72) |
| 45 | `handle_tls` | services/expresso-mail/src/smtp/submission.rs:220 (was 72) |
| 43 | `handle` | services/expresso-mail/src/smtp/session.rs:65 (was 59) |
| 43 | `exdates_stats` | services/expresso-calendar/src/api/events.rs:5532 |
| 31 | `session_loop` | services/expresso-mail/src/smtp/session.rs (was 46) |
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

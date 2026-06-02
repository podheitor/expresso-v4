# Exchange ActiveSync (EAS) — Implementation Plan

> **Status (2026-06): command set COMPLETE.** Sprints #70–#82 shipped the WBXML
> codec + every EAS command: OPTIONS, Provision, FolderSync (mail+calendar+
> contacts), Sync (mail read/write + calendar/contacts read), Ping, SendMail,
> GetItemEstimate, Search, ItemOperations(Fetch). Behind
> `mail_server.activesync_enabled` (default off). Remaining = refinements only:
> client-side calendar/contact writes, MIME multipart body decode, real
> device-policy, GetAttachment. The sections below are the original plan.


**Nature:** Unlike SAML/LDAP (brokered by Keycloak), ActiveSync **cannot be
delegated** — it's a Microsoft protocol (WBXML over HTTP POST to
`/Microsoft-Server-ActiveSync`) we must implement ourselves in Rust, reading
mail/calendar/contacts from the existing DB. This is the largest and highest-risk
target of the session, so it is sliced into small, independently-shippable
sprints, each gate-green, with a working checkpoint after the MVP.

**Scope decision (this plan):** **mail-only MVP first.** Calendar + contacts
collections are deferred to later sprints — get a phone syncing email end-to-end
before widening. EAS is versioned; we target **EAS 14.1** (widest modern client
support) with the minimal command set.

**Reuse:** the mail read/write path already exists (IMAP/POP3 over the
`mailboxes`/`messages` tables, body in object storage). EAS Sync maps onto the
same queries. Auth: EAS uses HTTP Basic over TLS — reuse the same legacy
`users.password_hash` (pgcrypto `crypt()`) check IMAP/POP3 use, plus the
`LoginLockout`. No new auth surface.

---

## Protocol shape (what we must implement)

- One endpoint: `POST /Microsoft-Server-ActiveSync?Cmd=<X>&User=<u>&DeviceId=<d>&DeviceType=<t>`
  + `OPTIONS` (advertises protocol versions + commands).
- Body: **WBXML** (WAP Binary XML) — a tokenized binary XML with per-codepage
  tag tables. This is the hard, foundational piece.
- HTTP Basic auth; `MS-ASProtocolVersion` header negotiation.
- Core MVP command handshake:
  1. **OPTIONS** → advertise versions/commands.
  2. **Provision** → device policy handshake (can be a minimal "no policy" ack).
  3. **FolderSync** (SyncKey 0 → N) → return the mail folder hierarchy.
  4. **Sync** → per-folder item add/change/delete with a rolling SyncKey.
  5. **Ping** → long-poll for changes (Direct Push); MVP can return "changes,
     re-sync" on a timer.

---

## Phases (each = 1 sprint, 1 commit, gate-green)

### Sprint 1 — WBXML codec (pure, no server) ← start here
- New module `services/expresso-mail/src/eas/wbxml/` (or a small lib crate):
  `encode` (XML-event stream → WBXML bytes) + `decode` (WBXML → events), plus
  the EAS codepage token tables for the namespaces the MVP needs (AirSync,
  FolderHierarchy, Provision; later AirSyncBase, Email).
- 100% pure logic → heavily unit-tested (round-trip encode/decode, multi-byte
  mb_u32 lengths, string tables, SWITCH_PAGE between codepages). Ideal Miri/fuzz
  target later.
- **No HTTP, no DB.** Zero risk to the running service. ~300–400 LOC + tests.

### Sprint 2 — endpoint skeleton + OPTIONS + Provision
- `eas/mod.rs`: axum routes for `OPTIONS` and `POST /Microsoft-Server-ActiveSync`,
  HTTP Basic auth (reuse the `users` crypt() check + LoginLockout), version
  negotiation, command dispatch stub.
- **OPTIONS** fully working; **Provision** minimal (acknowledge, grant a
  permissive policy key). Other commands → a valid WBXML "not implemented"
  status so a client doesn't hard-fail.
- Wire into `lib.rs` behind a config flag (`mail_server.activesync_port` or a
  path on the existing HTTP server), default off. ~250 LOC.

### Sprint 3 — FolderSync (mail hierarchy)
- **FolderSync**: SyncKey 0 → initial hierarchy (INBOX + the user's folders from
  `mailboxes`), SyncKey N → deltas. Map folder `special_use` → EAS folder types
  (2=Inbox, 5=Sent, 4=Deleted, …). A small `eas_sync_state` table (or reuse a
  per-device key) tracks the SyncKey. ~250 LOC + migration.

### Sprint 4 — Sync (mail items, read-only first)
- **Sync** for a mail folder: emit Add for new messages (envelope + body via
  AirSyncBase, truncated per client `TruncationSize`), Change for flag updates,
  Delete for expunged. Rolling per-(device,folder) SyncKey persisted. Read
  direction first; client→server (\Seen, delete, move) is a follow-up. Reuse the
  IMAP message queries. ~350 LOC.

### Sprint 5 — Ping (Direct Push) + client→server changes
- **Ping**: long-poll up to the client's heartbeat; return when a watched
  folder's max mod-sequence advanced (reuse the IMAP mod_sequence column).
- **Sync** client commands: apply \Seen / delete / move from the device.
- ~300 LOC.

### Later (separate plans, not now)
- Calendar collection (FolderSync type 8 + Sync of `calendar_events` as EAS
  Calendar items) and Contacts collection (type 9 + `contacts`). Each is a
  sprint pair (FolderSync entry + Sync mapping) once mail is solid.
- SendMail / SmartReply / SmartForward (compose via the existing SMTP submission
  path). GetAttachment via the existing attachment endpoint.
- Search, MeetingResponse, ItemOperations.

---

## Key decisions
- **Mail-only MVP**; calendar/contacts deferred. A phone syncing email is the
  proof point.
- **WBXML codec first** — the foundation everything else needs, pure and
  low-risk, so we validate the hardest piece before touching HTTP/DB.
- **Reuse the mail store + legacy auth** — no new DB schema beyond per-device
  sync state; no new auth surface.
- **Homegrown WBXML** — no maintained Rust EAS/WBXML crate exists; the codec is
  self-contained and well-specified (WAP-192 + MS-ASWBXML token tables). Decide
  at Sprint 1 whether it lives as a `mail` module or a tiny `libs/expresso-wbxml`
  crate (lean toward a lib crate so it's independently fuzzable/Miri-able).

## Risks / open items
- **WBXML correctness** is the make-or-break; real clients (iOS, Outlook mobile,
  Gaia) are unforgiving. Mitigation: exhaustive round-trip tests + test against
  captured sample WBXML from the MS-ASWBXML spec.
- **Provision policy**: clients may refuse to sync without a policy. MVP grants a
  permissive policy; real device-policy enforcement (PIN, remote wipe) is a much
  later, optional sprint.
- **Versioning**: target 14.1 only; advertise just that to avoid multi-version
  branching.
- **Where it lives**: mail service vs. a new `expresso-activesync` service.
  Recommend starting as an `eas` module in expresso-mail (shares the store);
  extract to its own service only if it grows large.
- This plan covers the **MVP through Ping**. Calendar, contacts, send, search,
  and device-policy are each follow-up work gated on the MVP proving out.

## Sequencing
1 (codec) → 2 (endpoint/OPTIONS/Provision) → 3 (FolderSync) → 4 (Sync mail) →
5 (Ping + client changes). After Sprint 4 a client can pull email; after 5 it's
a usable push-email account. Stop/reassess after each — especially after 1
(validate the codec) and after 4 (first real sync).

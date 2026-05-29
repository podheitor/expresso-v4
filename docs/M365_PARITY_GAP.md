# Expresso v4 — Microsoft 365 Parity Gap Analysis

> Honest assessment of what exists vs. what "a full M365 equivalent" requires.
> Written 2026-05-29, after removing ~891k lines of generated-bloat that had
> made several services *look* far more complete than they are.
>
> Numbers below are real post-debloat LOC + HTTP route counts per service.
> Route count is a rough proxy for API surface, not quality.

## TL;DR

"Full M365 in all detail" is a **multi-year, multi-team program**, not a backlog
that can be batch-written. The backend has genuine foundations for the
**communication + collaboration** half of M365 (mail, calendar, contacts, files,
chat/meet). It has **nothing** for the **Office document** half (Word/Excel/
PowerPoint/OneNote) or the **enterprise-platform** half (SharePoint, Power
Platform, Viva, etc.). The realistic path is to reach parity with **Google
Workspace / Exchange + ownCloud**, then *integrate* (not rewrite) an Office
editor suite.

## What exists today (real, per codebase)

| Service | LOC | Routes | State | M365 analogue |
|---|---:|---:|---|---|
| expresso-mail | 21,424 | 109 | **Substantial** — IMAP/SMTP/LMTP/Sieve/iMIP/DKIM + broad API | Exchange / Outlook |
| expresso-calendar | 22,186 | 125 | **Substantial** — CalDAV, events, recurrence, scheduling | Exchange Calendar |
| expresso-drive | 12,779 | 107 | **Substantial** — files, versions, shares, WOPI hooks | OneDrive (storage only) |
| expresso-contacts | 8,589 | 20 | **Partial** — CardDAV + address book | People |
| expresso-web | 7,237 | 99 | **Partial** — server-rendered UI shell | Outlook Web shell |
| expresso-admin | 6,182 | 36 | **Partial** — tenant/user admin | M365 Admin Center |
| expresso-auth | 6,194 | 10 | **Partial** — Keycloak/OIDC integration | Entra ID (thin) |
| expresso-search | 4,872 | 52 | **Partial** — index/query | Microsoft Search |
| expresso-meet | 4,388 | 26 | **Thin** — room/scheduling scaffolding | Teams Meetings |
| expresso-stats | 4,829 | 0 | Internal analytics (no HTTP) | — |
| expresso-notifications | 4,033 | 53 | **Partial** — alerts/DLQ | Activity feed |
| expresso-chat | 2,941 | 5 | **Stub** — minimal | Teams Chat |
| expresso-compliance | 2,523 | 29 | **Partial** — audit | Purview (thin) |
| expresso-flows | 977 | 8 | **Stub** | Power Automate |
| expresso-tenant-provision | 953 | 0 | Ops tooling | — |
| expresso-tenant-migrate | 889 | 0 | Ops tooling | — |
| expresso-imip-dispatch | 618 | 3 | Support svc (calendar invites) | — |
| expresso-milter | 439 | 2 | SMTP filter hook | — |
| expresso-wopi | 163 | 2 | **Stub** — WOPI protocol shell only | (host for Office editor) |
| expresso-event-audit | 159 | 3 | **Stub** | — |

## What is entirely missing (no service exists)

These are core M365 products with **zero** implementation here:

- **Word / Excel / PowerPoint** — document creation & co-editing. `expresso-wopi`
  is a 163-line protocol shell; WOPI only *embeds* an external editor. The editor
  itself (the hard part) does not exist and should **not** be written from
  scratch — see "Strategic recommendation."
- **OneNote** — notebooks.
- **SharePoint** — sites, lists, document libraries, intranet.
- **Power Platform** — Power Apps, Power BI, Power Automate (flows is a stub).
- **Viva / Yammer / Stream / Bookings / Forms / Planner / To Do / Loop** — none.
- **Teams as a product** — chat + meet are stubs; no presence, no calling, no
  channels/teams model, no federation.

## Gap within services that DO exist (parity, not presence)

Even the "substantial" services are short of M365 feature depth. Examples found
in the mail service this session (each a real, scoped unit of work):

- ✅ Signatures, conversation mute/pin, undo-send — *added this session*.
- ❌ Categories / labels (color-coded), rules UI on top of Sieve, focused inbox,
  @mentions, shared mailboxes, delegate access, retention/legal hold,
  S/MIME & encryption-at-rest per-message, ActiveSync/EAS for mobile.

Calendar, Drive, Contacts each have a comparable list. These are the *right* kind
of work to do incrementally — real, testable, reviewable.

## Strategic recommendation

1. **Don't chase "all of M365."** Target **Google Workspace / Exchange Online +
   ownCloud** parity for the comms+collab services. That is already ambitious and
   is where the codebase has real momentum.
2. **For Office editing, integrate — never rewrite.** Wire `expresso-wopi` to
   **Collabora Online** or **OnlyOffice Document Server** (both are WOPI-capable,
   self-hostable, open-core). This delivers Word/Excel/PowerPoint editing in
   weeks of integration instead of years of rewrite. This is the single
   highest-leverage move toward "looks like M365."
3. **Pick depth over breadth.** One genuinely complete app (Mail at Outlook
   parity) beats twelve stubs. Finish services in priority order; don't start new
   ones until the current tier is real.
4. **Never resume the numbered-sprint loop.** It generated ~891k lines of fake
   duplicate code (removed 2026-05-29). Volume is not progress.

## Suggested phased plan

- **Phase A — Stabilize:** confirm CI green after the debloat; the bloat removal
  touched 6 services. Nothing else should be built on an unverified base.
- **Phase B — Office editing (highest leverage):** Collabora/OnlyOffice via WOPI
  in `expresso-wopi` + drive. Gets the most visible M365 capability fastest.
- **Phase C — Mail to Outlook parity:** categories/labels, rules UI, focused
  inbox, delegate/shared mailboxes. (Signatures/mute/undo-send done.)
- **Phase D — Calendar + Contacts depth:** free/busy, resource booking, sharing
  ACL polish; contact groups, photo sync.
- **Phase E — Real-time (hardest):** turn chat/meet stubs into a Teams-like
  product — presence, channels, WebRTC calling. This is its own large program.
- **Phase F — Platform (optional, huge):** SharePoint-like sites, Power
  Automate. Only if the org genuinely needs them.

## Verification constraint (must-read)

The dev sandbox has **no Rust toolchain and no LAN/CI access** (confirmed
2026-05-29: `cargo`/`rustc`/`gh` absent; 192.168.15.x hosts unreachable).
Code changes are verified by reading + the GitHub `cargo test --workspace` CI
gate, which **someone with repo access must check after each push.** Do not
stack large unverified batches — that is how a repo ends up looking full and
not building.

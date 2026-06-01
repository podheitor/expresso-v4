# SAML 2.0 SSO — Implementation Plan

**Approach:** Keycloak-brokered SAML (SAML IdP registered as an external identity
provider inside each tenant's realm, exactly like the existing gov.br OIDC
broker). **Per-tenant IdP config. JIT auto-provisioning** on first federated
login.

**Why this approach:** Keycloak already *is* the IdP for Expresso (the Rust
`expresso-auth` service is an OIDC RP in front of it). Keycloak does all the hard
SAML work — XML parsing, XML-dsig signature verification, encryption — so the
Rust side needs **zero XML/xmlsec/FFI** and the security gates (geiger/unsafe,
deny) are untouched. The Rust service keeps speaking OIDC to Keycloak; the
SAML-vs-OIDC distinction lives only behind the broker. This mirrors gov.br 1:1
(`deploy/keycloak/seed-realm.sh` §10, `govbr_user_map`, `expresso-admin/govbr.rs`,
`expresso-auth/src/oidc/govbr.rs`).

---

## Architecture

```
Browser → expresso-auth /auth/login?kc_idp_hint=saml-<alias>
        → Keycloak realm <tenant-uuid>
            → SAML AuthnRequest → external SAML IdP (Okta/Azure AD/ADFS/…)
            ← SAML Response (signed assertion)  [KC verifies signature]
        → KC IdP-mapper copies SAML attributes → KC user attributes
        → KC firstBrokerLogin flow → KC issues OIDC token (as today)
        ← OIDC code → expresso-auth /auth/callback (UNCHANGED token path)
        → AuthContext built from JWT claims (as today)
        → JIT: upsert local `users` row + `saml_user_map` from claims
```

The OIDC callback/token-validation path (`handlers/callback.rs`,
`expresso-auth-client`) is **unchanged**. New work is: (a) KC realm SAML-IdP
provisioning, (b) per-tenant SAML-IdP config storage + admin CRUD, (c) JIT
provisioning of the local `users` row + `saml_user_map` on first login,
(d) plumbing the `kc_idp_hint` so the login redirect targets the right IdP.

---

## Phases (each = 1 sprint, 1 commit, full .105 gate green)

### Sprint A — schema + admin CRUD for per-tenant SAML IdP config
- **Migration `saml_idp_config`** (per-tenant IdP registration the admin manages):
  `id, tenant_id (FK tenants), alias (unique per tenant), display_name,
  entity_id, sso_url, slo_url NULL, signing_cert TEXT (PEM/base64 X509),
  name_id_format, attr_email/attr_display_name/attr_given/attr_family (mapping),
  enabled BOOL, created_at, updated_at`. RLS on tenant_id (sibling convention).
- **Migration `saml_user_map`** (mirrors `govbr_user_map`):
  `saml_subject TEXT, idp_alias TEXT, tenant_id FK, user_id FK, name_id_format,
  created_at, last_login_at`; PK `(tenant_id, idp_alias, saml_subject)`.
- **expresso-admin handler `saml.rs`** (mirrors `govbr.rs`, super_admin/tenant_admin
  gated): `GET/POST/DELETE /api/v1/admin/saml/idps` (CRUD of `saml_idp_config`),
  `GET /api/v1/admin/saml/idps/:alias`, `GET /api/v1/admin/saml/mappings`
  (list `saml_user_map`). Domain repo for both tables.
- Gate: admin is bin-only → `--bins`. ~250 LOC.

### Sprint B — Keycloak SAML-IdP provisioning (seed + sync)
- **Extend `deploy/keycloak/seed-realm.sh`**: add a SAML-IdP block analogous to
  §10 gov.br but `providerId:"saml"` with config keys
  `singleSignOnServiceUrl, singleLogoutServiceUrl, idpEntityId,
  signingCertificate, nameIDPolicyFormat, validateSignature:"true",
  wantAssertionsSigned:"true"`. Plus IdP attribute-mappers
  (`saml-user-attribute-idp-mapper`) email/name → user attributes, and client
  protocol-mappers copying those into the OIDC access_token (so the existing
  callback sees them as claims). Gated on env like gov.br.
- **`expresso-auth` KC-admin sync** (optional, additive): a small function in
  `kc_admin.rs` to push a `saml_idp_config` row into KC's
  `identity-provider/instances` API at provision time, so admin CRUD (Sprint A)
  can reflect into Keycloak without re-running the seed script. Behind the
  existing `KC_*` admin-config gate (no-op if unset).
- Gate: auth bin-only → `--bins`; shellcheck on the seed script. ~150 LOC + shell.

### Sprint C — JIT auto-provisioning + login hint
- **`expresso-auth/src/oidc/saml.rs`** (mirrors `govbr.rs`): detect a
  SAML-federated login from the token claims (an `idp_alias`/identity-provider
  claim KC stamps), extract subject + mapped attributes → a `SamlFederation`
  struct.
- **JIT in the callback path**: on a SAML-federated login, upsert the local
  `users` row (tenant from realm/`iss` as today, email/display from mapped
  claims, `role='user'`, `is_active=true`) and upsert `saml_user_map`
  (stamp `last_login_at`). Idempotent. Reuses the tenant-resolution that already
  exists; only adds the users/saml_user_map upsert. Audit event
  `auth.federation.saml` (mirrors the gov.br audit log).
- **Login hint**: `/auth/login` accepts `?idp=<alias>` → forwards
  `kc_idp_hint=<alias>` to the KC authorization endpoint so the user lands
  directly on their org's IdP (skip the KC IdP-picker).
- Gate: auth `--bins`. ~200 LOC.

### Sprint D — SP metadata convenience + docs + threat model
- **`GET /auth/saml/idp-template/:alias/metadata`** (optional): emit the SP-side
  SAML metadata XML that an org's IdP admin needs (ACS = KC's broker endpoint
  `/realms/<tenant>/broker/<alias>/endpoint`, entity_id = KC's). This is static
  XML templating (format!), **no signature/parsing** — safe, no new deps. Lets a
  customer self-configure their IdP without hand-assembling URLs.
- **`docs/SAML_SSO.md`**: per-tenant setup runbook (admin registers IdP →
  customer configures their IdP with our metadata → test login).
- **`docs/SECURITY.md` update**: add SAML to the threat model (untrusted input =
  SAML assertion, *covered by Keycloak's verified xmlsec*, not our code).
- Gate: rustdoc + shellcheck. ~100 LOC + docs.

---

## Key decisions baked in
- **No XML/xmlsec/samael/FFI in Rust** — Keycloak owns assertion parsing +
  signature verification. Security gates (geiger/deny/miri) unaffected.
- **OIDC token path unchanged** — SAML is invisible past the KC broker; the JWT
  the Rust service validates is identical in shape to today's.
- **Per-tenant** — `saml_idp_config` keyed by `tenant_id`; each realm gets its
  own KC IdP instance(s). Matches the realm-per-tenant model.
- **JIT auto-provision** — first federated login creates the local user (vs.
  gov.br's audit-only/admin-approve). `saml_user_map` records the binding.
- **Zero new crate dependencies** (only existing quick-xml *if* Sprint D needs
  any XML at all — likely just `format!` templating, so none).

## Risks / open items (surfaced, not blocking)
- KC SAML-IdP config has many optional knobs (encrypted assertions, signed
  AuthnRequests, NameID formats). Plan covers the common signed-assertion case;
  exotic options become follow-up sprints if a customer needs them.
- The exact KC claim that identifies "this login came via SAML IdP X" must be
  confirmed against the KC version in `deploy/` (likely the
  `identity_provider` claim via a hardcoded protocol-mapper) — verified in
  Sprint C against a live realm before wiring JIT.
- Local-user JIT writes to `users` from the auth service — today the auth
  service only does audit writes. This is a deliberate, scoped addition (one
  idempotent upsert), RLS-scoped by tenant. Flagged because it widens the auth
  service's DB surface.

## Sequencing
A → B → C are ordered (C depends on B's KC mappers and A's tables). D is
independent polish, can come last or be dropped. Recommend stopping after C for
a working end-to-end SAML login, then deciding on D.

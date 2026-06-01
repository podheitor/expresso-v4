# LDAP / Active Directory Sync — Implementation Plan

**Approach:** Keycloak User Federation (an LDAP/AD provider registered per tenant
realm via Keycloak's `components` API). Mirrors the SAML SSO design: Keycloak
does the LDAP bind, search, sync, and password delegation; the Rust side stores
per-tenant config, reflects it into KC, and JIT-provisions the local `users` row
on first login. **Zero LDAP client in Rust** — no `ldap3`/FFI, security gates
untouched.

**Why brokered (same call as SAML):** Keycloak's User Storage SPI already
implements full LDAP/AD federation (connection pooling, AD-specific quirks,
kerberos, sync modes, password policies). Re-implementing an LDAP client in Rust
would be large and risky. KC federates the directory; users then log in via the
existing OIDC flow and the JIT path from SAML sprint C provisions them locally —
**the JIT code already written is reused as-is** (it keys on the KC subject, not
on SAML specifically).

---

## Architecture

```
Admin registers LDAP config  → expresso-admin (ldap_config table)
                             → POST /internal/ldap/sync (auth → KC components API)
                             → Keycloak realm <tenant>: LDAP UserStorageProvider component
User logs in (USER/PASS or browser)
                             → Keycloak binds against LDAP/AD, imports the user
                             → KC issues OIDC token (unchanged)
                             → /auth/callback → JIT upsert local users row
                                (reuses jit_provision — keyed by KC sub)
```

The OIDC token path, callback, and JIT upsert are unchanged. New work: (a) a
per-tenant `ldap_config` table + admin CRUD, (b) KC `components` provisioning in
`kc_admin.rs` + an internal sync endpoint, (c) light JIT generalization so the
audit event reflects "ldap" federation, (d) docs.

---

## Phases (each = 1 sprint, 1 commit, full .105 gate green)

### Sprint A — schema + admin CRUD for per-tenant LDAP config
- **Migration `ldap_config`**: `id, tenant_id (FK), alias, vendor (ad|other),
  connection_url, users_dn, bind_dn, bind_credential, username_attr,
  rdn_attr, uuid_attr, user_object_classes, search_scope, sync_period_secs,
  enabled, created_at, updated_at`. UNIQUE(tenant_id, alias). RLS via tenant
  column. `bind_credential` is a secret — stored encrypted-at-rest or referenced
  (see open items); never returned in GET responses.
- **expresso-admin `ldap.rs`** (mirrors `saml.rs`): CRUD tenant-scoped via
  `require_tenant_match`; GET redacts `bind_credential`. Routes
  `/api/v1/ldap/configs[/:id]`.
- Gate: admin bin-only → `--bins`. ~250 LOC.

### Sprint B — Keycloak LDAP component provisioning
- **`kc_admin.rs`**: `LdapComponentSpec` + `upsert_ldap_component` /
  `delete_ldap_component` using `POST/PUT/DELETE
  /admin/realms/{realm}/components` (providerId `ldap`, providerType
  `org.keycloak.storage.UserStorageProvider`, parentId = realm, config as
  string-array map). Optional: trigger a full sync via
  `POST /admin/realms/{realm}/user-storage/{id}/sync?action=triggerFullSync`.
- **`handlers/ldap_sync.rs`**: internal LAN-trusted `POST /internal/ldap/sync`
  + `/internal/ldap/remove` (same trust model as the SAML ones) the admin calls
  after CRUD. 503 when KC admin unconfigured.
- **seed-realm.sh §13**: optional declarative LDAP block (gated on
  `LDAP_CONNECTION_URL` + `LDAP_BIND_DN`), mirroring §12.
- Gate: auth `--bins` + shellcheck. ~200 LOC + shell.

### Sprint C — JIT generalization + docs
- **JIT**: the existing `jit_provision_saml` becomes federation-agnostic
  (rename to `jit_provision_federated` or add an `ldap` branch). LDAP logins
  carry `identity_provider` = the LDAP component alias (KC sets it the same way),
  so `SamlFederation::from_ctx` already fires; generalize the marker type or add
  a sibling so the audit event is `auth.federation.ldap`. The `saml_user_map`
  table is SAML-specific — add a parallel `ldap_user_map` OR widen to a generic
  `federated_user_map(tenant, source, alias, subject, user_id)` (decision in
  open items). Login `?idp=<alias>` already works for any KC IdP/federation
  alias.
- **docs/LDAP_SYNC.md**: per-tenant setup runbook + AD vs generic-LDAP notes;
  **SECURITY.md** threat-model row (bind credential handling; LDAP done by KC).
- Gate: auth `--bins` + rustdoc. ~150 LOC + docs.

---

## Reuse from the SAML work (accelerators)
- `admin/saml.rs` → `admin/ldap.rs` (same tenant-scoped CRUD shape).
- `kc_admin.rs` `SamlIdpSpec`/upsert/delete → `LdapComponentSpec` (components
  endpoint instead of identity-provider/instances).
- `handlers/saml_sync.rs` internal endpoints → `handlers/ldap_sync.rs`.
- `identity_provider` claim + `kc_idp_hint` login + JIT upsert: **already built**
  in SAML sprint C — LDAP logins flow through the same path.
- seed-realm.sh §12 SAML block → §13 LDAP block.

## Key decisions baked in
- **No LDAP client / FFI in Rust** — KC owns the directory integration.
- **Per-tenant** config, realm-per-tenant, same as SAML.
- **JIT reused** — first login provisions the local user via the existing path.
- **Zero new crate dependencies.**

## Open items / risks (surfaced, not blocking)
- **bind_credential at rest**: it's a real secret (LDAP service-account
  password). Options: (a) store encrypted via `expresso-crypto` (exists in the
  workspace), (b) store a reference and keep the secret in KC only (write-only:
  POST sets it, GET never returns it, KC holds the source of truth). Recommend
  (b) — write-through to KC, never persist the plaintext in our DB; the
  `ldap_config` row keeps everything *except* the credential, which lives only in
  the KC component. This avoids adding a secrets-at-rest surface to our DB.
- **user_map shape**: reuse a generic `federated_user_map` vs. a new
  `ldap_user_map`. Generic is cleaner long-term but touches the SAML mapping;
  a sibling table is lower-risk for this feature. Decide in Sprint C.
- **AD vs plain LDAP**: KC has an `ad`/`other` vendor switch that sets sane
  defaults (objectGUID, sAMAccountName). Expose `vendor` in the config and pass
  through; don't try to auto-detect.
- **Sync mode**: import-on-demand (login) works with zero scheduling. A periodic
  full sync (`sync_period_secs`) is optional polish — KC runs it, we just set the
  config value.

## Sequencing
A → B → C ordered (B needs A's table; C needs B's KC component + reuses SAML
JIT). Recommend stopping after B for a working "users can log in via LDAP" state,
then C for the audit/docs polish. ~3 sprints total.

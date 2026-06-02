# LDAP / Active Directory Sync — Setup Runbook

Expresso federates an organization's LDAP or Active Directory per tenant,
brokered by Keycloak User Federation. Users authenticate against the directory;
on first login the local user is provisioned automatically (JIT).

Architecture note: **Keycloak does the LDAP work** (bind, search, sync, password
delegation) via its User Storage SPI. The Rust services never speak LDAP — no
LDAP client, no FFI. LDAP-backed users are *local* Keycloak users (User Storage,
not identity brokering), so they log in directly against the realm. See
`docs/LDAP_SYNC_PLAN.md` for the design.

## Concepts

- **realm = tenant_id (UUID)** — realm-per-tenant; each tenant federates its own
  directory.
- **name** — the LDAP component name within a realm (e.g. `corp-ad`); the
  `ldap_config.alias` row.
- **bind credential** — the LDAP service-account password. It is **write-through
  to Keycloak only**; Expresso never stores it (the `ldap_config` row carries
  everything except the password).

## Setup (per tenant)

1. **Register the directory** (admin API, super_admin or tenant_admin):

   ```http
   POST /api/v1/ldap/configs
   {
     "tenant_id": "<tenant-uuid>",
     "alias": "corp-ad",
     "vendor": "ad",
     "connection_url": "ldaps://ad.corp.example:636",
     "users_dn": "CN=Users,DC=corp,DC=example",
     "bind_dn": "CN=svc-keycloak,DC=corp,DC=example",
     "bind_credential": "<service-account-password>",
     "username_attr": "sAMAccountName",
     "rdn_attr": "cn",
     "uuid_attr": "objectGUID",
     "user_object_classes": "person, organizationalPerson, user"
   }
   ```

   For plain LDAP use `"vendor": "other"` and the LDAP defaults
   (`uid` / `entryUUID` / `inetOrgPerson`). The config (minus the credential) is
   stored; the admin service then calls `POST /internal/ldap/sync` (auth → KC
   components API), which writes the component **including the bind credential**
   into Keycloak. When KC admin is not wired, run `deploy/keycloak/seed-realm.sh`
   with the `LDAP_*` env vars (§13) instead.

2. **Test login:** any directory user logs in normally (USER/PASS or browser)
   against the tenant. Keycloak validates against LDAP, imports the user, and
   issues the usual OIDC token. On first login the user is JIT-provisioned: a
   local `users` row (keyed by the Keycloak subject) and an `ldap_user_map`
   binding are created.

## Verifying

- `GET /api/v1/ldap/configs?tenant_id=<uuid>` — list registered directories
  (the bind credential is never returned).
- Audit event `auth.federation.ldap` is emitted on each LDAP login.

## Removing a directory

```http
DELETE /api/v1/ldap/configs/<id>
```

Then `POST /internal/ldap/remove` (or re-seed) removes the Keycloak component.
Existing `ldap_user_map` rows + local users are retained; deactivate users via
the admin user API if required.

## AD vs plain LDAP

- **Active Directory**: `vendor: "ad"`, `username_attr: "sAMAccountName"`,
  `uuid_attr: "objectGUID"`, object classes `person, organizationalPerson, user`.
- **OpenLDAP / generic**: `vendor: "other"`, `username_attr: "uid"`,
  `uuid_attr: "entryUUID"`, object classes `inetOrgPerson, organizationalPerson`.

## Troubleshooting

- **No users can log in** — bind failed; check `connection_url` (use `ldaps://`
  for TLS), `bind_dn`, and that the credential was synced into KC.
- **User logs in but no local profile** — JIT failed (non-blocking by design);
  check the auth log for `LDAP JIT provisioning failed` and DB reachability.
- **Login works but isn't recorded as LDAP** — the `ldap_id` claim mapper is
  missing from the realm; re-run the seed (§13) so the `LDAP_ID` attribute is
  exposed as a claim.

# SAML 2.0 SSO — Setup Runbook

Expresso supports SAML 2.0 single sign-on per tenant, brokered by Keycloak. An
organization's SAML IdP (Okta, Azure AD / Entra ID, ADFS, Ping, …) federates
into the tenant's Keycloak realm; on first login the local user is provisioned
automatically (JIT).

Architecture note: **Keycloak is the SAML Service Provider.** The Rust services
never parse SAML or verify XML signatures — Keycloak does, and issues the same
OIDC token the platform already consumes. See `docs/SAML_SSO_PLAN.md` for the
design rationale.

## Concepts

- **alias** — short name for one IdP within a realm (e.g. `okta`). It is the
  `kc_idp_hint` value and the `saml_idp_config.alias` row. One realm may host
  several IdPs (distinct aliases).
- **realm = tenant_id (UUID)** — realm-per-tenant; each tenant configures its own
  IdP(s) independently.

## Setup (per tenant)

1. **Register the IdP** (admin API, super_admin or the tenant's tenant_admin):

   ```http
   POST /api/v1/saml/idps
   {
     "tenant_id": "<tenant-uuid>",
     "alias": "okta",
     "display_name": "Acme Okta",
     "entity_id": "http://www.okta.com/exk...",
     "sso_url": "https://acme.okta.com/app/.../sso/saml",
     "signing_cert": "<base64 X.509 from the IdP metadata>",
     "name_id_format": "urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress",
     "attr_email": "email",
     "attr_display_name": "displayName"
   }
   ```

   This stores the config. To push it into Keycloak immediately, the admin
   service calls `POST /internal/saml/idp-sync` (auth service → KC admin API);
   when Keycloak admin is not wired, run `deploy/keycloak/seed-realm.sh` with the
   `SAML_IDP_*` env vars instead (see §12 of that script).

2. **Hand the customer our SP metadata** so they configure their IdP:

   ```
   GET /auth/saml/idp-template/<alias>/metadata
   ```

   Returns the SP metadata XML. The two URLs the customer needs:
   - **ACS (Assertion Consumer Service):**
     `{kc_base}/realms/{tenant}/broker/{alias}/endpoint`
   - **SP entityID:** `{kc_base}/realms/{tenant}`

   The customer's IdP must sign assertions (we set `WantAssertionsSigned`),
   release an email attribute, and use the NameID format above.

3. **Test login:**

   ```
   GET /auth/login?idp=<alias>
   ```

   `?idp=<alias>` forwards `kc_idp_hint` so the user lands directly on their IdP
   (no Keycloak IdP-picker). On success the user is JIT-provisioned: a `users`
   row (keyed by the Keycloak subject) and a `saml_user_map` binding are created,
   and the normal session cookies are set.

## Verifying

- `GET /api/v1/saml/idps?tenant_id=<uuid>` — list a tenant's registered IdPs.
- `GET /api/v1/saml/mappings?tenant_id=<uuid>` — list provisioned
  (subject → user) bindings, newest login first.
- Audit log event `auth.federation.saml` is emitted on each SAML login.

## Removing an IdP

```http
DELETE /api/v1/saml/idps/<id>
```

Then `POST /internal/saml/idp-delete` (or re-seed) removes the Keycloak broker
instance. Existing `saml_user_map` rows are retained for audit; the local users
remain (deactivate them via the admin user API if required).

## Troubleshooting

- **Login lands on the KC picker instead of the IdP** — the `?idp=<alias>` hint
  didn't match a Keycloak IdP instance; check the alias was synced into the
  realm.
- **`invalid signature` in Keycloak logs** — the `signing_cert` doesn't match
  the IdP's current signing key; re-fetch it from the IdP metadata.
- **User logs in but has no mailbox/profile** — JIT provisioning failed
  (non-blocking by design); check the auth service log for
  `SAML JIT provisioning failed` and confirm the DB is reachable.

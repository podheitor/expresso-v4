# Pilot Multi-Realm Activation — Prod Runbook

Validated on 2026-04-24. Pilot realm `30aa38fd-5948-47f0-9e42-eee64a621745` created in prod Keycloak; `auth-rp` + `calendar` configured with multi-realm; end-to-end `/auth/me` returned 200 with correct `tenant_id` / `roles`.

## Key evidence

- `multi-realm validator ready, template: http://auth.expresso.local:8080/realms/{realm}, hosts: 1`
- `realm validator ready, realm: 30aa38fd-..., validators_cached: 1`
- `curl -H "Host: pilot.expresso.local" -H "Authorization: Bearer <pilot-jwt>" http://127.0.0.1:8012/auth/me` → 200
  - response: `tenant_id=30aa38fd-...`, `email=admin@pilot.expresso.local`, `roles=[TenantAdmin,...]`
- Same request without `Host` → 401 (legacy single-realm `expresso` rejects pilot issuer)

## Env vars wired (prod 125)

Both `compose-phase3.yaml` (expresso-calendar block) and `compose-auth-rp.yaml` (expresso-auth block):

```yaml
AUTH__OIDC_ISSUER_TEMPLATE: "http://auth.expresso.local:8080/realms/{realm}"
AUTH__OIDC_AUDIENCE: "account"
AUTH__TENANT_HOSTS: "pilot.expresso.local:30aa38fd-5948-47f0-9e42-eee64a621745"
extra_hosts:
  - "auth.expresso.local:host-gateway"   # auth-rp (HTTPS :443 via nginx)
  - "auth.expresso.local:172.19.0.3"     # calendar (HTTP :8080 direct to KC)
```

## Format gotchas

- `AUTH__TENANT_HOSTS`: comma-sep entries `host:realm` (colon sep, ≠ `=`). Multiple: `a.ex:a,b.ex:b`.
- Template placeholder: `{realm}` (≠ `{tenant}`).
- Audience: JWT from Keycloak direct-grant carries `aud=account` by default → set `AUTH__OIDC_AUDIENCE=account`.
- JWT `iss` = `http://auth.expresso.local:8080/realms/...` regardless of request URL (Keycloak `KC_HOSTNAME` setting). Template MUST match exactly.

## Activation for additional tenants

1. Provision realm: see [`docs/TENANT-ONBOARDING.md`](../docs/TENANT-ONBOARDING.md).
2. Append entry to `AUTH__TENANT_HOSTS`: `pilot.expresso.local:...,acme.expresso.local:<realm-uuid>`.
3. `docker compose up -d --force-recreate <service>`.
4. Validate via `/auth/me` with `Host: <tenant-host>` + bearer JWT issued by Keycloak.

## Rollback

Remove the 3 `AUTH__*` env vars + `extra_hosts` auth.expresso.local:172.19.0.3 from compose files → recreate. Services revert to single-realm legacy validator.

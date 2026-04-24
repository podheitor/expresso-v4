# Deploy fase 2 (realm-per-tenant) — 2026-04-24

Commits em produção (prod 125):
- `997c522` sprint #40c-fase4 — drop tenant_id claim mapper
- `993342f` sprint #40c-fase3 — expresso-tenant-migrate CLI
- `37f1c03` + `e46a888` + `aba8b94` + `892fcda` sprint #40c-fase2 — multi-realm lib + wiring
- `35e2498` sprint #40c-fase1 — expresso-tenant-provision CLI

Imagens fase2 (SHA256):
- expresso-chat:fase2     1f90c794e63f
- expresso-meet:fase2     3cb8b4acee3c
- expresso-calendar:fase2 08be07e3069f
- expresso-auth:fase2     d167dcc66d8e

Todas tagueadas como `:latest`. Imagens anteriores preservadas em `:pre-fase2`
p/ rollback rápido (`docker tag expresso-X:pre-fase2 expresso-X:latest && docker compose up -d --force-recreate`).

## Modo operacional atual

- **Single-realm compat**: sem `AUTH__OIDC_ISSUER_TEMPLATE` + `AUTH__TENANT_HOSTS`
  no env, services operam em modo single-realm usando `AUTH__OIDC_ISSUER` /
  `AUTH__OIDC_AUDIENCE` (comportamento pré-fase2 preservado).
- **Multi-realm ativo** (fase 2 completa): configurar em compose:
  ```yaml
  environment:
    AUTH__OIDC_ISSUER_TEMPLATE: "http://expresso-keycloak:8080/realms/{realm}"
    AUTH__OIDC_AUDIENCE: "expresso-web"
    AUTH__TENANT_HOSTS: "tenant1.expresso.com.br:tenant1,tenant2.expresso.com.br:tenant2"
  ```
  Services então fazem extração de tenant via Host header → realm name →
  validator cache.

## Smoke test validado

- chat/meet/calendar/auth-rp: binários fase2 contém símbolos
  `AUTH__OIDC_ISSUER_TEMPLATE`, `multi-realm validator ready`,
  `Arc<MultiRealmValidator>` — fase2 code presente.
- Serviços escutam nas portas 8010/8011/8002/8012 respectivamente.
- JWKS load via `auth.expresso.local` funciona para auth-rp (host networking).
- chat/meet degradam p/ DEV auth mode pois KC OIDC discovery retorna
  issuer `auth.expresso.local:8080` não resolvível dentro da rede docker —
  problema pré-existente da config KC (frontend-url), não regressão fase2.

## Rollback

```bash
for svc in chat meet calendar auth; do
  sudo docker tag expresso-$svc:pre-fase2 expresso-$svc:latest
done
cd ~/expresso
sudo docker compose -f compose-chat-meet.yaml  up -d --force-recreate expresso-chat expresso-meet
sudo docker compose -f compose-phase3.yaml     up -d --force-recreate expresso-calendar
sudo docker compose -f compose-auth-rp.yaml    up -d --force-recreate expresso-auth
```

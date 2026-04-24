# Multi-Realm JWT Validation — Architecture Guide

Técnica que permite múltiplos tenants Keycloak (realms) validarem contra os
mesmos binários de serviço sem rebuild. Entregue em sprints #42–#45 (serviços
auth-rp, calendar, contacts, drive, mail, chat, meet).

## Ideia central

- Cada serviço carrega um `MultiRealmValidator` ao invés de um `OidcValidator`
  único.
- Request ingressa → middleware resolve o tenant pelo header `Host` → escolhe
  o `OidcValidator` do realm correspondente → valida JWT.
- Config triad por serviço (env vars):

```
AUTH__OIDC_ISSUER_TEMPLATE = http://auth.host/realms/{realm}
AUTH__OIDC_AUDIENCE        = account[,expresso-web]
AUTH__TENANT_HOSTS         = <host1>:<realm-uuid1>,<host2>:<realm-uuid2>
```

## Componentes

| Arquivo | Responsabilidade |
|---------|------------------|
| [libs/expresso-auth-client/src/validator.rs](../libs/expresso-auth-client/src/validator.rs) | `OidcConfig` + `OidcValidator`. Suporta audience **CSV** (split/trim/filter). |
| [libs/expresso-auth-client/src/multi_validator.rs](../libs/expresso-auth-client/src/multi_validator.rs) | `MultiRealmValidator`. Mantém pool `HashMap<realm, OidcValidator>` lazy. |
| [libs/expresso-auth-client/src/tenant_resolver.rs](../libs/expresso-auth-client/src/tenant_resolver.rs) | `TenantResolver`. Parse `AUTH__TENANT_HOSTS` → `HashMap<host, realm>`. |
| `services/<svc>/src/api/context.rs` | Middleware axum extrai Host, resolve realm, injeta `AuthContext` via extension. |
| `services/<svc>/src/main.rs` | Fn `resolve_multi_realm()` constrói validator + resolver na startup e wire via `Extension` layer. |

## Multi-audience (sprint #45)

Chat/meet tem conflito: webapp legacy emite JWT `aud=expresso-web`, DAV clients
emitem `aud=account`. Solução: CSV em `AUTH__OIDC_AUDIENCE`.

```rust
// libs/expresso-auth-client/src/validator.rs
pub fn audiences(&self) -> Vec<&str> {
    self.audience.split(',').map(str::trim).filter(|s| !s.is_empty()).collect()
}
val.set_audience(&self.cfg.audiences());  // jsonwebtoken aceita slice
```

`primary_audience()` = primeiro entry, usado para extração de roles via
`resource_access[audience].roles`.

## Runtime log esperado (startup)

```
INFO expresso_<svc>: multi-realm validator ready, template: http://auth.host/realms/{realm}, hosts: N
INFO expresso_<svc>: HTTP API listening, addr: 0.0.0.0:PORT
```

`hosts: N` = número de tenants carregados. Se `hosts: 0` → `AUTH__TENANT_HOSTS`
está vazio/malformado, serviço rejeitará 100% dos requests.

## Onboarding novo tenant

Ver [ops/tenant-add.sh](../ops/tenant-add.sh) — emite checklist completo dos
env snippets, systemd timer command e compose files a patchar.

## Smoke tests

- [ops/smoke-dav.sh](../ops/smoke-dav.sh) — 7 probes (JWT + 6 services).
- [ops/smoke-chat-meet.sh](../ops/smoke-chat-meet.sh) — chat+meet isolado.
- Systemd timer `expresso-smoke-dav@<tenant>.timer` — 10min interval, push
  métricas a Pushgateway `expresso_smoke_dav_success{service,tenant}`.
- Prometheus rules: `ExpressoSmokeDavFailing` + `ExpressoSmokeDavStale`
  ([ops/prometheus/alerts/smoke-dav.yml](../ops/prometheus/alerts/smoke-dav.yml)).

## Troubleshooting

| Sintoma | Causa provável | Fix |
|---------|----------------|-----|
| `401 invalid_token` em todos requests | `AUTH__TENANT_HOSTS` não contem o Host recebido | Conferir header Host + reverse proxy + env var |
| `401 invalid_token` só em um realm | Keycloak do realm sem client direct-access ou secret errado | Conferir client `expresso-dav` (confidential, directAccessGrants=true) |
| `InvalidAudience` JWT error | aud do token não está em `AUTH__OIDC_AUDIENCE` CSV | Adicionar aud ao CSV **ou** configurar audience mapper no client Keycloak |
| `hosts: 0` no log startup | Env vazia ou sintaxe `host:uuid` errada | CSV `<fqdn>:<uuid>` separado por `,`, sem espaço após `,` |
| `503 upstream` em todos | `extra_hosts` não mapeia `auth.expresso.local` | Adicionar `extra_hosts: [auth.expresso.local:<kc-ip>]` no service compose |

## Status produção (2026-04-24)

7 serviços × 2 tenants (pilot, pilot2) = 14 probes E2E PASS a cada 10min.
Ver [SESSION_HANDOFF.md](../SESSION_HANDOFF.md) para histórico completo.

## Escopo — API/DAV apenas (2026-04-24)

Este rollout cobre **backends de API/DAV** (7 serviços). A **UI web
(`expresso-web` + `expresso-auth-rp` + `expresso-nginx`) permanece em modo
single-realm** nesta entrega:

- `expresso-nginx`: config default, sem vhosts por tenant.
- `expresso-web`: URLs de login fixas (`https://expresso.local/auth/*`).
- `expresso-auth-rp`: tem `AUTH__TENANT_HOSTS` carregado (validator multi-realm
  ativo), mas o fluxo OIDC code-flow usa `AUTH_RP__ISSUER` estático
  (`/realms/expresso`).

### O que funciona HOJE (multi-tenant)
- Clientes DAV (iOS/Android, Thunderbird): CalDAV, CardDAV.
- Chat/Meet API calls diretas (Bearer JWT).
- Mail (IMAP/SMTP + REST API).
- Drive (REST API).

### O que NÃO funciona ainda
- Login via browser em `https://pilot.expresso.local` → resolve default nginx.
- Seleção de tenant no UI web.

### Roadmap próxima sprint (UI multi-tenant)
1. Nginx vhosts per-tenant (TLS + Host routing → `expresso-web`/`expresso-auth-rp`).
2. `expresso-auth-rp`: derivar issuer do Host header (similar aos backends).
3. `expresso-web`: URLs de login dinâmicas per-tenant.
4. Certs: multi-SAN ou per-tenant.

Ver [SESSION_HANDOFF.md](../SESSION_HANDOFF.md) para decisão registrada.

## Sprint #46 — UI web multi-tenant ATIVO (2026-04-24)

Refactor estendido ao **Relying Party (`expresso-auth-rp`)** + **reverse proxy**:

- `expresso-auth-rp`: resolve `authorization_endpoint` / `token_endpoint` /
  `end_session_endpoint` **por Host header** via `TenantProviderCache` (lazy
  discovery per realm). Falls back a config single-realm quando Host não mapeado.
- `expresso-web`: `PUBLIC__AUTH_LOGIN=/auth/login` (relative) + `PUBLIC__WEB_BASE_URL=""`
  → browser usa Host corrente (pilot/pilot2/expresso.local) em todo o fluxo.
- Nginx: catch-all `server_name _` no vhost 443 continua roteando novos hosts
  para `expresso-web` + `/auth/*` → `expresso-auth-rp` (zero config change).

### Novas env vars (`expresso-auth-rp`)

| Var                              | Exemplo                                            | Função                         |
| -------------------------------- | -------------------------------------------------- | ------------------------------ |
| `AUTH_RP__ISSUER_TEMPLATE`       | `http://auth.expresso.local:8080/realms/{realm}`   | issuer per tenant              |
| `AUTH_RP__REDIRECT_URI_TEMPLATE` | `https://{host}/auth/callback`                     | callback absoluto per Host     |
| `AUTH_RP__POST_LOGOUT_TEMPLATE`  | `https://{host}/`                                  | redirect pós-logout per Host   |

### Smoke test web (3 cenários validados)
1. `GET https://pilot.expresso.local/auth/login` → 303 Keycloak realm pilot, redirect_uri correto.
2. `GET https://pilot2.expresso.local/auth/login` → 303 Keycloak realm pilot2, redirect_uri correto.
3. `GET https://expresso.local/auth/login` → 303 realm default `expresso` (legacy fallback).

### Requisitos Keycloak (por realm tenant)
- Client `expresso-web` com `redirect_uris = [https://<tenant-host>/auth/callback]`.
- Client `expresso-web` com `web_origins = [https://<tenant-host>]`.
- Pre-registrado em `pilot.expresso.local` + `pilot2.expresso.local`.

### Limitação conhecida
- Client `expresso-web` precisa existir em cada realm do tenant. Onboarding
  automatizado: atualizar `ops/tenant-add.sh` para criar o client via API admin.

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

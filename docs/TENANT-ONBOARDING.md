# Tenant Onboarding — Runbook (multi-realm)

Pós-fase4 (commit `997c522`) cada tenant = 1 realm Keycloak dedicado.
`tenant_id` é o próprio nome do realm (UUID v4), extraído do claim `iss` do
JWT pelos serviços (`realm_from_iss` em
[libs/expresso-auth-client/src/claims.rs](libs/expresso-auth-client/src/claims.rs)).

## 1. Provisionar novo tenant

```bash
export KC_ADMIN_PASS='...'
export TENANT_ADMIN_PASSWORD='senha-inicial-forte'
cargo run -p expresso-tenant-provision -- \
  --kc-url https://auth.expresso.exemplo.br \
  --realm "$(uuidgen)" \
  --display "ACME Corp" \
  --admin-email admin@acme.exemplo.br \
  --base-redirect https://acme.expresso.exemplo.br
```

Cria idempotente: realm + 3 clients (`expresso-web` public PKCE,
`expresso-dav` confidential directGrants, `expresso-admin` confidential
serviceAccount) + 4 roles (`SuperAdmin`/`TenantAdmin`/`User`/`Readonly`)
+ 1 usuário admin inicial. Sem mapper hardcoded de `tenant_id` (fase4).

Fonte: [services/expresso-tenant-provision/src/main.rs](services/expresso-tenant-provision/src/main.rs).

## 2. Migrar usuários de realm legado (opcional)

Se havia realm monolítico `expresso` com atributo `tenant_id` por usuário:

```bash
export KC_ADMIN_PASS='...'
# Dry-run primeiro
cargo run -p expresso-tenant-migrate -- \
  --kc-url https://auth.expresso.exemplo.br \
  --kc-admin-user admin \
  --source-realm expresso

# Depois, aplicar
cargo run -p expresso-tenant-migrate -- \
  --kc-url https://auth.expresso.exemplo.br \
  --kc-admin-user admin \
  --source-realm expresso \
  --apply \
  --send-reset
```

`--send-reset` dispara `UPDATE_PASSWORD` + `VERIFY_EMAIL` nos usuários
copiados. Fonte: [services/expresso-tenant-migrate/src/main.rs](services/expresso-tenant-migrate/src/main.rs).

## 3. Limpeza mapper legado (one-shot)

Realms legados podem ter mapper hardcoded `tenant_id`. Remover:

```bash
KC_URL=http://127.0.0.1:8080 KC_ADMIN=admin KC_PASS='...' REALM=expresso \
  bash ops/keycloak/remove-hardcoded-tenant-mapper.sh
```

Idempotente. Fonte: [ops/keycloak/remove-hardcoded-tenant-mapper.sh](ops/keycloak/remove-hardcoded-tenant-mapper.sh).

## 4. Ativar validação multi-realm nos serviços

Default = compat single-realm (`AUTH__OIDC_ISSUER` setado, tenant único).
Para multi-realm, setar no compose de cada serviço (`expresso-chat`,
`expresso-meet`, `expresso-calendar`, `expresso-auth-rp`):

```yaml
environment:
  AUTH__OIDC_ISSUER_TEMPLATE: "https://auth.expresso.exemplo.br/realms/{tenant}"
  AUTH__TENANT_HOSTS: "acme.expresso.exemplo.br=<realm-uuid>,..."
```

`{tenant}` placeholder é substituído pelo realm extraído do Host header do
request (via `TenantAuthenticated` extractor ou resolver em
[libs/expresso-auth-client/src/resolver.rs](libs/expresso-auth-client/src/resolver.rs)).

## 5. iMIP (convites calendar)

Habilitar em produção: setar no compose do `expresso-imip-dispatch`:

```yaml
environment:
  IMIP_ENABLED: "true"
  SMTP_HOST: expresso-postfix
  SMTP_PORT: "25"
  SMTP_STARTTLS: "false"
  SMTP_FROM: "calendar@expresso.local"
```

Postfix interno (rede `expresso_default`) aceita relay sem SASL pois
`mynetworks` cobre `172.16.0.0/12`. Verificação:

```bash
# dentro da rede
docker run --rm --network expresso_default curlimages/curl -sS \
  http://expresso-imip-dispatch:9192/metrics | grep imip_dispatch_total
```

Publicar evento teste:

```bash
docker run --rm --network expresso_default natsio/nats-box:latest \
  nats --server nats://expresso-nats:4222 pub expresso.imip.request '{...}'
```

Envelope wire (campos obrigatórios):

```json
{
  "method": "REQUEST",
  "invite": {
    "uid": "...",
    "summary": "...",
    "dtstart": "2026-05-01T14:00:00Z",
    "dtend":   "2026-05-01T15:00:00Z",
    "organizer_email": "calendar@expresso.local",
    "attendees": [{"email": "...", "common_name": "...", "rsvp": true}]
  }
}
```

`method` uppercase `REQUEST`|`CANCEL`. Fonte enum:
[services/expresso-imip-dispatch/src/main.rs](services/expresso-imip-dispatch/src/main.rs).

## 6. Rollback rápido

Todas as imagens fase2 preservam tag `:pre-fase2`. Rollback:

```bash
docker tag expresso-chat:pre-fase2 expresso-chat:latest
docker compose -f compose-phase3.yaml up -d --force-recreate expresso-chat
```

Repetir para `expresso-meet`, `expresso-calendar`, `expresso-auth` conforme
necessário.

## 7. Referências

- [ops/deploy-fase2-notes.md](ops/deploy-fase2-notes.md) — deploy notes detalhadas
- [docs/ROADMAP.md](docs/ROADMAP.md) — contexto estratégico
- Issue #40c — multi-realm trilha
- Issue #41 — iMIP dispatch

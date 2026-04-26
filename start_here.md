# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #312 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#308–#312)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #308 | mail | `Last-Modified` + `If-Modified-Since` em `GET /mail/messages` list — MAX(received_at) antes do keyset |
| #309 | mail | `Last-Modified` + `If-Modified-Since` em `GET /mail/threads/:id` — MAX(received_at) já calculado |
| #310 | compliance | `Last-Modified` + `If-Modified-Since` em `GET /compliance/archive` list — MAX(archived_at) |
| #311 | compliance | `If-Modified-Since` → 304 em `GET /compliance/archive/:id` — LM já existia, faltava IMS |
| #312 | flows | `Last-Modified` + `If-Modified-Since` em `GET /flows/rules` list — MAX(updated_at) |

**Estado atual:** caching HTTP completo em contacts/calendar/chat/mail/flows/compliance.

---

## Próximos candidatos (por ordem de prioridade)

1. **mail: `GET /mail/messages/:id` (get_message) — If-Modified-Since check** *(já tem ETag + LM + INM; falta IMS)*
   - `received_at` imutável; verificar se já tem IMS no handler

2. **IMAP: LIST-EXTENDED RETURN STATUS (RFC 5258)**
   - Aguardar imap_types alpha

3. **IMAP: NAMESPACE (RFC 2342)**
   - Aguardar imap_types alpha

4. **mail: search endpoint (`GET /mail/search`) — Last-Modified + If-Modified-Since**
   - Verificar se tem campo temporal adequado

5. **notifications: testar Redis pub/sub cross-pod**

---

## Arquitetura resumida

| Serviço | Porta | Arquivo principal |
|---------|-------|-------------------|
| expresso-mail | 8001 | `services/expresso-mail/src/` |
| expresso-flows | 8005 | `services/expresso-flows/src/main.rs` |
| expresso-compliance | 8009 | `services/expresso-compliance/src/main.rs` |
| expresso-notifications | 8007 | `services/expresso-notifications/src/` |
| expresso-contacts | 8003 | `services/expresso-contacts/src/` |
| expresso-calendar | 8004 | `services/expresso-calendar/src/` |
| expresso-chat | 8006 | `services/expresso-chat/src/` |
| expresso-imap | 993/143 | `services/expresso-imap/src/` |
| expresso-smtp | 25/587 | `services/expresso-smtp/src/` |

**Shared libs:** `expresso-core` (DB pool, migrations, AppConfig), `expresso-auth-client` (OIDC/JWT), `expresso-observability` (Prometheus metrics router)

---

## Padrões usados recorrentemente

```rust
// MAX(field) + IMS check antes do rows query (list handlers)
let max_ts: Option<OffsetDateTime> = sqlx::query_scalar(
    "SELECT MAX(updated_at) FROM tbl WHERE tenant_id = $1 AND user_id = $2",
)
.bind(ctx.tenant_id).bind(ctx.user_id)
.fetch_one(&pool).await.unwrap_or(None);

if let Some(ts) = max_ts {
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if ts <= ims_dt { return Ok(StatusCode::NOT_MODIFIED.into_response()); }
            }
        }
    }
}
// ... rows query ...
let mut resp = (...).into_response();
if let Some(ts) = max_ts {
    let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
}

// ETag + LM + IMS em get_one
let etag = format!("\"{}-{}\"", resource.updated_at.unix_timestamp(), resource.id);
if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
    if inm.as_bytes() == etag.as_bytes() { return Ok(StatusCode::NOT_MODIFIED.into_response()); }
}
let lm = resource.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
    if let Ok(ims_str) = ims_val.to_str() {
        if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
            if resource.updated_at <= ims_dt { return Ok(StatusCode::NOT_MODIFIED.into_response()); }
        }
    }
}
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

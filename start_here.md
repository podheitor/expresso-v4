# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #320 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#313–#320)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #313 | mail | `Last-Modified` + `If-Modified-Since` em `GET /api/v1/mail/search` — MAX(received_at) com mesmos filtros |
| #314 | mail | `Last-Modified` + `If-Modified-Since` em `GET /mail/vacation` e `GET /mail/sieve` — `updated_at` |
| #315 | calendar | `Last-Modified` + `If-Modified-Since` em `GET /calendars/:id/acl` — MAX(created_at) |
| #316 | contacts,calendar | `Last-Modified` + `If-Modified-Since` em `GET /addressbooks/:id/acl` e `GET /calendars/:id/events/:id/attendees` |
| #317 | drive | `Last-Modified` + `If-Modified-Since` em `GET /drive/files`, `/drive/files/:id/metadata`, `/drive/trash`, `/drive/files/:id/versions` |
| #318 | admin | `Last-Modified` + `If-Modified-Since` em `GET /api/v1/audit` — MAX(created_at) com mesmos filtros |
| #319 | drive | `Last-Modified` + `If-Modified-Since` em `GET /drive/files/:id/shares` — MAX(created_at) |
| #320 | mail | `If-Modified-Since` → 304 em `GET /mail/messages/:id/flags` — `received_at` imutável; fechou LM/IMS=31/31 |

**Estado atual:** caching HTTP COMPLETO — LM/IMS 31/31 em todos os serviços.

---

## Próximos candidatos

1. **IMAP: LIST-EXTENDED RETURN STATUS (RFC 5258)**
   - Aguardar imap_types alpha

2. **IMAP: NAMESPACE (RFC 2342)**
   - Aguardar imap_types alpha

3. **notifications: testar Redis pub/sub cross-pod**

4. **drive: ETag em `/drive/files/:id/metadata`** — usar hash de `updated_at + id`

5. **drive: HEAD `/drive/files/:id`** — para checagem leve de existência + ETag

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
| expresso-drive | 8008 | `services/expresso-drive/src/` |
| expresso-admin | 8010 | `services/expresso-admin/src/` |
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

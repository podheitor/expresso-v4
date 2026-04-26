# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #322 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#321–#322)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #321 | drive | `ETag` + `If-None-Match` em `GET /drive/files/:id/metadata` — `updated_at + id` hash |
| #322 | drive | `HEAD /drive/files/:id` — ETag + Last-Modified + Content-Length; INM + IMS → 304 |

**Estado atual:** caching HTTP COMPLETO — todos os candidatos implementados. Próximos: IMAP (bloqueados) e features novas.

---

## Sprints anteriores (#313–#320)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #313 | mail | `Last-Modified` + `If-Modified-Since` em `GET /api/v1/mail/search` — MAX(received_at) |
| #314 | mail | `Last-Modified` + `If-Modified-Since` em `GET /mail/vacation` e `GET /mail/sieve` |
| #315 | calendar | `Last-Modified` + `If-Modified-Since` em `GET /calendars/:id/acl` — MAX(created_at) |
| #316 | contacts,calendar | LM+IMS em `GET /addressbooks/:id/acl` e `GET /calendars/:id/events/:id/attendees` |
| #317 | drive | LM+IMS em `GET /drive/files`, `/drive/files/:id/metadata`, `/drive/trash`, `/drive/files/:id/versions` |
| #318 | admin | LM+IMS em `GET /api/v1/audit` — MAX(created_at) com mesmos filtros |
| #319 | drive | LM+IMS em `GET /drive/files/:id/shares` — MAX(created_at) |
| #320 | mail | IMS → 304 em `GET /mail/messages/:id/flags` — fechou LM/IMS=31/31 |

---

## Próximos candidatos

1. **IMAP: LIST-EXTENDED RETURN STATUS (RFC 5258)**
   - Aguardar imap_types alpha

2. **IMAP: NAMESPACE (RFC 2342)**
   - Aguardar imap_types alpha

3. **notifications: testar Redis pub/sub cross-pod**

4. **features novas** — a definir pelo user

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

// ETag + LM + IMS em get_one / metadata
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
// HEAD handler — mesmo padrão mas resp = StatusCode::OK.into_response() sem body
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

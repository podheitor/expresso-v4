# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #301 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#298–#301)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #298 | chat | `Last-Modified` + `If-Modified-Since` em `GET /api/v1/channels/:id` — ETag já existia |
| #299 | contacts | `Last-Modified` + `If-Modified-Since` em `GET /addressbooks/:id/contacts/:id` — usa `Response::builder()` |
| #300 | contacts | `If-Modified-Since` → 304 em `GET /addressbooks/:id/contacts` — check antes do list completo |
| #301 | calendar | `If-Modified-Since` → 304 em `GET /calendars/:id/events` — adicionado `HeaderMap` ao import |

---

## Próximos candidatos (por ordem de prioridade)

1. **chat: `GET /api/v1/channels/:id/messages` — Last-Modified + If-Modified-Since**
   - `list` provavelmente já tem X-Total-Count; verificar se tem LM; adicionar IMS → 304

2. **calendar: `GET /calendars/:id/events/:id` — Last-Modified + If-Modified-Since**
   - `get_one` já tem ETag (`ev.etag`); `Event` tem `updated_at` → emitir LM + check IMS

3. **contacts: `GET /addressbooks` — If-Modified-Since → 304**
   - `list` já tem LM (sprint #293); adicionar IMS check antes do SELECT completo

4. **calendar: `GET /calendars` — If-Modified-Since → 304**
   - `list` já tem LM (sprint #294); mesmo padrão

5. **chat: `GET /api/v1/channels` — If-Modified-Since → 304**
   - `list` já tem LM (sprint #295); mesmo padrão

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
// X-Total-Count + Last-Modified num único query (list)
let (total, max_updated): (i64, Option<OffsetDateTime>) = sqlx::query_as(
    "SELECT COUNT(*), MAX(updated_at) FROM tbl WHERE tenant_id = $1 AND ..."
).bind(...).fetch_one(pool).await?;
let mut resp = ([(header::HeaderName::from_static("x-total-count"), total.to_string())], Json(rows)).into_response();
if let Some(ts) = max_updated {
    let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
}

// ETag + Last-Modified em get_one (updated_at como base)
let etag = format!("\"{}-{}\"", resource.updated_at.unix_timestamp(), resource.id);
// If-None-Match → 304 check
let lm = resource.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
let mut resp = Json(resource).into_response();
resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());

// If-Modified-Since → 304 em get_one / list
if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
    if let Ok(ims_str) = ims_val.to_str() {
        if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
            if resource.updated_at <= ims_dt {
                return Ok(StatusCode::NOT_MODIFIED.into_response());
            }
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

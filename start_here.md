# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #307 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#302–#307)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #302 | calendar | `Last-Modified` + `If-Modified-Since` em `GET /calendars/:id/events/:id` — `ev.updated_at` |
| #303 | contacts | `If-Modified-Since` → 304 em `GET /addressbooks` list — check antes do list completo |
| #304 | calendar | `If-Modified-Since` → 304 em `GET /calendars` list — adicionou `HeaderMap` ao import |
| #305 | chat | `If-Modified-Since` → 304 em `GET /channels` list — adicionou `HeaderMap` ao import |
| #306 | contacts | `If-Modified-Since` → 304 em `GET /addressbooks/:id` get_one — LM já existia |
| #307 | calendar | `If-Modified-Since` → 304 em `GET /calendars/:id` get_one — LM já existia |

**Estado atual:** todos os handlers de contacts/calendar/chat têm LM=IMS=2 (list + get_one cada).

---

## Próximos candidatos (por ordem de prioridade)

1. **mail: `GET /api/v1/messages` (list) — If-Modified-Since → 304**
   - `messages.rs` já tem LM em line ~571 e ~910; verificar se tem IMS no list handler

2. **mail: folders/threads — Last-Modified + If-Modified-Since**
   - Verificar `GET /folders`, `GET /folders/:id`, `GET /threads` — quais têm LM/IMS

3. **flows: `GET /api/v1/flows` + `GET /flows/:id` — Last-Modified + If-Modified-Since**
   - Verificar handlers em expresso-flows

4. **compliance: `GET /api/v1/archive` — If-Modified-Since → 304**
   - Verificar handlers em expresso-compliance

5. **IMAP: LIST-EXTENDED RETURN STATUS (RFC 5258)**
   - Aguardar imap_types alpha

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
// If-Modified-Since → 304 ANTES do SELECT completo
if let Some(ts) = max_updated {
    if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(ims_str) = ims_val.to_str() {
            if let Ok(ims_dt) = OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
                if ts <= ims_dt { return Ok(StatusCode::NOT_MODIFIED.into_response()); }
            }
        }
    }
}
let rows = Repo::new(pool).list(...).await?;
let mut resp = ([(header::HeaderName::from_static("x-total-count"), total.to_string())], Json(rows)).into_response();
if let Some(ts) = max_updated {
    let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
}

// ETag + Last-Modified + If-Modified-Since em get_one
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
let mut resp = Json(resource).into_response();
resp.headers_mut().insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #297 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#293–#297)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #293 | contacts | `Last-Modified` em `GET /api/v1/addressbooks` — COUNT + MAX(updated_at) num único query |
| #294 | calendar | `Last-Modified` em `GET /api/v1/calendars` — mesmo padrão |
| #295 | chat | `Last-Modified` em `GET /api/v1/channels` — MAX(c.updated_at) com JOIN members + is_archived=FALSE |
| #296 | contacts | `Last-Modified` em `GET /api/v1/addressbooks/:id` — get_one já tinha ETag; adicionado LM = updated_at |
| #297 | calendar | `Last-Modified` em `GET /api/v1/calendars/:id` — mesmo padrão |

---

## Próximos candidatos (por ordem de prioridade)

1. **chat: `GET /api/v1/channels/:id` — Last-Modified**
   - `get_one` já tem ETag de `updated_at`; emitir também `Last-Modified` = `ch.updated_at` (Rfc2822)
   - Capturar `ch.updated_at` antes de `Json(ch)` consumir o valor

2. **contacts: `GET /addressbooks/:id/contacts/:id` — Last-Modified**
   - `get_one` já tem ETag (`c.etag`); `Contact` tem `updated_at` → emitir `Last-Modified`
   - Handler usa `Response::builder()` — adicionar `.header(header::LAST_MODIFIED, lm)`

3. **contacts: `GET /addressbooks/:id/contacts` — If-Modified-Since**
   - `list` já tem X-Total-Count + Last-Modified; adicionar `If-Modified-Since` → 304
   - Comparar MAX(updated_at) ≤ IMS → 304

4. **calendar: `GET /calendars/:id/events` — If-Modified-Since**
   - `list` já tem X-Total-Count + Last-Modified; adicionar `If-Modified-Since` → 304

5. **chat: `GET /api/v1/channels/:id` — If-Modified-Since**
   - `get_one` terá LM após #298; adicionar `If-Modified-Since` → 304

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

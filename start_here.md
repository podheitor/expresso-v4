# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #292 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#288–#292)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #288 | contacts | ETag + If-None-Match em `GET /api/v1/addressbooks/:id`; ETag=`"{updated_at_unix}-{id}"` |
| #289 | calendar | ETag + If-None-Match em `GET /api/v1/calendars/:id`; mesmo padrão |
| #290 | chat | ETag + If-None-Match em `GET /api/v1/channels/:id`; mesmo padrão |
| #291 | contacts | `Last-Modified` em `GET /addressbooks/:id/contacts` — MAX(updated_at) num único query com COUNT |
| #292 | calendar | `Last-Modified` em `GET /calendars/:id/events` — MAX(updated_at) mesmos filtros from/to |

---

## Próximos candidatos (por ordem de prioridade)

1. **contacts: `GET /api/v1/addressbooks` — Last-Modified**
   - `list` já tem X-Total-Count; adicionar MAX(updated_at) da tabela `addressbooks` tenant+user

2. **calendar: `GET /api/v1/calendars` — Last-Modified**
   - `list` já tem X-Total-Count; adicionar MAX(updated_at) da tabela `calendars` tenant+user

3. **chat: `GET /api/v1/channels` — Last-Modified**
   - `list` já tem X-Total-Count; MAX(c.updated_at) com JOIN chat_channel_members + is_archived=FALSE

4. **contacts: `GET /api/v1/addressbooks/:id` — Last-Modified**
   - `get_one` já tem ETag (updated_at); emitir também `Last-Modified` = updated_at (Rfc2822)

5. **calendar: `GET /api/v1/calendars/:id` — Last-Modified**
   - `get_one` já tem ETag (updated_at); emitir também `Last-Modified` = updated_at (Rfc2822)

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
// X-Total-Count em handler
let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tbl WHERE tenant_id = $1 AND ...").bind(...).fetch_one(pool).await?;
Ok(([(header::HeaderName::from_static("x-total-count"), total.to_string())], Json(rows)).into_response())

// X-Total-Count + Last-Modified num único query (list com aggregate)
let (total, max_updated): (i64, Option<OffsetDateTime>) = sqlx::query_as(
    "SELECT COUNT(*), MAX(updated_at) FROM tbl WHERE tenant_id = $1 AND ..."
).bind(...).fetch_one(pool).await?;
// ... build resp, then:
if let Some(ts) = max_updated {
    let lm = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
    resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
}

// ETag + If-None-Match/304 em get_one
let etag = format!("\"{}-{}\"", resource.updated_at.unix_timestamp(), resource.id);
if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
    if inm.as_bytes() == etag.as_bytes() {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
}

// Last-Modified em get_one (junto com ETag)
let lm = resource.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
resp.headers_mut().insert(header::LAST_MODIFIED, HeaderValue::from_str(&lm).unwrap());
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

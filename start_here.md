# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #287 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#283–#287)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #283 | contacts | `X-Total-Count` em `GET /api/v1/addressbooks` — lista de address books |
| #284 | calendar | `X-Total-Count` em `GET /api/v1/calendars` — lista de calendários |
| #285 | chat | `X-Total-Count` em `GET /api/v1/channels` — JOIN com chat_channel_members + is_archived=FALSE |
| #286 | calendar | `If-None-Match` + 304 em `GET /api/v1/calendars/:id/events/:id` — get_one já enviava ETag |
| #287 | contacts | `If-None-Match` + 304 em `GET /api/v1/addressbooks/:id/contacts/:id` — get_one já enviava ETag |

---

## Próximos candidatos (por ordem de prioridade)

1. **contacts: `GET /api/v1/addressbooks/:id` — ETag + If-None-Match**
   - `get_one` retorna `Json<Addressbook>`; sem ETag; `Addressbook` tem `updated_at` → ETag = `"{updated_at_unix}-{id}"`
   - Retornar `Response` com header `ETag`; check `If-None-Match` → 304

2. **calendar: `GET /api/v1/calendars/:id` — ETag + If-None-Match**
   - `get_one` retorna `Json<Calendar>`; sem ETag; `Calendar` tem `updated_at` → ETag = `"{updated_at_unix}-{id}"`
   - Mesmo padrão que addressbooks

3. **chat: `GET /api/v1/channels/:id` — ETag + If-None-Match**
   - `get_one` retorna `Json<Channel>`; `Channel` tem `updated_at` → ETag = `"{updated_at_unix}-{id}"`

4. **contacts: `GET /api/v1/addressbooks/:id/contacts` — Last-Modified**
   - List já tem X-Total-Count; considerar `Last-Modified` = MAX(updated_at) dos contatos

5. **calendar: `GET /api/v1/calendars/:id/events` — Last-Modified**
   - List já tem X-Total-Count; considerar `Last-Modified` = MAX(updated_at) dos eventos

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
// X-Total-Count em handler sem pool-own
let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tbl WHERE tenant_id = $1 AND ...").bind(...).fetch_one(pool).await?;
Ok(([(header::HeaderName::from_static("x-total-count"), total.to_string())], Json(rows)).into_response())

// X-Total-Count em compliance (usa &st.db, map_err)
let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tbl WHERE tenant_id = $1")
    .bind(ctx.tenant_id).fetch_one(&st.db).await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e.to_string()}))))?;

// ETag + If-None-Match/304
let etag = format!("\"{}-{}\"", ts.unix_timestamp(), id);
if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
    if inm.as_bytes() == etag.as_bytes() {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
}

// Last-Modified
let last_modified = ts.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();

// ETag de flags (mutáveis, sem timestamp)
flags.sort_unstable();
let etag = format!("\"{}\"", flags.join(","));

// Location header em POST
let location = format!("/api/v1/{resource}/{}", created.id);
Ok((StatusCode::CREATED, [(header::LOCATION, location)], Json(created)))

// ETag de recurso com updated_at (sem campo etag dedicado)
let etag = format!("\"{}-{}\"", resource.updated_at.unix_timestamp(), resource.id);
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

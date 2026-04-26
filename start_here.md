# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #282 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#278–#282)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #278 | api | `X-Total-Count` em `GET /mail/folders/all` — pastas incluindo não subscritas |
| #279 | contacts | `X-Total-Count` em `GET /api/v1/addressbooks/:id/contacts` |
| #280 | api | `Last-Modified` em `GET /mail/messages/:id/flags` — via `received_at` imutável |
| #281 | calendar | `X-Total-Count` em `GET /api/v1/calendars/:id/events` — com filtros from/to |
| #282 | compliance | `X-Total-Count` em `GET /compliance/retention-policies` |

---

## Próximos candidatos (por ordem de prioridade)

1. **contacts: `GET /api/v1/addressbooks` — X-Total-Count**
   - Lista de address books do usuário; verificar se já tem; adicionar se não tiver

2. **calendar: `GET /api/v1/calendars` — X-Total-Count**
   - Lista de calendários do usuário; mesmo padrão

3. **chat: `GET /api/v1/channels` — X-Total-Count**
   - Handler `list_channels` em `services/expresso-chat/src/api/channels.rs`

4. **contacts: `GET /api/v1/addressbooks/:id/contacts` — ETag em list**
   - Atualmente lista sem ETag; considerar ETag de aggregate (MAX etag) se viável

5. **calendar: `GET /api/v1/calendars/:id/events/:uid` — ETag + If-None-Match**
   - `get_one` já retorna ETag via header; verificar se lida com If-None-Match (304) ou só envia ETag

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
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

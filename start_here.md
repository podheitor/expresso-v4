# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #277 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#273–#277)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #273 | api | `X-Total-Count` em `GET /mail/folders` — COUNT(*) em query separada antes do SELECT |
| #274 | api | `PATCH /mail/messages/:id/move` retorna `200 + Json(MessageDetail)` em vez de `204` |
| #275 | flows | `Location: /api/v1/flows/rules/{id}` header em `POST /api/v1/flows/rules` |
| #276 | compliance | ETag + Last-Modified em `GET /compliance/retention-policies/:id` — ETag = `"{created_at_unix}-{id}"` |
| #277 | api | ETag + If-None-Match em `GET /mail/messages/:id/flags` — ETag = flags sorted e joined |

---

## Próximos candidatos (por ordem de prioridade)

1. **api: `GET /mail/folders/all` — X-Total-Count**
   - Mesmo padrão do #273, mas para o endpoint `/mail/folders/all` (inclui pastas não subscritas)

2. **compliance: `GET /compliance/archive` — X-Total-Count**
   - Lista paginada de arquivo; adicionar COUNT(*) com os mesmos filtros como `x-total-count`

3. **api: `GET /mail/messages/:id/flags` — Last-Modified**
   - Atualmente só tem ETag; adicionar `Last-Modified` baseado em `received_at` (proxy para quando flags foram atualizadas pela última vez) ou omitir se não houver campo adequado

4. **flows: `GET /api/v1/flows/rules` — X-Total-Count**
   - Atualmente retorna lista com x-total-count (verificar se já tem); se não, adicionar

5. **api: `PATCH /mail/messages/:id/flags` — retornar flags atualizadas**
   - Atualmente retorna `200 + Json(msg.flags)` (verificar); uniformizar se necessário

---

## Arquitetura resumida

| Serviço | Porta | Arquivo principal |
|---------|-------|-------------------|
| expresso-mail | 8001 | `services/expresso-mail/src/` |
| expresso-flows | 8005 | `services/expresso-flows/src/main.rs` |
| expresso-compliance | 8009 | `services/expresso-compliance/src/main.rs` |
| expresso-notifications | 8007 | `services/expresso-notifications/src/` |
| expresso-imap | 993/143 | `services/expresso-imap/src/` |
| expresso-smtp | 25/587 | `services/expresso-smtp/src/` |

**Shared libs:** `expresso-core` (DB pool, migrations, AppConfig), `expresso-auth-client` (OIDC/JWT), `expresso-observability` (Prometheus metrics router)

---

## Padrões usados recorrentemente

```rust
// X-Total-Count
let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ... WHERE ...").bind(...).fetch_one(&mut *tx).await?;
Ok(([(header::HeaderName::from_static("x-total-count"), total.to_string())], Json(rows)).into_response())

// ETag + If-None-Match/304
let etag = format!("\"{}-{}\"", ts.unix_timestamp(), id);
if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
    if inm.as_bytes() == etag.as_bytes() {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
}

// If-Modified-Since/304
if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
    if let Ok(ims_str) = ims_val.to_str() {
        if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
            if ts <= ims_dt {
                return Ok(StatusCode::NOT_MODIFIED.into_response());
            }
        }
    }
}

// ETag de flags (flags mutáveis, sem timestamp de update)
flags.sort_unstable();
let etag = format!("\"{}\"", flags.join(","));

// Location header em POST de criação
let location = format!("/api/v1/{resource}/{}", created.id);
Ok((StatusCode::CREATED, [(header::LOCATION, location)], Json(created)))

// ILIKE filter
let esc = s.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
format!("AND col ILIKE '%{esc}%'")
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

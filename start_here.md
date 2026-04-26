# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #272 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#268–#272)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #268 | api | `If-Modified-Since` + `Last-Modified` em `GET /mail/messages/:id` — `received_at` imutável como Last-Modified |
| #269 | api | ETag + If-None-Match em `GET /mail/threads/:id` — ETag = `MAX(received_at)` do thread |
| #270 | api | `DELETE /mail/messages/bulk` — novo endpoint dedicado com body `{"ids":[…]}`; retorna `{"affected":N}` |
| #271 | api | ETag + If-None-Match em `GET /mail/messages/:id/raw` — ETag = `"{size_bytes}-{id}"` (imutável) |
| #272 | compliance | ETag + Last-Modified em `GET /compliance/archive/:id` — ETag = `"{archived_at_unix}-{id}"` |

---

## Próximos candidatos (por ordem de prioridade)

1. **api: `GET /mail/folders` — X-Total-Count**
   - Atualmente retorna apenas a lista; adicionar `COUNT(*)` como header `x-total-count`

2. **api: `PATCH /mail/messages/:id/move` — retornar mensagem atualizada**
   - Atualmente retorna `204 NO_CONTENT`; mudar para `200 + Json(MessageDetail)` com novo `mailbox_id`

3. **flows: `POST /api/v1/flows/rules` — retornar `201 + Location` header**
   - Atualmente retorna `201 + Json(rule)` mas sem `Location: /api/v1/flows/rules/{id}` header

4. **compliance: `GET /compliance/retention-policies/:id` — ETag + Last-Modified**
   - Mesmo padrão que archive/:id; ETag = `"{created_at_unix}-{id}"` (policies não mudam `updated_at`)

5. **api: `GET /mail/messages/:id/flags` — ETag + If-None-Match**
   - Flags mudam; ETag baseado em hash das flags ou timestamp do último update

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
Ok((StatusCode::OK,
    [(header::HeaderName::from_static("x-total-count"), total.to_string())],
    Json(rows)).into_response())

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

// ILIKE filter
let esc = s.replace('\'', "''").replace('%', "\\%").replace('_', "\\_");
format!("AND col ILIKE '%{esc}%'")

// jsonb text array filter
format!("AND EXISTS (SELECT 1 FROM jsonb_array_elements_text(col) t WHERE t ILIKE '%{esc}%')")

// Numeric range filter
params.size_min.map(|v| format!("AND size_bytes >= {v}")).unwrap_or_default()

// Bulk DELETE body
#[derive(Debug, Deserialize)]
struct BulkDeleteRequest { ids: Vec<Uuid> }
// DELETE /route — body via Json extractor
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

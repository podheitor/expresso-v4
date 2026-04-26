# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #267 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#263–#267)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #263 | api | `cc_addr` ILIKE em `GET /mail/messages` e `GET /mail/search` — `jsonb_array_elements_text(m.cc_addrs)`; também adicionado `size_min`/`size_max` faltantes em `ListParams` |
| #264 | api | `PATCH /mail/messages/:id/flags` retorna `200 + Json(flags)` em vez de `204 NO_CONTENT` |
| #265 | api | `POST /mail/messages/bulk` — novos arms `mark_read` / `mark_unread` no enum `BulkRequest` |
| #266 | flows | `If-Modified-Since` em `GET /api/v1/flows/rules/:id` — complemento ao If-None-Match (#254) |
| #267 | compliance | sort param `asc`/`desc` em `GET /compliance/retention-policies` (antes fixo ASC) |

---

## Próximos candidatos (por ordem de prioridade)

1. **api: `GET /mail/messages/:id` — If-Modified-Since**
   - Complemento ao ETag/If-None-Match (#261); `received_at` é imutável então Last-Modified = received_at; comparar com `If-Modified-Since` header

2. **api: `GET /mail/threads/:id` — ETag + If-None-Match**
   - Derivar ETag do `MAX(received_at)` das mensagens no thread; retornar 304 se coincidir

3. **compliance: `GET /compliance/archive` — X-Total-Count**
   - Já tem paginação keyset+offset mas não retorna contagem total; adicionar `COUNT(*) FILTER` com mesmos filtros

4. **flows: `GET /api/v1/flows/rules` — X-Total-Count**
   - Mesmo padrão; COUNT(*) com mesmos filtros sem paginação → header `x-total-count`

5. **api: `DELETE /mail/messages/bulk`**
   - Endpoint dedicado `DELETE /mail/messages/bulk` com body `{"ids":[…]}`; alternativa REST-pura ao `POST /bulk action:delete`

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
if req_headers.get(header::IF_NONE_MATCH).map(|v| v.as_bytes()) == Some(etag.as_bytes()) {
    return Ok(StatusCode::NOT_MODIFIED.into_response());
}

// If-Modified-Since/304
if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) {
    if let Ok(ims_str) = ims_val.to_str() {
        if let Ok(ims_dt) = time::OffsetDateTime::parse(ims_str, &time::format_description::well_known::Rfc2822) {
            if updated_at <= ims_dt {
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
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

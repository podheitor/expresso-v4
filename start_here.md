# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #328 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#321–#328)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #321 | drive | `ETag` + `If-None-Match` em `GET /drive/files/:id/metadata` |
| #322 | drive | `HEAD /drive/files/:id` — ETag + Last-Modified + Content-Length |
| #323 | search | `SearchHit` expõe `subject` + `from_addr` — elimina round-trip ao mail |
| #324 | search | `snippet` de body no `SearchHit` — body STORED + `SnippetGenerator` 200 chars |
| #325 | search | `bulk_delete` remove mensagens do índice Tantivy via `DELETE /index/:id` |
| #326 | admin | REST API `govbr_user_map` — GET/POST/DELETE `/api/v1/govbr/mappings` |
| #327 | meet | LM+IMS em `GET /meetings`, ETag+INM em `GET /meetings/:id` |
| #328 | meet | LM+IMS em `GET /meetings/:id/participants` |

---

## Próximos candidatos

1. **IMAP: LIST-EXTENDED RETURN STATUS (RFC 5258)** — aguardar imap_types alpha
2. **IMAP: NAMESPACE (RFC 2342)** — aguardar imap_types alpha
3. **notifications: testar Redis pub/sub cross-pod**
4. **search: integrar GET /mail/search com Tantivy** — atualmente usa SQL ILIKE
5. **scheduling_counter_proposals** — tabela existe sem API REST
6. **meet: ETag em GET /meetings/:id (já feito #327) + scheduling_counter REST**

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
| expresso-meet | 8011 | `services/expresso-meet/src/` |
| expresso-search | 8013 | `services/expresso-search/src/` |
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
.fetch_one(pool).await.unwrap_or(None);

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

// ETag + LM + IMS em get_one
let etag = format!("\"{}-{}\"", resource.updated_at.unix_timestamp(), resource.id);
if let Some(inm) = req_headers.get(header::IF_NONE_MATCH) {
    if inm.as_bytes() == etag.as_bytes() { return Ok(StatusCode::NOT_MODIFIED.into_response()); }
}
let lm = resource.updated_at.format(&time::format_description::well_known::Rfc2822).unwrap_or_default();
if let Some(ims_val) = req_headers.get(header::IF_MODIFIED_SINCE) { ... IMS check ... }
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

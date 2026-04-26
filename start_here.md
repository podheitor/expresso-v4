# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #334 (2026-04-26)

```
git log --oneline | head -10
```

---

## O que foi feito nesta sessão (#333–#334)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #333 | admin | Migration `updated_at` em `govbr_user_map` + trigger automático; `govbr.rs` usa `updated_at` direto para ETag/LM |
| #334 | search | `POST /api/v1/index/bulk` — indexa até 500 docs por chamada; um único commit Tantivy |

---

## Sessões anteriores (#329–#332)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #329 | calendar | REST API para COUNTER proposals — GET/GET/:id/POST accept/POST reject em `/api/v1/scheduling/counters` |
| #330 | mail | `GET /mail/search` integrado com Tantivy — fallback transparente para ILIKE |
| #331 | chat | Last-Modified + IMS em `GET /channels/:id/members` — MAX(joined_at) |
| #332 | admin | ETag + Last-Modified em `GET /govbr/mappings` e `GET /govbr/mappings/:cpf_hash` |

---

## Próximos candidatos

1. **IMAP: LIST-EXTENDED RETURN STATUS (RFC 5258)** — aguardar imap_types alpha
2. **IMAP: NAMESPACE (RFC 2342)** — aguardar imap_types alpha
3. **notifications: testar Redis pub/sub cross-pod** — ops concern
4. **compliance: LM em list_retention_policies** — só tem `created_at`, sem updated_at

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

// bulk_index (search)
// POST /api/v1/index/bulk  body: {"documents": [IndexDoc, ...]}  (max 500)
// resposta: {"indexed": N, "rejected": ["doc_id_invalido", ...]}

// COUNTER proposals accept (calendar)
let new_raw = itip::apply_proposed_times(&event.ical_raw, prop.proposed_dtstart, prop.proposed_dtend)?;
erepo.update(ctx.tenant_id, event.id, &new_raw).await?;
crepo.resolve(ctx.tenant_id, id, "accepted", Some(ctx.user_id)).await?;
```

---

## Instrução para continuar

```
execute all next phase tasks with no prompt.
```

Claude lê `memory/project_status.md` automaticamente e retoma do sprint seguinte.

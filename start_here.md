# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #345 (2026-04-27)

```
git log --oneline | head -15
```

---

## O que foi feito nesta sessão (#336–#345)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #336 | calendar | Last-Modified + IMS em `export.ics`; ETag + LM + INM + IMS em `itip/request.ics` |
| #337 | drive | Last-Modified + IMS em `GET /quota` via `MAX(updated_at)` drive_files |
| #338 | mail | ETag + INM em `HEAD /messages/:id/raw` — mesmo formato que GET /raw |
| #339 | mail | ETag + INM em `GET /messages/:id/attachments` — imutável como raw |
| #340 | mail | Last-Modified + IMS em `list_folders` e `list_all_folders` via `MAX(updated_at)` mailboxes |
| #341 | mail | Last-Modified + IMS em `GET /quota` via `MAX(received_at)` messages |
| #342 | search | Parâmetro `offset` em `GET /api/v1/search` — paginação via `TopDocs::and_offset()` |
| #343 | meet | `PATCH /api/v1/meetings/:id` — atualiza title/schedule/lobby/password (moderator-only) |
| #344 | drive | `PATCH /api/v1/drive/files/:id/metadata` — renomear arquivo/pasta sem reupload |
| #345 | drive | `POST /api/v1/drive/files/:id/move` — mover arquivo/pasta para outro diretório |

---

## Sessões anteriores (#333–#335)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #333 | admin | Migration `updated_at` em `govbr_user_map` + trigger automático; `govbr.rs` usa `updated_at` direto para ETag/LM |
| #334 | search | `POST /api/v1/index/bulk` — indexa até 500 docs por chamada; um único commit Tantivy |
| #335 | compliance | `Last-Modified` + `IMS` em `list_policies`; ETag/LM via `updated_at` em `get_policy` |

---

## Próximos candidatos

1. **IMAP: LIST-EXTENDED RETURN STATUS (RFC 5258)** — aguardar imap_types alpha
2. **IMAP: NAMESPACE (RFC 2342)** — aguardar imap_types alpha
3. **notifications: testar Redis pub/sub cross-pod** — ops concern
4. **drive: mover múltiplos arquivos em batch** — `POST /drive/files/bulk-move`
5. **meet: cancelar/reativar reunião arquivada** — `POST /meetings/:id/restore`

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

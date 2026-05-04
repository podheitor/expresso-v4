# Expresso v4 — Ponto de Retomada

**Último sprint:** #678 — (commit pendente)

```
git log --oneline | head -10
```

---

## Contexto rápido

Projeto Rust multi-tenant de mail/colaboração com 7 serviços:
- **expresso-mail** (`:8001`) — SMTP/IMAP/REST mail
- **expresso-calendar** (`:8002`) — CalDAV + REST; família `events-by-range/*` com 25+ endpoints
- **expresso-drive** (`:8003`) — armazenamento de arquivos, quotas, versões
- **expresso-notifications** (`:8006`) — SSE + DLQ + Redis pub/sub
- **expresso-search** (`:8007`) — Tantivy full-text search
- **expresso-meet** (`:8004`) — reuniões virtuais
- **expresso-contacts** (`:8005`) — agenda de contatos

Libs compartilhadas: `expresso-core` (DB pool, RLS tenant tx), `expresso-auth-client` (OIDC JWT), `expresso-observability` (Prometheus).

---

## Padrões estabelecidos (resumo)

- **Multi-tenant RLS**: `begin_tenant_tx(pool, tenant_id)` → `SET LOCAL app.tenant_id`
- **Stats queries**: `COUNT(*) FILTER (WHERE ...)::BIGINT`, `COALESCE(SUM,0)`, `DATE_TRUNC + to_char`
- **Optional temporal bounds**: `$N::timestamptz IS NULL OR col >= $N` (prepared statement estático)
- **Tri-state patch**: `Option<Option<T>>` — None=preserve, Some(None)=clear, Some(Some(v))=set
- **Route ordering**: rota estática antes de wildcard (`/segments/count` antes de `/segments/{id}`)
- **Null-over-404**: `{entry: null}` / `{segment: null}` para peek em coleções possivelmente vazias
- **Best-effort side-effects**: DLQ, Redis, webhooks — sempre 2xx, falha parcial não aborta
- **SELECT explícito > SELECT \*** em `query_as` com `FromRow`
- **Subquery pattern para stats por thread**: `MIN(received_at) per thread_id` → outer `DATE_TRUNC + COUNT`
- **8-FILTER size-bucket**: COUNT+SUM FILTER sem GROUP BY para faixas fixas

---

## Sprints recentes (#658–#666)

| Sprint | Serviço | Endpoint |
|--------|---------|----------|
| #658 | mail | `GET /mail/messages/stats/unread-by-folder` — total+unread por mailbox, unread DESC |
| #659 | search | `GET /search/index/segments/stats` — segment_count+total/largest/smallest_disk_bytes |
| #660 | calendar | `GET /calendars/:cal_id/events-by-range/attendee-count-stats` — avg/max+with/without (parse in-app) |
| #661 | notifications | `GET /dlq/stats/by-tenant-and-day?since=&until=` — COUNT GROUP BY (day, tenant_id) |
| #662 | drive | `GET /drive/files/stats/by-owner-and-ext?limit=N` — top-N (owner_user_id, extension) por total_bytes |
| #663 | mail | `GET /mail/messages/stats/size-by-folder` — total_bytes+avg_bytes+max_bytes por mailbox, total DESC |
| #664 | search | `GET /search/index/segments/age-stats` — min/max/avg num_docs por segmento |
| #665 | calendar | `GET /calendars/:cal_id/events-by-range/location-stats` — with/without_location + top-20 locations |
| #666 | notifications | `GET /dlq/stats/by-error-kind?since=&until=` — COUNT GROUP BY last_error, count DESC |
| #667 | drive | `GET /drive/files/stats/recent?since=&limit=N` — arquivos não-deletados ordenados por updated_at DESC |
| #668 | mail | `GET /mail/messages/stats/attachments-by-folder` — with/without_attachments+size_bytes por mailbox |
| #669 | search | `GET /search/index/segments/doc-distribution` — histograma tiny/small/medium/large por num_docs |
| #670 | calendar | `GET /calendars/:cal_id/events-by-range/organizer-stats` — with/without_organizer + top-20 organizers |
| #671 | notifications | `GET /dlq/stats/by-tenant?limit=N` — COUNT GROUP BY tenant_id ORDER BY count DESC |
| #672 | drive | `GET /drive/files/stats/mime-by-folder?folder_id=` — mime_type breakdown por pasta (parent_id ou raiz) |
| #673 | mail | `GET /mail/messages/stats/flags-by-folder` — LATERAL unnest GROUP BY (folder, flag) ORDER BY count DESC |
| #674 | search | `GET /search/index/segments/top-n?limit=N` — top-N segmentos por disk_bytes; default 5 max 50 |
| #675 | calendar | `GET /calendars/:cal_id/events-by-range/status-stats` — COUNT FILTER CONFIRMED/TENTATIVE/CANCELLED/other/unset |
| #676 | notifications | `GET /dlq/stats/by-kind?limit=N` — GROUP BY kind ORDER BY count DESC; rollup sem temporal |
| #677 | drive | `GET /drive/files/stats/top-files?limit=N` — top-N arquivos por size_bytes; default 20 max 200 |
| #678 | mail | `GET /mail/messages/stats/received-by-folder` — COUNT(*) por mailbox; LEFT JOIN pastas vazias; total DESC |

---

## Próximos candidatos (#679+)

1. **search** — `GET /search/index/segments/bottom-n?limit=N` — bottom-N segmentos por disk_bytes (candidatos a merge); sort ASC + truncate; simetria com top-n (#674)
2. **calendar** — `GET /calendars/stats/by-tenant` — COUNT calendars+events por tenant_id; `{rows:[{tenant_id,calendar_count,event_count}]}`; visão ops cross-calendar
3. **notifications** — `GET /dlq/stats/attempts-distribution` — histograma de `attempts` (1/2/3/4/5+); `{buckets:[{attempts,count}]}`; identifica entradas presas em retry loops
4. **drive** — `GET /drive/files/stats/created-by-day?since=&until=` — COUNT arquivos criados por dia; `{days:[{day,count}]}`; análogo a activity (#636) mas foca só em criação via drive_files.created_at

---

## Workflow de sprint

Cada sprint = 1 commit no formato:
```
feat(scope): descrição — sprint #N
```

Rotation: notifications → drive → mail → search → calendar (5 serviços, 3 sprints por "next").

Para continuar: diga **"vai"** ou **"next"**.

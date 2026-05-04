# Expresso v4 — Ponto de Retomada

**Último sprint:** #662 — `33c1f70`

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

## Sprints recentes (#650–#655)

| Sprint | Serviço | Endpoint |
|--------|---------|----------|
| #650 | notifications | `GET /dlq/stats/by-day?since=&until=` — timeline falhas DLQ por dia |
| #651 | drive | `GET /drive/files/stats/size-buckets` — distribuição <1MB/1–10MB/10–100MB/>100MB |
| #652 | calendar | `GET /calendars/:cal_id/events-by-range/class-stats?after=&before=` — COUNT FILTER por CLASS |
| #653 | mail | `GET /mail/messages/stats/threads-by-day?since=&until=` — threads iniciadas por dia (subquery MIN) |
| #654 | search | `GET /search/index/segments/smallest` — simetria com /largest; sort ASC |
| #655 | calendar | `GET /calendars/:cal_id/events-by-range/transp-stats?after=&before=` — COUNT FILTER por TRANSP |
| #656 | notifications | `GET /dlq/stats/by-kind-and-day?since=&until=` — COUNT GROUP BY (day, kind) |
| #657 | drive | `GET /drive/files/stats/deleted` — deleted_count, deleted_bytes, oldest/newest_deleted_at |
| #658 | mail | `GET /mail/messages/stats/unread-by-folder` — total+unread por mailbox, unread DESC |
| #659 | search | `GET /search/index/segments/stats` — segment_count+total/largest/smallest_disk_bytes |
| #660 | calendar | `GET /calendars/:cal_id/events-by-range/attendee-count-stats` — avg/max+with/without (parse in-app) |
| #661 | notifications | `GET /dlq/stats/by-tenant-and-day?since=&until=` — COUNT GROUP BY (day, tenant_id) |
| #662 | drive | `GET /drive/files/stats/by-owner-and-ext?limit=N` — top-N (owner_user_id, extension) por total_bytes |

---

## Próximos candidatos (#661+)

1. **mail** — `GET /mail/messages/stats/size-by-folder` — total+avg+max size_bytes por mailbox; `{folders:[{folder,total_bytes,avg_bytes,max_bytes}]}`
2. **search** — `GET /search/index/segments/age-stats` — min/max/avg num_docs por segmento; `{segment_count,min_docs,max_docs,avg_docs}`
3. **calendar** — `GET /calendars/:cal_id/events-by-range/location-stats?after=&before=` — eventos com/sem location + top-N locations; `{with_location,without_location,top_locations:[{location,count}]}`
4. **notifications** — `GET /dlq/stats/by-error-kind?since=&until=` — COUNT GROUP BY error_kind; `{rows:[{error_kind,count}]}`

---

## Workflow de sprint

Cada sprint = 1 commit no formato:
```
feat(scope): descrição — sprint #N
```

Rotation: notifications → drive → mail → search → calendar (5 serviços, 3 sprints por "next").

Para continuar: diga **"vai"** ou **"next"**.

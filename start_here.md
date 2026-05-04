# Expresso v4 — Ponto de Retomada

**Último sprint:** #655 — `cc63425`

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

---

## Próximos candidatos (#656+)

1. **notifications** — `GET /dlq/stats/by-kind-and-day?since=&until=` — DATE_TRUNC + COUNT GROUP BY day, kind; `{rows:[{day,kind,count}]}`
2. **drive** — `GET /drive/files/stats/deleted` — count+bytes de deleted_at IS NOT NULL; `{deleted_count, deleted_bytes, oldest_deleted_at, newest_deleted_at}`
3. **mail** — `GET /mail/messages/stats/unread-by-folder` — COUNT FILTER (NOT seen) por mailbox; `{folders:[{folder,total,unread}]}`
4. **search** — `GET /search/index/segments/stats` — consolida count+largest+smallest+total_disk_bytes numa resposta; evita 4 calls no dashboard
5. **calendar** — `GET /calendars/:cal_id/events-by-range/attendee-count-stats?after=&before=` — parse ical_raw in-app; `{avg_attendees, max_attendees, events_with_attendees, events_without_attendees}`

---

## Workflow de sprint

Cada sprint = 1 commit no formato:
```
feat(scope): descrição — sprint #N
```

Rotation: notifications → drive → mail → search → calendar (5 serviços, 3 sprints por "next").

Para continuar: diga **"vai"** ou **"next"**.

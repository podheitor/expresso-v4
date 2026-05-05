# Expresso v4 — Ponto de Retomada

**Último sprint:** #710 — (commit pendente)

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
| #679 | search | `GET /search/index/segments/bottom-n?limit=N` — sort ASC + truncate; simetria com top-n (#674); default 5 max 50 |
| #680 | calendar | `GET /calendars/stats/by-tenant` — COUNT DISTINCT calendars + events por tenant_id; ops cross-tenant |
| #681 | notifications | `GET /dlq/stats/attempts-distribution` — COUNT FILTER buckets 1/2/3/4/5+; identifica retry loops |
| #682 | drive | `GET /drive/files/stats/created-by-day?since=&until=` — DATE_TRUNC dia via created_at; kind='file' não-deletados |
| #683 | mail | `GET /mail/messages/stats/threads-by-folder` — COUNT DISTINCT thread_id + unread_thread_count por mailbox; LEFT JOIN pastas vazias |
| #684 | search | `GET /search/stats/by-tenant?limit=N` — docs_count_by_tenant (AllQuery scan); `{rows:[{tenant_id,doc_count}]}`; ops cross-tenant |
| #685 | calendar | `GET /calendars/:cal_id/events-by-range/priority-stats?after=&before=` — COUNT FILTER PRIORITY 0=undefined/1-4=high/5=medium/6-9=low |
| #686 | notifications | `GET /dlq/stats/by-user?limit=N` — COUNT GROUP BY user_id ORDER BY count DESC; análogo a by-tenant (#671) |
| #687 | drive | `GET /drive/files/stats/by-size-bucket?folder_id=` — 8 faixas <1KB…>1GB; COUNT + SUM FILTER; `folder_id` opcional |
| #688 | mail | `GET /mail/messages/stats/senders-by-folder` — top-20 from_addr por pasta; GROUP BY (folder, from_addr); BTreeMap truncate |
| #689 | search | `GET /search/index/segments/merge-candidates?min_docs=&max_docs=` — filtra segmentos por faixa num_docs ASC |
| #690 | calendar | `GET /calendars/:cal_id/events/class-distribution` — rollup total CLASS sem filtro temporal |
| #691 | notifications | `GET /dlq/stats/by-day-and-user?since=&until=` — GROUP BY (day, user_id) ASC; análogo a by-tenant-and-day (#661) |
| #692 | drive | `GET /drive/files/stats/updated-by-day?since=&until=` — DATE_TRUNC('day', updated_at); complementa created-by-day (#682) |
| #693 | mail | `GET /mail/messages/stats/date-by-folder` — MIN/MAX received_at + COUNT por folder; envelope temporal |
| #694 | search | `GET /search/index/segments/size-stats` — min/max/avg disk_bytes; análogo a age-stats (#664) |
| #695 | notifications | `GET /dlq/stats/retention?days=N` — COUNT entries com `failed_at < NOW() - N days`; oldest_failed_at |
| #696 | drive | `GET /drive/files/stats/folder-depth` — CTE recursiva histograma depth→count+total_bytes |
| #697 | mail | `GET /mail/messages/stats/cc-by-folder` — COUNT with_cc/without_cc por folder; LEFT JOIN |
| #698 | search | `GET /search/index/segments/doc-ratio` — num_docs/disk_bytes por segmento; docs_per_byte DESC |
| #699 | calendar | `GET /calendars/:cal_id/events-by-range/duration-distribution` — histograma <1h/1-4h/4-8h/8h-1d/>1d |
| #700 | notifications | `GET /dlq/stats/by-hour?since=&until=` — DATE_TRUNC('hour') GROUP BY; granularidade intra-dia |
| #701 | drive | `GET /drive/files/stats/version-count?limit=N` — top-N arquivos por version_count; JOIN drive_file_versions |
| #702 | mail | `GET /mail/messages/stats/bcc-by-folder` — with_bcc/without_bcc por mailbox; análogo a cc-by-folder (#697) |
| #703 | search | `GET /search/index/segments/overlap` — pares em mesma banda de num_docs (band param); merge candidates |
| #704 | calendar | `GET /calendars/:cal_id/events-by-range/recurrence-duration-stats` — avg/min/max/total minutos só rrule |
| #705 | notifications | `GET /dlq/stats/by-kind-and-user?limit=N` — GROUP BY (kind, user_id) COUNT DESC |
| #706 | drive | `GET /drive/files/stats/tag-count?limit=N` — top-N tags por file_count; GROUP BY drive_file_tags.tag |
| #707 | mail | `GET /mail/messages/stats/reply-rate-by-folder` — replies(in_reply_to IS NOT NULL)/non_replies por folder |
| #708 | search | `GET /search/index/segments/cumulative` — cumsum num_docs+disk_bytes por segmento ASC |
| #709 | calendar | `GET /calendars/stats/event-density?bucket=day|week|month` — cross-tenant DATE_TRUNC GROUP BY |
| #710 | notifications | `GET /dlq/stats/by-tenant-and-kind?limit=N` — GROUP BY (tenant_id, kind) COUNT DESC |

---

## Próximos candidatos (#711+)

1. **drive** — `GET /drive/files/stats/ext-by-folder?folder_id=` — breakdown de extensão por pasta (parent_id ou raiz); análogo a mime-by-folder (#672) mas por extensão
2. **mail** — `GET /mail/messages/stats/to-count-by-folder` — avg/max to_addrs por pasta; mede "fan-out" de mensagens
3. **search** — `GET /search/index/segments/percentile?p=N` — num_docs e disk_bytes no percentil N (0-100) via rank em memória
4. **calendar** — `GET /calendars/:cal_id/events-by-range/all-day-stats` — COUNT all-day events (dtstart sem hora = DATE type) vs timed

---

## Workflow de sprint

Cada sprint = 1 commit no formato:
```
feat(scope): descrição — sprint #N
```

Rotation: notifications → drive → mail → search → calendar (5 serviços, 3 sprints por "next").

Para continuar: diga **"vai"** ou **"next"**.

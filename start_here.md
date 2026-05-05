# Expresso v4 — Ponto de Retomada

**Último sprint:** #754 — (commit pendente)

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
| #711 | drive | `GET /drive/files/stats/ext-by-folder?folder_id=` — breakdown extensão por pasta; substring(name FROM '\.[^.]*$') |
| #712 | mail | `GET /mail/messages/stats/to-count-by-folder` — avg/max jsonb_array_length(to_addrs) por pasta |
| #713 | search | `GET /search/index/segments/percentile?p=N` — num_docs+disk_bytes no percentil N via rank em memória |
| #714 | calendar | `GET /calendars/:cal_id/events-by-range/all-day-stats` — all_day(time='00:00:00') vs timed |
| #715 | notifications | `GET /dlq/stats/by-day-and-kind?since=&until=` — GROUP BY (day, kind) ASC; análogo a by-tenant-and-day (#661) escopado por kind |
| #716 | drive | `GET /drive/files/stats/lock-count` — COUNT arquivos bloqueados (locked_at IS NOT NULL); total + by_user |
| #717 | mail | `GET /mail/messages/stats/subject-length-by-folder` — avg/max LENGTH(subject) por pasta; indica verbosidade |
| #718 | search | `GET /search/index/segments/stdev` — desvio padrão amostral (n-1) de num_docs e disk_bytes; medida de desbalanceamento |
| #719 | calendar | `GET /calendars/:cal_id/events-by-range/description-stats` — with/without + avg/max LENGTH(description) |
| #720 | notifications | `GET /dlq/stats/by-hour-and-kind?since=&until=` — GROUP BY (hour, kind) ASC; granularidade intra-dia por tipo |
| #721 | drive | `GET /drive/files/stats/starred-count` — COUNT starred_at IS NOT NULL; total + by_user DESC |
| #722 | mail | `GET /mail/messages/stats/preview-length-by-folder` — avg/max LENGTH(preview_text) por pasta |
| #723 | search | `GET /search/index/segments/entropy` — entropia de Shannon H=-sum(p*log2(p)) sobre num_docs |
| #724 | calendar | `GET /calendars/:cal_id/events-by-range/summary-length-stats` — with/without + avg/max LENGTH(summary) |
| #725 | notifications | `GET /dlq/stats/by-minute?since=&until=` — DATE_TRUNC('minute') GROUP BY; granularidade fina |
| #726 | drive | `GET /drive/files/stats/expiry-count` — total_with_expiry + already_expired (expires_at < NOW()) |
| #727 | mail | `GET /mail/messages/stats/has-date-by-folder` — with_date/without_date (date IS NOT NULL) por pasta |
| #728 | search | `GET /search/index/segments/gini` — coeficiente de Gini da distribuição num_docs |
| #729 | calendar | `GET /calendars/:cal_id/events-by-range/location-length-stats` — with/without + avg/max LENGTH(location) |
| #730 | notifications | `GET /dlq/stats/by-minute-and-kind?since=&until=` — GROUP BY (minute, kind) ASC |
| #731 | drive | `GET /drive/files/stats/mime-top-n?limit=N` — top-N mime_types por file_count global; default 20 max 100 |
| #732 | mail | `GET /mail/messages/stats/from-domain-by-folder` — top-20 domínios de remetente (SPLIT_PART('@')) por pasta |
| #733 | search | `GET /search/index/segments/iqr` — IQR Q1/Q3 de num_docs e disk_bytes; interpolação linear |
| #734 | calendar | `GET /calendars/:cal_id/events-by-range/uid-uniqueness` — total vs COUNT(DISTINCT uid); duplicate_entries |
| #735 | notifications | `GET /dlq/stats/by-kind-and-tenant?limit=N` — GROUP BY (kind, tenant_id) COUNT DESC; default 50 max 500 |
| #736 | drive | `GET /drive/files/stats/orphan-versions` — LEFT JOIN IS NULL; versões sem drive_files pai |
| #737 | mail | `GET /mail/messages/stats/in-reply-to-by-folder` — with/without in_reply_to; LEFT JOIN pastas vazias |
| #738 | search | `GET /search/index/segments/range` — min/max/range de num_docs e disk_bytes |
| #739 | calendar | `GET /calendars/:cal_id/events-by-range/attendee-domain-stats` — top-20 domínios de email dos attendees (parse in-app) |
| #740 | notifications | `GET /dlq/stats/by-tenant-and-hour?since=&until=` — GROUP BY (tenant_id, hour); granularidade intra-dia cross-tenant |
| #741 | drive | `GET /drive/files/stats/empty-files` — COUNT total_empty + null_size + zero_size (kind='file', não-deletados) |
| #742 | mail | `GET /mail/messages/stats/message-id-coverage` — with/without message_id; LEFT JOIN pastas vazias |
| #743 | search | `GET /search/index/segments/cv` — coeficiente de variação (stdev/mean) de num_docs e disk_bytes |
| #744 | calendar | `GET /calendars/:cal_id/events-by-range/overlap-count` — pares com dtstart/dtend sobrepostos via self-join |
| #745 | notifications | `GET /dlq/stats/by-kind-and-hour?since=&until=` — GROUP BY (kind, hour) ASC |
| #746 | drive | `GET /drive/files/stats/deleted-by-day?since=&until=` — DATE_TRUNC('day', deleted_at); kind='file' |
| #747 | mail | `GET /mail/messages/stats/body-size-by-folder` — avg/max size_bytes; LEFT JOIN pastas vazias |
| #748 | search | `GET /search/index/segments/skewness` — g1 sample skewness de num_docs e disk_bytes |
| #749 | calendar | `GET /calendars/:cal_id/events-by-range/sequence-stats` — avg/max sequence (revisões RFC 5545) |
| #750 | notifications | `GET /dlq/stats/by-error-prefix?limit=N` — top-N LEFT(last_error,60) por contagem |
| #751 | drive | `GET /drive/files/stats/name-length` — avg/max LENGTH(name) global; kind='file' não-deletados |
| #752 | mail | `GET /mail/messages/stats/reply-to-by-folder` — with/without reply_to; LEFT JOIN pastas vazias |
| #753 | search | `GET /search/index/segments/mad` — MAD (median absolute deviation) de num_docs e disk_bytes |
| #754 | calendar | `GET /calendars/:cal_id/events-by-range/organizer-domain-stats` — top-20 domínios de organizer_email (SQL SPLIT_PART) |

---

## Próximos candidatos (#755+)

1. **notifications** — `GET /dlq/stats/summary` — rollup global: total + by_kind counts num; snapshot de saúde DLQ
2. **drive** — `GET /drive/files/stats/ext-top-n?limit=N` — top-N extensões globais por file_count; complementa mime-top-n (#731)
3. **mail** — `GET /mail/messages/stats/thread-depth-by-folder` — avg/max thread size (msgs por thread_id) por pasta
4. **search** — `GET /search/index/segments/kurtosis` — curtose (g2 excess) de num_docs; distribuição de caudas
5. **calendar** — `GET /calendars/:cal_id/events-by-range/created-by-day` — DATE_TRUNC('day', created_at) COUNT

---

## Workflow de sprint

Cada sprint = 1 commit no formato:
```
feat(scope): descrição — sprint #N
```

Rotation: notifications → drive → mail → search → calendar (5 serviços, 3 sprints por "next").

Para continuar: diga **"vai"** ou **"next"**.

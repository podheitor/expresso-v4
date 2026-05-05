# Expresso v4 — Ponto de Retomada

**Último sprint:** #974

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
| #755 | notifications | `GET /dlq/stats/summary` — rollup global: total + by_kind |
| #756 | drive | `GET /drive/files/stats/ext-top-n?limit=N` — top-N extensões globais por file_count |
| #757 | mail | `GET /mail/messages/stats/thread-depth-by-folder` — avg/max msgs por thread_id por pasta |
| #758 | search | `GET /search/index/segments/kurtosis` — curtose g2 excess de num_docs e disk_bytes |
| #759 | calendar | `GET /calendars/:cal_id/events-by-range/created-by-day` — COUNT por DATE_TRUNC('day', created_at) |
| #760 | notifications | `GET /dlq/stats/by-tenant-and-kind-and-day` — 3D GROUP BY (day, tenant_id, kind) |
| #761 | drive | `GET /drive/files/stats/storage-by-user?limit=N` — top-N users por total_bytes |
| #762 | mail | `GET /mail/messages/stats/flags-summary` — cross-folder COUNT por flag via unnest |
| #763 | search | `GET /search/index/segments/trimmed-mean?pct=N` — média podada descartando top/bottom N% |
| #764 | calendar | `GET /calendars/:cal_id/events-by-range/updated-by-day` — COUNT por DATE_TRUNC('day', updated_at) |
| #765 | notifications | `GET /dlq/stats/by-day-and-tenant` — GROUP BY (day, tenant_id) ASC |
| #766 | drive | `GET /drive/files/stats/quota-usage` — max_bytes + used_bytes + pct_used por folder com quota |
| #767 | mail | `GET /mail/messages/stats/size-distribution` — histograma <1KB/1-10KB/10-100KB/100KB-1MB/>1MB |
| #768 | search | `GET /search/index/segments/harmonic-mean` — média harmônica de num_docs e disk_bytes |
| #769 | calendar | `GET /calendars/:cal_id/events-by-range/etag-collision-check` — total vs COUNT(DISTINCT etag) |
| #770 | notifications | `GET /dlq/stats/age-distribution` — buckets <1h/1-6h/6-24h/1-7d/>7d por NOW()-failed_at |
| #771 | drive | `GET /drive/files/stats/folder-file-count?limit=N` — top-N pastas por file_count |
| #772 | mail | `GET /mail/messages/stats/oldest-newest-by-folder` — MIN/MAX received_at por pasta |
| #773 | search | `GET /search/index/segments/geometric-mean` — média geométrica de num_docs e disk_bytes |
| #774 | calendar | `GET /calendars/:cal_id/events-by-range/no-end-count` — COUNT eventos sem dtend |
| #775 | notifications | `GET /dlq/stats/by-hour-and-tenant?since=&until=` — GROUP BY (hour, tenant_id) ASC |
| #776 | drive | `GET /drive/files/stats/deep-files?min_depth=N` — CTE recursiva depth >= min_depth; by_depth breakdown |
| #777 | mail | `GET /mail/messages/stats/references-count-by-folder` — with/without references_ array |
| #778 | search | `GET /search/index/segments/winsorized-mean?pct=N` — clamp top/bottom N% antes da média |
| #779 | calendar | `GET /calendars/:cal_id/events-by-range/rrule-freq-stats` — GROUP BY FREQ= extraído do rrule |
| #780 | notifications | `GET /dlq/stats/by-user-and-kind?limit=N` — GROUP BY (user_id, kind) COUNT DESC |
| #781 | drive | `GET /drive/files/stats/mime-entropy` — Shannon H=-Σp*log2(p) sobre mime_type; null→octet-stream |
| #782 | mail | `GET /mail/messages/stats/to-count-distribution` — histograma 0/1/2/3/4/5+ destinatários |
| #783 | search | `GET /search/index/segments/normalized-entropy` — H/log2(n) ∈ [0,1] para num_docs e disk_bytes |
| #784 | calendar | `GET /calendars/:cal_id/events-by-range/transparency-stats` — OPAQUE/TRANSPARENT/unset COUNT |
| #785 | notifications | `GET /dlq/stats/by-day-and-kind-and-tenant` — 3D GROUP BY (day, kind, tenant_id) ASC |
| #786 | drive | (incorporado no #781 como mime-entropy) |
| #787 | mail | `GET /mail/messages/stats/avg-recipients-by-folder` — AVG(to+cc+bcc) jsonb_array_length |
| #788 | search | `GET /search/index/segments/relative-sizes` — pct_bytes por segmento; disk_bytes DESC |
| #789 | calendar | `GET /calendars/:cal_id/events-by-range/class-by-day` — GROUP BY (day, class) |
| #790 | notifications | `GET /dlq/stats/by-hour-and-user?since=&until=` — GROUP BY (hour, user_id) ASC |
| #791 | drive | `GET /drive/files/stats/avg-versions` — AVG/MAX versões por arquivo via drive_file_versions |
| #792 | mail | `GET /mail/messages/stats/first-message-by-folder` — MIN received_at por pasta |
| #793 | search | `GET /search/index/segments/size-ratio` — disk_bytes/num_docs (bytes per doc) DESC |
| #794 | calendar | `GET /calendars/:cal_id/events-by-range/title-word-count` — avg/max palavras em summary |

| #795 | notifications | `GET /dlq/stats/by-minute-and-tenant` — GROUP BY (minute, tenant_id) ASC |
| #796 | drive | `GET /drive/files/stats/ext-entropy` — Shannon H sobre extensões; top-20 |
| #797 | mail | `GET /mail/messages/stats/attachment-size-by-folder` — avg/max size_bytes WHERE has_attachments |
| #798 | search | `GET /search/index/segments/z-scores` — z=(x-mean)/stdev por segmento; docs_z DESC |
| #799 | calendar | `GET /calendars/:cal_id/events-by-range/location-entropy` — Shannon H sobre locations |
| #800 | notifications | `GET /dlq/stats/by-minute-and-user` — GROUP BY (minute, user_id) ASC |
| #801 | drive | `GET /drive/files/stats/checksum-coverage` — with/without sha256; coverage_pct |
| #802 | mail | `GET /mail/messages/stats/read-ratio-by-folder` — Seen flag read/unread + read_pct |
| #803 | search | `GET /search/index/segments/doc-density` — num_docs/disk_bytes (docs_per_byte) DESC |
| #804 | calendar | `GET /calendars/:cal_id/events-by-range/has-alarm-stats` — LIKE '%BEGIN:VALARM%' |
| #805 | notifications | `GET /dlq/stats/top-tenants-by-kind?limit=N` — top-N tenants por kind |
| #806 | drive | `GET /drive/files/stats/storage-key-coverage` — with/without storage_key; coverage_pct |
| #807 | mail | `GET /mail/messages/stats/subject-word-count-by-folder` — avg/max palavras via regexp_split |
| #808 | search | `GET /search/index/segments/coefficient-dispersion` — range/mean para num_docs e disk_bytes |
| #809 | calendar | `GET /calendars/:cal_id/events-by-range/dtstart-hour-distribution` — histograma 0-23h |
| #810 | notifications | `GET /dlq/stats/by-kind-and-minute` — GROUP BY (kind, minute) ASC |
| #811 | drive | `GET /drive/files/stats/locked-by-user?limit=N` — top-N users por arquivos bloqueados |
| #812 | mail | `GET /mail/messages/stats/cc-count-distribution` — histograma 0/1/2/3/4/5+ cc_addrs |
| #813 | search | `GET /search/index/segments/percentile-rank` — percentis 25/50/75/90/95 de num_docs e disk_bytes |
| #814 | calendar | `GET /calendars/:cal_id/events-by-range/weekday-distribution` — COUNT por DOW (0=Sun..6=Sat) |

| #815 | notifications | `GET /dlq/stats/by-attempts-and-kind` — histograma tentativas × kind (1/2/3/4/5+) |
| #816 | drive | `GET /drive/files/stats/mime-by-ext` — top mime_type por extensão GROUP BY (ext, mime) |
| #817 | mail | `GET /mail/messages/stats/flagged-by-folder` — with/without \Flagged por pasta |
| #818 | search | `GET /search/index/segments/outliers?threshold=N` — segmentos com |z| > threshold |
| #819 | calendar | `GET /calendars/:cal_id/events-by-range/month-distribution` — COUNT por mês (1-12) |
| #820 | notifications | `GET /dlq/stats/by-tenant-and-minute` — GROUP BY (minute, tenant_id) ASC |
| #821 | drive | `GET /drive/files/stats/size-trend-by-day?since=&until=` — SUM(size_bytes) por dia |
| #822 | mail | `GET /mail/messages/stats/bcc-count-distribution` — histograma 0/1/2/3/4/5+ bcc_addrs |
| #823 | search | `GET /search/index/segments/size-bands` — histograma tiny/small/medium/large/huge |
| #824 | calendar | `GET /calendars/:cal_id/events-by-range/calendar-coverage` — days_with_events / total_days |
| #825 | notifications | `GET /dlq/stats/by-day-and-user-and-kind` — 3D GROUP BY (day, user_id, kind) |
| #826 | drive | `GET /drive/files/stats/version-age?limit=N` — arquivos com versões mais antigas |
| #827 | mail | `GET /mail/messages/stats/priority-by-folder` — X-Priority por pasta |
| #828 | search | `GET /search/index/segments/top-docs-ratio` — pct_docs = num_docs/total DESC |
| #829 | calendar | `GET /calendars/:cal_id/events-by-range/duration-by-class` — avg/max minutos por class |
| #830 | notifications | `GET /dlq/stats/by-hour-and-kind-and-tenant` — 3D GROUP BY (hour, kind, tenant_id) |
| #831 | drive | `GET /drive/files/stats/mime-count-by-user` — top (owner, mime) por file_count |
| #832 | mail | `GET /mail/messages/stats/importance-by-folder` — Importance header por pasta |
| #833 | search | `GET /search/index/segments/decay?threshold=N` — razão below_threshold / total |
| #834 | calendar | `GET /calendars/:cal_id/events-by-range/recurrence-by-weekday` — rrule events por DOW |

| #835 | notifications | `GET /dlq/stats/by-minute-and-kind-and-tenant` — 3D GROUP BY (minute, kind, tenant_id) |
| #836 | drive | `GET /drive/files/stats/created-vs-deleted-by-day` — net criados/deletados por dia |
| #837 | mail | `GET /mail/messages/stats/sensitivity-by-folder` — Sensitivity header por pasta |
| #838 | search | `GET /search/index/segments/balance-score` — 1 − (stdev/mean); ∈ (-∞,1] |
| #839 | calendar | `GET /calendars/:cal_id/events-by-range/organizer-by-weekday` — top organizers por DOW |
| #840 | notifications | `GET /dlq/stats/by-second-and-kind` — DATE_TRUNC('second') × kind |
| #841 | drive | `GET /drive/files/stats/version-size-by-user` — total version bytes por owner |
| #842 | mail | `GET /mail/messages/stats/list-id-by-folder` — List-Id (mailing list) por pasta |
| #843 | search | `GET /search/index/segments/age-index-ratio` — avg_bytes_per_doc global |
| #844 | calendar | `GET /calendars/:cal_id/events-by-range/all-day-by-weekday` — all-day events por DOW |
| #845 | notifications | `GET /dlq/stats/by-minute-and-user-and-kind` — 3D GROUP BY (minute, user_id, kind) |
| #846 | drive | `GET /drive/files/stats/ext-size-by-folder` — SUM(size_bytes) por (ext, folder) |
| #847 | mail | `GET /mail/messages/stats/keywords-by-folder` — X-Keywords header por pasta |
| #848 | search | `GET /search/index/segments/doc-index-ratio` — docs_per_segment = total_docs/segment_count |
| #849 | calendar | `GET /calendars/:cal_id/events-by-range/location-by-weekday` — location × DOW breakdown |
| #850 | notifications | `GET /dlq/stats/by-second-and-tenant` — DATE_TRUNC('second') × tenant_id |
| #851 | drive | `GET /drive/files/stats/tag-by-user` — top tags por owner_user_id |
| #852 | mail | `GET /mail/messages/stats/inboxed-vs-sent-by-day` — COUNT msgs por (day, mailbox) ASC |
| #853 | search | `GET /search/index/segments/fragmentation` — segment_count / total_docs |
| #854 | calendar | `GET /calendars/:cal_id/events-by-range/attendee-response-by-weekday` — PARTSTAT × DOW |

| #855 | notifications | `GET /dlq/stats/by-second-and-user` — DATE_TRUNC('second') × user_id |
| #856 | drive | `GET /drive/files/stats/tag-entropy` — Shannon H sobre tags |
| #857 | mail | `GET /mail/messages/stats/auto-replied-by-folder` — Auto-Submitted header por pasta |
| #858 | search | `GET /search/index/segments/bytes-per-doc-by-segment` — bytes/doc por segmento DESC |
| #859 | calendar | `GET /calendars/:cal_id/events-by-range/organizer-domain-by-weekday` — SPLIT_PART('@',2) × DOW |
| #860 | notifications | `GET /dlq/stats/by-second-and-kind-and-tenant` — 3D second×kind×tenant |
| #861 | drive | `GET /drive/files/stats/folder-mime-entropy` — Shannon H sobre mime_type por folder |
| #862 | mail | `GET /mail/messages/stats/x-mailer-by-folder` — X-Mailer header por pasta |
| #863 | search | `GET /search/index/segments/health-score` — balance × (1−fragmentation) |
| #864 | calendar | `GET /calendars/:cal_id/events-by-range/class-by-weekday` — CLASS × DOW |
| #865 | notifications | `GET /dlq/stats/by-minute-and-kind-and-user` — 3D minute×kind×user |
| #866 | drive | `GET /drive/files/stats/size-entropy` — Shannon H sobre 8 size buckets |
| #867 | mail | `GET /mail/messages/stats/content-type-by-folder` — Content-Type header por pasta |
| #868 | search | `GET /search/index/segments/write-amplification` — total_bytes/total_docs global |
| #869 | calendar | `GET /calendars/:cal_id/events-by-range/status-by-weekday` — STATUS × DOW |
| #870 | notifications | `GET /dlq/stats/by-hour-and-kind-and-user` — 3D hour×kind×user |
| #871 | drive | `GET /drive/files/stats/version-count-by-ext` — avg/max versões por extensão |
| #872 | mail | `GET /mail/messages/stats/disposition-by-folder` — Content-Disposition por pasta |
| #873 | search | `GET /search/index/segments/utilization?max_docs=N` — num_docs/max_docs ratio |
| #874 | calendar | `GET /calendars/:cal_id/events-by-range/priority-by-weekday` — PRIORITY bucket × DOW |

| #875 | notifications | `GET /dlq/stats/by-hour-and-user-and-kind` — 3D hour×user×kind |
| #876 | drive | `GET /drive/files/stats/folder-size-entropy` — Shannon H sobre total_bytes por folder |
| #877 | mail | `GET /mail/messages/stats/organization-by-folder` — Organization header por pasta |
| #878 | search | `GET /search/index/segments/docs-percentile-band` — 4 bandas percentil de num_docs |
| #879 | calendar | `GET /calendars/:cal_id/events-by-range/transparency-by-weekday` — TRANSP × DOW |
| #880 | notifications | `GET /dlq/stats/retry-rate-by-kind` — AVG/MAX attempts por kind |
| #881 | drive | `GET /drive/files/stats/avg-file-size-by-folder` — AVG/MAX size_bytes por folder |
| #882 | mail | `GET /mail/messages/stats/from-addr-length-by-folder` — avg/max LENGTH(from_addr) por pasta |
| #883 | search | `GET /search/index/segments/docs-size-correlation` — Pearson r entre num_docs e disk_bytes |
| #884 | calendar | `GET /calendars/:cal_id/events-by-range/duration-by-weekday` — avg minutos por DOW |
| #885 | notifications | `GET /dlq/stats/failed-at-hour-distribution` — histograma hora do dia (0-23) |
| #886 | drive | `GET /drive/files/stats/storage-by-folder` — total_bytes + file_count por folder top-N |
| #887 | mail | `GET /mail/messages/stats/subject-entropy` — Shannon H sobre subjects únicos |
| #888 | search | `GET /search/index/segments/size-percentile` — p25/p50/p75/p90/p95 de disk_bytes |
| #889 | calendar | `GET /calendars/:cal_id/events-by-range/rrule-interval-stats` — INTERVAL= do rrule |
| #890 | notifications | `GET /dlq/stats/by-attempts-and-tenant` — attempts × tenant COUNT |
| #891 | drive | `GET /drive/files/stats/ext-version-age` — MIN/MAX created_at versões por ext |
| #892 | mail | `GET /mail/messages/stats/from-domain-entropy` — Shannon H sobre domínios de remetente |
| #893 | search | `GET /search/index/segments/top-by-docs` — segmento com maior num_docs |
| #894 | calendar | `GET /calendars/:cal_id/events-by-range/location-word-count` — avg/max palavras em location |

| #895 | notifications | `GET /dlq/stats/by-user-and-tenant` — GROUP BY (user_id, tenant_id) COUNT DESC |
| #896 | drive | `GET /drive/files/stats/tag-frequency-by-folder` — top (folder, tag) COUNT DESC |
| #897 | mail | `GET /mail/messages/stats/has-preview-by-folder` — with/without preview_text por pasta |
| #898 | search | `GET /search/index/segments/bottom-by-docs` — segmento com menor num_docs |
| #899 | calendar | `GET /calendars/:cal_id/events-by-range/summary-word-count-by-weekday` — avg palavras × DOW |
| #900 | notifications | `GET /dlq/stats/by-kind-and-day-and-tenant` — 3D kind×day×tenant |
| #901 | drive | `GET /drive/files/stats/size-trend-by-folder` — SUM(size_bytes) por (folder, dia) |
| #902 | mail | `GET /mail/messages/stats/attachment-count-distribution` — with/without attachments cross-folder |
| #903 | search | `GET /search/index/segments/segment-age-rank` — rank por id lexicográfico |
| #904 | calendar | `GET /calendars/:cal_id/events-by-range/created-vs-updated-by-day` — criados+atualizados por dia |
| #905 | notifications | `GET /dlq/stats/error-length-by-kind` — avg/max LENGTH(last_error) por kind |
| #906 | drive | `GET /drive/files/stats/folder-count-by-user` — COUNT pastas por owner |
| #907 | mail | `GET /mail/messages/stats/thread-age-by-folder` — avg age por thread por pasta |
| #908 | search | `GET /search/index/segments/docs-above-median` — segmentos acima da mediana |
| #909 | calendar | `GET /calendars/:cal_id/events-by-range/dtstart-month-by-year` — COUNT por (year, month) |
| #910 | notifications | `GET /dlq/stats/tenant-coverage` — DISTINCT tenant_id + user_id |
| #911 | drive | `GET /drive/files/stats/file-age-by-folder` — avg/max age dias por folder |
| #912 | mail | `GET /mail/messages/stats/size-entropy` — Shannon H 5 buckets cross-folder |
| #913 | search | `GET /search/index/segments/median-docs` — mediana de num_docs |
| #914 | calendar | `GET /calendars/:cal_id/events-by-range/has-description-by-weekday` — with/without description × DOW |

| #915 | notifications | `GET /dlq/stats/by-hour-and-day?since=&until=` — 2D (day, hour) GROUP BY ASC |
| #916 | drive | `GET /drive/files/stats/starred-by-folder?limit=N` — COUNT starred_at IS NOT NULL por pasta |
| #917 | mail | `GET /mail/messages/stats/unread-rate-by-folder` — unread/total ratio por pasta ORDER BY rate DESC |
| #918 | search | `GET /search/index/segments/variance` — variância amostral (n-1) de num_docs e disk_bytes |
| #919 | calendar | `GET /calendars/:cal_id/events-by-range/organizer-count-by-day` — COUNT DISTINCT organizer por dia |
| #920 | notifications | `GET /dlq/stats/by-day-and-hour-and-kind?since=&until=` — 3D (day, hour, kind) ASC |
| #921 | drive | `GET /drive/files/stats/last-modified-by-folder?limit=N` — MAX updated_at por pasta |
| #922 | mail | `GET /mail/messages/stats/recent-by-folder` — COUNT msgs últimas 24h/7d/30d por pasta |
| #923 | search | `GET /search/index/segments/above-p75` — segmentos com disk_bytes acima do P75 |
| #924 | calendar | `GET /calendars/:cal_id/events-by-range/attendee-response-stats` — PARTSTAT breakdown global (parse in-app) |
| #925 | notifications | `GET /dlq/stats/by-hour-and-day-and-tenant?since=&until=` — 3D (day, hour, tenant_id) ASC |
| #926 | drive | `GET /drive/files/stats/created-by-hour` — histograma hora-do-dia de created_at (0-23) |
| #927 | mail | `GET /mail/messages/stats/flagged-count-by-folder` — Flagged + total + flagged_rate por pasta |
| #928 | search | `GET /search/index/segments/compaction-ratio` — avg docs/segment (total_docs/segment_count) |
| #929 | calendar | `GET /calendars/:cal_id/events-by-range/end-hour-distribution` — histograma 0-23h de dtend |
| #930 | notifications | `GET /dlq/stats/user-coverage?limit=N` — COUNT DISTINCT user_id por tenant_id ORDER BY DESC |
| #931 | drive | `GET /drive/files/stats/large-files?limit=N` — arquivos ≥ 100MB ORDER BY size_bytes DESC |
| #932 | mail | `GET /mail/messages/stats/avg-size-by-weekday` — AVG size_bytes por DOW (0=Dom) |
| #933 | search | `GET /search/index/segments/size-spread` — max_bytes − min_bytes amplitude de disk_bytes |
| #934 | calendar | `GET /calendars/:cal_id/events-by-range/organizer-top-n` — top-20 organizers por event_count |

| #935 | notifications | `GET /dlq/stats/by-kind-and-user-and-day?since=&until=` — 3D kind×user×day ASC |
| #936 | drive | `GET /drive/files/stats/modified-by-hour` — histograma hora-do-dia de updated_at (0-23) |
| #937 | mail | `GET /mail/messages/stats/sender-domain-by-weekday` — top domínio por DOW via SPLIT_PART |
| #938 | search | `GET /search/index/segments/docs-density-rank` — rank docs/byte por segmento DESC |
| #939 | calendar | `GET /calendars/:cal_id/events-by-range/class-stats` — CLASS distribution global com pct |
| #940 | notifications | `GET /dlq/stats/by-tenant-and-day-and-kind?since=&until=` — 3D tenant×day×kind ASC |
| #941 | drive | `GET /drive/files/stats/size-by-weekday` — AVG/SUM size_bytes por DOW de created_at |
| #942 | mail | `GET /mail/messages/stats/subject-re-fwd-by-folder` — replies+forwards+total por pasta |
| #943 | search | `GET /search/index/segments/docs-sum` — soma total_docs + avg_docs_per_segment |
| #944 | calendar | `GET /calendars/:cal_id/events-by-range/summary-entropy` — Shannon H sobre summaries |
| #945 | notifications | `GET /dlq/stats/by-user-and-hour?since=&until=` — GROUP BY (user_id, hour_of_day) ASC |
| #946 | drive | `GET /drive/files/stats/ext-by-weekday` — COUNT por (DOW, extensão) de created_at |
| #947 | mail | `GET /mail/messages/stats/to-addrs-per-message` — AVG/MAX jsonb_array_length(to_addrs) |
| #948 | search | `GET /search/index/segments/id-length-stats` — avg/min/max LENGTH(segment_id) |
| #949 | calendar | `GET /calendars/:cal_id/events-by-range/duration-bucket` — histograma <30m/30-60m/1-4h/4-8h/>8h |
| #950 | notifications | `GET /dlq/stats/by-tenant-and-user-and-kind?limit=N` — 3D tenant×user×kind COUNT DESC |
| #951 | drive | `GET /drive/files/stats/zero-size` — null_size + zero_bytes + total_zero |
| #952 | mail | `GET /mail/messages/stats/received-by-weekday` — COUNT por DOW de received_at |
| #953 | search | `GET /search/index/segments/docs-floor` — segmento com menor num_docs (piso) |
| #954 | calendar | `GET /calendars/:cal_id/events-by-range/alarm-count-stats` — with/without VALARM + avg alarms |

| #955 | notifications | `GET /dlq/stats/by-kind-and-tenant-and-hour` — 3D kind×tenant×hour |
| #956 | drive | `GET /drive/files/stats/size-percentile` — p25/p50/p75/p90/p95 de size_bytes |
| #957 | mail | `GET /mail/messages/stats/from-addr-count` — COUNT DISTINCT from_addr por pasta |
| #958 | search | `GET /search/index/segments/bytes-ceiling` — segmento com maior disk_bytes |
| #959 | calendar | `GET /calendars/:cal_id/events-by-range/sequence-by-weekday` — avg sequence por DOW |
| #960 | notifications | `GET /dlq/stats/by-kind-and-hour-and-user` — 3D kind×hour×user |
| #961 | drive | `GET /drive/files/stats/owner-entropy` — Shannon H sobre owner_user_id |
| #962 | mail | `GET /mail/messages/stats/msg-id-length-by-folder` — avg/max LENGTH(message_id) por pasta |
| #963 | search | `GET /search/index/segments/docs-above-mean` — segmentos com num_docs > média |
| #964 | calendar | `GET /calendars/:cal_id/events-by-range/recurrence-by-month` — rrule events por mês |
| #965 | notifications | `GET /dlq/stats/by-tenant-and-hour-and-user` — 3D tenant×hour×user |
| #966 | drive | `GET /drive/files/stats/version-size-by-ext` — total version bytes por extensão |
| #967 | mail | `GET /mail/messages/stats/to-addrs-domain` — top domínios em to_addrs jsonb |
| #968 | search | `GET /search/index/segments/bytes-above-mean` — segmentos com disk_bytes > média |
| #969 | calendar | `GET /calendars/:cal_id/events-by-range/description-word-count` — avg/max palavras em description |
| #970 | notifications | `GET /dlq/stats/by-user-and-day-and-hour` — 3D user×day×hour ASC |
| #971 | drive | `GET /drive/files/stats/locked-age` — avg/max dias bloqueados (locked_at) |
| #972 | mail | `GET /mail/messages/stats/thread-count-by-weekday` — COUNT DISTINCT thread_id por DOW |
| #973 | search | `GET /search/index/segments/size-above-mean` — alias semântico bytes-above-mean |
| #974 | calendar | `GET /calendars/:cal_id/events-by-range/dtstart-by-month` — COUNT por mês (1-12) de dtstart |

---

## Próximos candidatos (#975+)

1. **notifications** — `GET /dlq/stats/by-kind-and-user-and-hour` — 3D kind×user×hour
2. **drive** — `GET /drive/files/stats/tag-size-by-ext` — top (tag, ext) por total_bytes
3. **mail** — `GET /mail/messages/stats/has-reply-to-by-folder` — with/without reply_to por pasta
4. **search** — `GET /search/index/segments/bytes-floor` — segmento com menor disk_bytes
5. **calendar** — `GET /calendars/:cal_id/events-by-range/organizer-by-month` — top organizers por mês

---

## Workflow de sprint

Cada sprint = 1 commit no formato:
```
feat(scope): descrição — sprint #N
```

Rotation: notifications → drive → mail → search → calendar (5 serviços, 3 sprints por "next").

Para continuar: diga **"vai"** ou **"next"**.

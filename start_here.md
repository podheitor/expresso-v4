# Expresso v4 — Ponto de Retomada

**Último sprint commitado:** #519 (2026-05-03)

```
git log --oneline | head -15
```

---

## O que foi feito nesta sessão (#336–#347)

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
| #346 | drive | `POST /api/v1/drive/files/bulk-move` — mover até 200 itens atomicamente |
| #347 | meet | `POST /api/v1/meetings/:id/restore` — reativa reunião arquivada (creator ou moderador) |
| #348 | meet | `GET /api/v1/meetings?archived=true` — lista reuniões arquivadas do usuário |
| #349 | drive | `POST /api/v1/drive/files/:id/copy` — cópia shallow: nova row, mesmo blob |
| #350 | drive | `POST /api/v1/drive/files/bulk-trash` — soft-delete até 200 itens em batch |
| #351 | meet | `DELETE /api/v1/meetings/:id/participants/:user_id` — remover participante (moderator-only) |
| #352 | drive | `GET /api/v1/drive/users/:user_id/usage` — bytes usados por usuário no tenant |
| #353 | meet | `PATCH /api/v1/meetings/:id/participants/:user_id` — promover/rebaixar role (moderator-only) |
| #354 | drive | `GET /api/v1/drive/files/search?q=` — busca por nome ILIKE no tenant |
| #355 | meet | `GET /api/v1/meetings/:id/participants/:user_id` — detalhe de participante com ETag/LM |
| #356 | drive | `GET /api/v1/drive/files?kind=file\|folder` — filtro por tipo na listagem |
| #357 | meet | `GET /api/v1/meetings/:id/participants/count` — contagem de participantes |
| #358 | drive | `GET /api/v1/drive/files?sort=name\|updated_at\|created_at\|size_bytes&order=asc\|desc` |
| #359 | meet | `PATCH /api/v1/meetings/:id` — campo `is_recurring` adicionado ao `UpdateBody` |
| #360 | drive | `GET /api/v1/drive/files?limit=N&offset=N` — paginação na listagem (limit 1–500, default 200) |
| #361 | meet | `GET /api/v1/meetings?after=&before=` — filtro por `scheduled_for` em RFC 3339 |
| #362 | drive | `GET /api/v1/drive/files/search?q=&limit=&offset=` — paginação na busca por nome |
| #363 | meet | `GET /api/v1/meetings/:id/participants?limit=N&offset=N` — paginação na lista de participantes |
| #364 | imap | NAMESPACE (RFC 2342) — anuncia namespace pessoal prefix="" delim="."; feature `ext_namespace` habilitada |
| #365 | notifications | Redis pub/sub cross-pod — `internal_notify` publica em `"expresso:notifications"`; outros pods recebem via relay subscriber |
| #366 | imap | CONDSTORE/QRESYNC (RFC 7162) — `mod_sequence` em `messages`, `HIGHESTMODSEQ` em SELECT/STATUS, `CHANGEDSINCE` em FETCH, `MODSEQ` data item, `ENABLE CondStore` |
| #367 | drive | `GET /api/v1/drive/files/:id/preview` — `Content-Disposition: inline` para image/* e application/pdf; 415 para outros tipos |
| #368 | meet | Webhook on meeting created/archived — `MEET__WEBHOOK_URL` fire-and-forget POST `{event, tenant_id, meeting}` |
| #369 | compliance | `GET /api/v1/compliance/archive/export` — ZIP em memória com `manifest.json` + `messages/*.eml`; mesmos filtros que list |
| #370 | meet | Webhook on meeting restored — `meeting.restored` event via `webhook::dispatch` no handler `restore` |
| #371 | compliance | Export ZIP signed — HMAC-SHA256 em `X-Export-Signature` via `COMPLIANCE__EXPORT_SECRET` (hmac+sha2+hex) |
| #372 | notifications | Metrics — `IntCounterVec notifications_dispatched_total{kind}` incrementado em `internal_notify` |
| #373 | drive | `DELETE /api/v1/drive/files/:id/versions/:v` — remove versão específica + blob em disco |
| #374 | drive | `POST /api/v1/drive/files/bulk-copy` — shallow-copy até 200 itens com nome "<original> (cópia)" |
| #375 | meet | Webhook retry — exponential backoff até 3 tentativas (1s/2s/4s) em `webhook::dispatch` |
| #376 | drive | `POST /api/v1/drive/files/bulk-restore` — desfaz trash de até 200 itens atomicamente |
| #377 | compliance | Export ZIP password — AES-256 via `?password=` em `GET /api/v1/compliance/archive/export` |
| #378 | notifications | Webhook externo — `NOTIFICATIONS__WEBHOOK_URL` dispara POST fire-and-forget em `internal_notify` |
| #379 | drive | File tags — `GET/POST /api/v1/drive/files/:id/tags` + `DELETE /:id/tags/:tag` — tabela `drive_file_tags` |
| #380 | meet | Recording — `POST /api/v1/meetings/:id/recording/start|stop` — campo `recording_started_at`; `MeetError::Conflict` |
| #381 | search | Facets — `GET /api/v1/search?facet=kind` retorna `facets.kind[]` via `DocSetCollector` + campo `kind` no schema |
| #382 | drive | File share link — já implementado (detectado); sprint pulado |
| #383 | mail | Scheduled send — `POST /api/v1/mail/messages/schedule` com `deliver_at TIMESTAMPTZ`; background worker 30s |
| #384 | meet | Chat integration — `GET /api/v1/meetings/:id/chat` retorna metadados do canal via `chat_channels` |
| #385 | notifications | Digest — `GET /api/v1/notifications/digest?since=` agrega não-lidas por kind; tabela `notifications` persistida |
| #386 | drive | Folder download — `GET /api/v1/drive/folders/:id/download` — ZIP recursivo; `collect_files_recursive` em `FileRepo`; crate `zip` adicionado |
| #387 | mail | Vacation toggle — `PATCH /api/v1/mail/vacation/toggle` com `is_active`; preserva outros campos; re-renderiza sieve_script |
| #388 | meet | Waiting room — `GET /api/v1/meetings/:id/lobby` + `POST /approve/:user_id` + `DELETE /:user_id`; tabela `meeting_lobby` |
| #389 | search | Delete by tenant — `DELETE /api/v1/index?tenant_id=` remove todos docs via `delete_term` no Tantivy |
| #390 | drive | File expiry — `PATCH /api/v1/drive/files/:id/expiry`; campo `expires_at`; worker GC horário purga blobs expirados |
| #391 | mail | Read receipts — `POST /api/v1/mail/messages/:id/read-receipt` envia MDN RFC 8098 multipart/report |
| #392 | meet | Poll/vote — `POST/GET /api/v1/meetings/:id/polls` + `GET/DELETE /:poll_id` + `POST /vote`; tabelas `meeting_polls` + `meeting_poll_votes` |
| #393 | notifications | Mark read — `PATCH /api/v1/notifications/:id/read` + `/read-all`; UPDATE is_read=true |
| #394 | compliance | Retention policy — `GET/PUT /api/v1/compliance/retention`; tabela `compliance_tenant_retention`; default 365 dias |
| #395 | drive | File lock — `POST/DELETE /api/v1/drive/files/:id/lock`; campos `locked_by + locked_at`; lock otimista por usuário |
| #396 | mail | Flag presets — `GET/POST /api/v1/mail/flag-presets` + `GET/PUT/DELETE /:id`; tabela `mail_flag_presets` com flags JSONB |
| #397 | meet | Breakout rooms — `POST/GET /api/v1/meetings/:id/breakouts` + `GET/DELETE /:room_id` + `POST/DELETE /:room_id/participants`; tabelas `meeting_breakout_rooms` + `meeting_breakout_participants` |
| #398 | contacts | Import CSV — `POST /api/v1/contacts/import`; multipart (book_id + file); parser CSV quoted; gera vCard 3.0 por linha |
| #399 | — | Pulado — calendar event RSVP já implementado (detectado em `events.rs`) |
| #400 | drive | Starred files — `POST/DELETE /api/v1/drive/files/:id/star` + `GET /api/v1/drive/starred`; campo `starred_at` |
| #401 | calendar | Event alarms — `GET/POST /api/v1/calendars/:cal_id/events/:event_id/alarms` + `DELETE /:alarm_uid`; tabela `calendar_event_alarms(uid, action, trigger_rel, trigger_abs, description)`; `CalendarError::AlarmNotFound` |
| #402 | — | Pulado — mail thread view já implementado (`/mail/threads/:thread_id` + `list_thread` + `thread_id` filter em `list_messages`) |
| #403 | notifications | Push subscription — `POST/DELETE /api/v1/notifications/push`; tabela `notification_push_subscriptions(endpoint, p256dh, auth)`; UPSERT por endpoint |
| #404 | meet | Transcript metadata — `GET/POST /api/v1/meetings/:id/transcript`; tabela `meeting_transcripts(url, language, starts_at, ends_at, created_by)`; GET para participantes; POST moderator-only |
| #405 | drive | Comments — `GET/POST /api/v1/drive/files/:id/comments` + `DELETE /:comment_id`; tabela `drive_file_comments(user_id, body)`; DELETE restrito ao autor |
| #406-#485 | vários | Roadmap continuado (search/calendar/mail/meet/drive/contacts/compliance/notifications) — ver `git log --oneline` ou `memory/project_status.md` para detalhes |
| #486 | compliance | Archive tag co-occurrence — `GET /api/v1/compliance/archive/tags/co-occurrence` (paralelo drive #484, USER-scoped) |
| #487 | mail | Mark-unread bulk per special-use — `POST /api/v1/mail/folders/special-use/mark-unread?slots=trash,junk` (combo #485 + #473) |
| #488 | calendar | Event instance EXDATE cancel — `POST /:cal_id/events/:id/cancel-instance {instance}` injeta EXDATE; expander filtra via `parse_exdates` |
| #489 | drive | Tag intersect-exclude — `GET /api/v1/drive/tags/intersect-exclude?tags=&exclude=` (AND-set + NOT EXISTS) |
| #490 | mail | Folder rename revert-all batch — `POST /folders/rename-history/revert-all?n=N` (DISTINCT ON + atômico) |
| #491 | calendar | EXDATE list/clear/delete — `GET /:id/exdates` + `DELETE /:id/exdates` + `DELETE /:id/exdates/:instance` (inverso completo de #488) |
| #492 | compliance | Archive tag intersect-exclude — `GET /archive/tags/intersect-exclude?tags=&exclude=` (paralelo USER-scoped de #489) |
| #493 | drive | Tag co-occurrence by user — `GET /tags/co-occurrence-by-user?user_id=&tag=&min_count=` (extensão #484 + #479) |
| #494 | mail | Folder rename revert-all dry-run — `?dry=true` em `revert-all` (preview SELECT-only) |
| #495 | calendar | RECURRENCE-ID instance override — `POST /:id/override-instance {instance, summary?,...}` (alternativa não-destrutiva ao EXDATE cancel #488) |
| #496 | calendar | RECURRENCE-ID override list — `GET /:id/overrides` lista os VEVENT overrides existentes (paralelo ao #491 EXDATE list) |
| #497 | calendar | RECURRENCE-ID override delete — `DELETE /:id/overrides/:recurrence_id` remove um override (inverso de #495) |
| #498 | calendar | RECURRENCE-ID override patch — `PATCH /:id/overrides/:recurrence_id` edita summary/description/location/dtstart/dtend in-place (complemento de #495+#496+#497) |
| #499 | calendar | EXDATE/RECURRENCE-ID conflict-aware — 409 em `override-instance` se EXDATE pra mesma instance e em `cancel-instance` se override existe; `Conflict` enum agora carrega `String` |
| #500 | calendar | RECURRENCE-ID override get-one — `GET /:id/overrides/:recurrence_id` snapshot completo (summary/description/location/dtstart/dtend/dtstamp) com ETag/LM herdados do master, 304 em INM/IMS, 404 se não existe |
| #501 | calendar | Override→cancel migration — `POST /:id/overrides/:recurrence_id/cancel` substitui workflow 2-passos (DELETE override + POST cancel-instance) por 1 chamada atômica; 1 só DB write, sequence bumpa 1 vez |
| #502 | calendar | Cancel→override migration — `POST /:id/exdates/:instance/override {summary?,description?,location?,dtstart?,dtend?}` inverso simétrico do #501; `remove_exdate_value` + `inject_before_end_vcalendar` compostos antes do único `update`; 1 DB write, sequence bumpa 1 vez; 404 se EXDATE não existe; 409 se override já existe; 400 se sem rrule ou sem campos override |
| #503 | calendar | Override list rica — `GET /:id/overrides?detail=full` adiciona description+location em cada item pra paridade com get-one (#500); default `summary` mantém shape original do #496; helper `list_recurrence_id_overrides` ganha bool `full`; refator pra `upper16` uniforme (universal prefix matcher); 400 em detail desconhecido |
| #504 | calendar | EXDATE list rica — `GET /:id/exdates?detail=full` (paralelo simétrico do #503); novo helper `parse_exdates_rich` cobre TZID/parametros/date-only/unknown que `parse_exdates` plain ignora; cada item ganha `tzid?`, `params?`, `kind`(`"utc"\|"tzid"\|"date-only"\|"unknown"`), `raw_value`; default `summary` preserva shape `{compact,rfc3339}` do #491; 400 em detail desconhecido |
| #505 | calendar | Override DTSTAMP-only touch — `POST /:id/overrides/:recurrence_id/touch` refresca SÓ o DTSTAMP do VEVENT override sem mutação de campos; reusa `patch_recurrence_id_override_block` com TODOS os campos None; sequence NÃO bumpa (DTSTAMP fora das colunas DISTINCT FROM) mas ETag/updated_at do master refrescam — bastante pra invalidar HTTP/CalDAV cache; sem body; retorna `{event_id, recurrence_id, touched:true, dtstamp, etag, sequence}`; 404 se override não existe |
| #506 | calendar | Master event DTSTAMP-only touch — `POST /:id/touch` paralelo do #505 mas no master VEVENT; novo helper `patch_master_dtstamp` matcha bloco com `UID==master AND !has_recurrence_id` (overrides com mesmo UID preservados); se DTSTAMP ausente no master, adiciona; mesma semantics do #505 (sequence NÃO bumpa, ETag/updated_at refrescam); use case: forçar re-sync em clients iCal cacheando por DTSTAMP, "ressuscitar" eventos pós-restore; sem body; retorna `{event_id, touched:true, dtstamp, etag, sequence}`; 400 se master sem UID; requer WRITE+ |
| #507 | calendar | Override DTSTAMP-only touch BULK — `POST /:id/touch-overrides {"instances":[…]}` variante batch do #505; valida cada instance via `has_recurrence_id_override` separando `touched` vs `not_found` (best-effort, não 404 individualmente); aplica `patch_recurrence_id_override_block(..., None×5)` sequencialmente in-memory; 1 único `EventRepo::update` no fim (vs N round-trips); dedup por `target_compact`; limite 1..256 instances; 404 só se NENHUMA bate; mesma semantics do #505 (sequence NÃO bumpa, ETag/updated_at refrescam); use case: ressuscitar série inteira após bug de sync sem N requests; retorna `{event_id, touched, not_found, dtstamp, etag, sequence}`; requer WRITE+ |
| #508 | calendar | Master+overrides touch-all — `POST /:id/touch-all` combina #506 + #507; descobre overrides via `list_recurrence_id_overrides(raw, uid, false)` (mesmo walker do #503), itera extraindo `compact` e aplica `patch_recurrence_id_override_block(..., None×5, &dtstamp_now)` in-memory; depois `patch_master_dtstamp(raw, uid, &dtstamp_now)` no fim; 1 único `EventRepo::update`; cache-nuke total do VCALENDAR sem cliente listar nada; mesma semantics #505/#506/#507 (sequence NÃO bumpa, ETag/updated_at refrescam); sem body; 400 se master sem UID; retorna `{event_id, master_touched:true, overrides_touched:[…compact…], dtstamp, etag, sequence}`; requer WRITE+ |
| #509 | calendar | Override touch by range — `POST /:id/touch-overrides-by-range?after=&before=` variante range do #507 sem listar instances; descobre overrides via `list_recurrence_id_overrides`, parseia cada `compact` via `parse_one_exdate`, filtra `[after, before)` (half-open, ambos opcionais — sem nenhum ≡ #508 sem master); aplica `patch_recurrence_id_override_block(..., None×5)` in-memory; 1 único `EventRepo::update`; mesma semantics #505 (sequence NÃO bumpa, ETag/updated_at refrescam); 400 se `after >= before`; 404 se nenhum override no range; retorna `{event_id, touched:[…compacts…], skipped:[…fora do range…], dtstamp, etag, sequence}`; use case: ressuscitar só overrides futuros sem afetar histórico, ou janela de migração específica; requer WRITE+ |
| #510 | calendar | Touch-all dry-run — `?dry=true` no #508 retorna o plano (lista de compacts que SERIAM tocados + master:true) sem chamar `EventRepo::update`, sem alterar ETag/updated_at/DTSTAMP, sem publicar `EventUpdated`; nova `TouchAllQuery` struct + `Query<TouchAllQuery>` extractor no `touch_all`; quando `dry`, walka `list_recurrence_id_overrides` igual ao path real mas só popula `overrides_touched` sem patches; retorna `{dry:true, event_id, master_touched:true, overrides_touched:[…]}` (sem etag/sequence/dtstamp); 400 ainda fired se master sem UID; default `dry=false` preserva semantics original; útil pra UI confirmar "vai mexer em N overrides + master, ok?" antes de cache-nuke; paralelo do #494 (mail revert-all dry-run) — sprint #510 |
| #511 | calendar | Touch-overrides bulk dry-run — `?dry=true` no #507 retorna `{dry:true, event_id, touched, not_found}` sem `EventRepo::update`, sem ETag/updated_at/DTSTAMP, sem `EventUpdated`; nova `TouchOverridesBulkQuery` struct + `Query<TouchOverridesBulkQuery>` extractor adicionado ao `touch_overrides_bulk`; ramo dry valida instances igual ao real (parse `parse_one_exdate`, dedup `target_compact`, check `has_recurrence_id_override(&ev.ical_raw, …)`) e particiona em `touched`/`not_found` sem aplicar `patch_recurrence_id_override_block`; mesma validação 400 (lista 1..256, master sem UID) e 404 (touched vazio) que path real; default `dry=false` preserva semantics #507; UI consegue prever "instances X/Y/Z viram tocadas, A/B não existem" antes de rodar; segundo dry-run da família touch (#510 fez touch-all primeiro) — sprint #511 |
| #512 | calendar | Touch-overrides-by-range dry-run — `?dry=true` no #509 retorna `{dry:true, event_id, touched, skipped}` sem `EventRepo::update`, sem ETag/updated_at/DTSTAMP, sem `EventUpdated`; campo `dry: Option<bool>` adicionado a `TouchOverridesByRangeQuery` (composto com `after`/`before`); ramo dry walka `list_recurrence_id_overrides(&ev.ical_raw, …)` igual ao real, parseia compact via `parse_one_exdate`, aplica filtros `[after, before)` half-open, particiona em `touched`/`skipped` sem patch; mesma validação 400 (`after >= before`, master sem UID) e 404 (touched vazio) que path real — UI não vê dry "ok" mas real "fail"; default `dry=false` preserva semantics #509; trio bulk dry-run da família touch fechado (#510 touch-all + #511 bulk-list + #512 by-range); resta apenas dry-run nos singles #505/#506 — sprint #512 |
| #513 | calendar | Touch single dry-run — `?dry=true` no #505 (`POST /overrides/:recurrence_id/touch`) e #506 (`POST /:id/touch`) retorna `{dry:true, event_id, [recurrence_id,] touched:true}` sem `EventRepo::update`, sem ETag/sequence/dtstamp, sem `EventUpdated`; nova struct compartilhada `TouchSingleQuery { dry: Option<bool> }` + `Query<TouchSingleQuery>` extractor adicionado a ambos handlers; ramo dry só roda DEPOIS dos checks reais (assert_can_write, EventRepo::get, parse_one_exdate, extract_uid, has_recurrence_id_override) — preserva 100% das validações 400/404 do path real; default `dry=false` preserva semantics original; **família touch dry-run 100% completa:** 5 endpoints × 5 dry-runs (#510 touch-all + #511 bulk-list + #512 by-range + #513 touch-single + override-single) — sprint #513 |
| #514 | calendar | Touch combined preview — `GET /api/v1/calendars/:cal_id/events/:id/touch-preview?after=&before=` consolida em 1 chamada o que SERIA tocado por `touch-all` (#508) + `touch-overrides-by-range` (#509) sem nenhum side effect; ortogonal aos POST `?dry=true` (#510-#513) que precisam de WRITE+ por serem POST short-circuit; este é GET puro READ-only (não exige `assert_can_write`); retorna `{event_id, master:true, total_overrides, in_range, out_of_range, unparseable}` agregando 3 dimensões num só payload — `master` sempre true (touch-all sempre tocaria), `in_range`/`out_of_range` particiona via filtros half-open `[after, before)` opcionais (mesma semantics #509), `unparseable` lista RECURRENCE-IDs que `parse_one_exdate` rejeita (compact corrompido); sem `after`/`before`, todos vão pra `in_range` (degenera ≡ touch-all sem master); novo `TouchPreviewQuery { after, before }` com `time::serde::rfc3339::option`; reusa `list_recurrence_id_overrides(false)` walker; 400 se `after >= before` ou master sem UID; útil pra UI "discovery" antes de qualquer mutação (audit/dry preview unificado) — sprint #514 |
| #515 | calendar | Touch-preview parseable filters — `?include_unparseable=false` (default `true`) esconde a lista `unparseable` do payload mas mantém no `total_overrides` (UI sabe que existem N items corrompidos sem precisar listá-los); `?only_parseable=true` (default `false`) exclui de TUDO — payload e count refletem só o universo de RECURRENCE-IDs que `touch-all` efetivamente conseguiria mexer; flags compostas: `only_parseable=true` + `include_unparseable=true` explícito → 400 (conflito); `only_parseable=true` implica `include_unparseable=false` automaticamente; campo opcional do JSON omitido em vez de `[]` quando escondido (consistência com pattern de detail-aware response); zero impacto em request sem flags (shape original do #514 100% preservado); útil pra UI "modo limpo" não mostrar lixo + UI "modo análise" agregando count completo sem detalhes — extensão direta do #514 — sprint #515 |
| #516 | calendar | EXDATE list filter por kind — `GET /api/v1/calendars/:cal_id/events/:id/exdates?detail=full&kind=utc\|tzid\|date-only\|unknown` filtra a lista pelo `kind` parseado em `parse_exdates_rich` (extensão direta do #504); novo campo `kind: Option<String>` em `ListExdatesQuery` validado pra um dos 4 valores fixos (qualquer outro → 400); só faz sentido com `detail=full` porque `summary` degenera pra UTC-only (pula TZID/date-only/unknown silenciosamente) — `kind=utc` em `summary` é no-op aceito (sem warning); `kind=tzid\|date-only\|unknown` com `summary` → 400 explícito ("requires detail=full"); `kind` ausente preserva 100% shape do #504 (filter degenera num pass-through); útil pra audit "quais EXDATEs estão em formato MVP-unsupported" (`kind=unknown`) ou "quais precisam migrar pra UTC" (`kind=tzid`); read-only, não requer WRITE; resposta mantém formato `{event_id, count, exdates:[...]}` com `count` refletindo só items que sobraram no filtro — sprint #516 |
| #517 | calendar | Overrides list range filter — `GET /api/v1/calendars/:cal_id/events/:id/overrides?after=&before=` filtra a lista de RECURRENCE-ID overrides por intervalo half-open `[after, before)` parseando o `compact` de cada item via `parse_one_exdate` (paralelo simétrico do `touch-overrides-by-range` #509 e do EXDATE list filter #516); ambos opcionais (RFC3339), ausência total preserva 100% shape do #496/#503; quando algum bound é dado, RECURRENCE-IDs não-parseáveis (TZID-based, etc.) são pulados silenciosamente — sem range, todos aparecem (mesmo formato exótico); composto com `?detail=full` do #503 (filtra primeiro pelo walker, range depois via `retain`); 400 se `after >= before`; read-only, não requer WRITE; `count` reflete só items pós-filtro; útil pra UI "esta semana"/"próximo mês" sem N+1 GETs e sem listar tudo + filtrar client-side — sprint #517 |
| #518 | calendar | Overrides list filter por presença — `GET /api/v1/calendars/:cal_id/events/:id/overrides?has_summary=&has_dtstart=&has_dtend=` filtros booleanos qualitativos em AND aplicados após o range filter do #517; cada flag opcional independente — `true` exige campo presente no override, `false` exige ausência (campo nulo no JSON); combinados segmentam categorias semânticas: "só rename de título" (`has_summary=true&has_dtstart=false&has_dtend=false`) vs. "só reschedule" (`has_summary=false&has_dtstart=true`); aplicados via segundo `Vec::retain` condicional (skip total quando todas 3 são `None`) que checa `item.get(key).map(|v| !v.is_null())` por campo; `description`/`location` ficam de fora porque só estão presentes em `?detail=full` — assimétrico, não vale flag dedicada; ausência total dos 3 flags preserva 100% shape do #517 (e por extensão #496/#503); read-only, não requer WRITE; `count` reflete intersecção range+presença pós-filtro; padrão: filter chains ortogonais (range temporal + presença qualitativa) compostos via `Vec::retain` sequenciais condicionais — variant qualitativa do #517 — sprint #518 |
| #519 | calendar | Overrides count-by-detail stats — `GET /api/v1/calendars/:cal_id/events/:id/overrides/stats` agrega counts de presença dos campos `summary`/`dtstart`/`dtend` em todos os overrides do evento (agregado do filter qualitativo do #518); retorna `{event_id, total, by_field:{summary:{present,absent}, dtstart:{...}, dtend:{...}}, by_category:{none, only_summary, only_dtstart, only_dtend, summary_dtstart, summary_dtend, dtstart_dtend, all_three}}`; `by_field` é cardinality marginal (`present + absent = total` por campo, somas independentes), `by_category` particiona em 8 buckets disjuntos por combinação de presença (soma das 8 categorias = total); útil pra dashboards exibirem distribuição "só rename" vs "só reschedule" vs "rename + reschedule" sem puxar lista inteira do #518 e contar client-side; rota literal `/overrides/stats` precede `/overrides/:recurrence_id` (axum matcha literal antes de param wildcard); description/location ficam de fora pela mesma assimetria do #518 (só existem em `?detail=full`); reusa `list_recurrence_id_overrides(raw, uid, false)` do #503/#517/#518; read-only, não requer WRITE; 404 se evento não existe; complementa o filter do #518 (sample) com agregado completo (population) — sprint #519 |

---

## Sessões anteriores (#333–#335)

| Sprint | Escopo | O que foi feito |
|--------|--------|-----------------|
| #333 | admin | Migration `updated_at` em `govbr_user_map` + trigger automático; `govbr.rs` usa `updated_at` direto para ETag/LM |
| #334 | search | `POST /api/v1/index/bulk` — indexa até 500 docs por chamada; um único commit Tantivy |
| #335 | compliance | `Last-Modified` + `IMS` em `list_policies`; ETag/LM via `updated_at` em `get_policy` |

---

## Próximos candidatos (#520-#525)

1. **search:** adicionar `received_at` ao tantivy schema + facet temporal (sprint maior, requer reindex)
2. **meet:** participant invite via mail real — chamada cross-service usando `reqwest`
3. **mail:** sieve filter test endpoint — `POST /api/v1/mail/sieve/test`
4. **drive:** trash auto-purge schedule — config tenant pra auto-rodar #453 periodicamente
5. **calendar:** events bulk-update por range — PATCH em massa (mover, mudar calendar, set RRULE)
6. **mail:** folder rename revert-by-mailbox — `POST /folders/rename-history/by-mailbox/:mailbox_id/undo` (granular variant de #490)
7. **drive:** tag intersect-exclude por user — variant user-scoped de #489 com filtro `created_by`
8. **calendar:** touch-preview com summaries — extensão do #514/#515 com `?detail=full` pra trazer SUMMARY/DTSTART/DTEND de cada in_range item (preview rico pra UI confirmar visualmente quem vai ser afetado)
9. **calendar:** EXDATE preview combo — `GET /:id/exdates-preview?after=&before=` aplicando mesmo padrão de #514/#515 mas pra EXDATE (paralelo simétrico ao preview de overrides)
10. **calendar:** EXDATE list count-by-kind — `GET /:id/exdates/stats` agrega counts `{utc, tzid, date_only, unknown, total}` num só payload (paralelo agregado ao #516 pra dashboard)
11. **calendar:** EXDATE list filter por presença — paralelo simétrico do #518 mas no EXDATE list, ex: `?with_tzid=true|false` (em vez de `kind=tzid` exclusivo do #516, permite filtro qualitativo "tem TZID ou não")
12. **calendar:** overrides stats com filtros — `?after=&before=` em `/overrides/stats` (#519) restringindo agregado a uma janela temporal (composição #517 + #519)

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

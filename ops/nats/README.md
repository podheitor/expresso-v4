# NATS ops toolbox

Ferramentas bash para operar/diagnosticar JetStream em Expresso v4.

## `smoke.sh` — presença de stream
Sprint #22. Verifica se um stream existe e imprime stats básicas.

```bash
ops/nats/smoke.sh http://localhost:8222 EXPRESSO_CALENDAR
# OK: stream 'EXPRESSO_CALENDAR' present.
```

Exit: 0 presente, 1 ausente, 2 endpoint inacessível.

## `e2e-smoke.sh` — write-path end-to-end
Sprint #26. Mede delta `state.messages` antes/depois de um trigger.

```bash
ops/nats/e2e-smoke.sh http://localhost:8222 EXPRESSO_CALENDAR \
    "docker run --rm --network host natsio/nats-box:latest \
     nats --server=nats://localhost:4222 pub expresso.calendar.test payload"
# OK: +1 messages
```

Exit: 0 count aumentou, 1 não aumentou ou stream ausente.

## `tail.sh` — live subscribe
Sprint #27. Subscreve a um subject pattern e imprime mensagens.

```bash
ops/nats/tail.sh nats://localhost:4222 'expresso.calendar.>'
# >> tailing expresso.calendar.> on nats://localhost:4222
# [#1] Received on "expresso.calendar.<tenant>.event_created"
# {"kind":"event_created","tenant_id":"...","event_id":"...","summary":"..."}
```

Usa `natsio/nats-box:latest` como CLI (pull automático). Ctrl-C para parar.

## Streams atuais

| Stream | Subject | Publisher | Retenção |
|---|---|---|---|
| EXPRESSO_CALENDAR | `expresso.calendar.<tenant>.<kind>` | calendar service (sprint #20) | 7 dias |
| EXPRESSO_CONTACTS | `expresso.contacts.<tenant>.<kind>` | contacts service (sprint #23+#24) | 7 dias |

## Próximos passos

- Consumer worker lendo `expresso.calendar.>` para iMIP dispatch.
- Grafana dashboard JetStream — ver `ops/grafana/` (sprint #21).

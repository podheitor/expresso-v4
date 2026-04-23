# Expresso v4 — Referência de APIs

Documento consolidado das APIs HTTP dos serviços Expresso. Todos os serviços usam autenticação baseada em headers `x-user-id` + `x-tenant-id` (UUIDs). Em produção, esses headers são injetados por um gateway/auth-RP após validação do JWT emitido pelo Keycloak. Para testes diretos, use os UUIDs do Postgres (coluna `user_entity.id` + atributo `tenant_id`).

**Host base (dev/lab):** `http://192.168.15.125`

---

## 📋 Sumário

| Serviço            | Porta | Base Path       | Papel                              |
|--------------------|-------|-----------------|-------------------------------------|
| expresso-web       | 8090  | `/`             | SSR UI + proxy para backends        |
| expresso-admin     | 8101  | `/`             | Admin UI (dashboard, users, realm)  |
| expresso-auth      | 8012  | `/auth`         | OIDC RP (login/callback/refresh/me) |
| expresso-mail      | 8001  | `/api/v1/mail`  | Mail (IMAP-backed, JMAP-lite)       |
| expresso-calendar  | 8002  | `/api/v1`       | Calendar + CalDAV                   |
| expresso-contacts  | 8003  | `/api/v1`       | Contacts + CardDAV                  |
| expresso-drive     | 8004  | `/api/v1/drive` | Drive (files, TUS upload, WOPI)     |
| expresso-chat      | 8010  | `/api/v1`       | Chat (Matrix/Synapse wrapper)       |
| expresso-meet      | 8011  | `/api/v1`       | Meet (Jitsi scheduler)              |
| keycloak           | 8080  | `/realms/*`     | IdP (OIDC)                          |

---

## 🔐 Autenticação

Todos os services backend aceitam dois headers:

```
x-user-id: <uuid>      # KC sub (user_entity.id)
x-tenant-id: <uuid>    # atributo user_attribute "tenant_id"
```

Em produção, estes headers são injetados pelo `expresso-auth` após validar o Bearer JWT. Para tests diretos no lab:

```bash
export UID_A="c3a1459f-3c3f-4ff5-bee4-8e7958bbb698"
export TID_A="40894092-7ec5-4693-94f0-afb1c7fb51c4"
alias alice='curl -sS -H "x-user-id: $UID_A" -H "x-tenant-id: $TID_A"'
```

---

## ✉️ expresso-mail (`:8001`)

| Método | Path                                      | Descrição                          |
|--------|-------------------------------------------|-------------------------------------|
| GET    | `/api/v1/mail/folders`                    | Lista pastas IMAP                   |
| GET    | `/api/v1/mail/messages?folder=INBOX`      | Lista mensagens da pasta            |
| GET    | `/api/v1/mail/messages/:id`               | Detalhe da mensagem (body + meta)   |
| POST   | `/api/v1/mail/messages/:id/flags`         | Marca flags (seen, flagged, etc.)   |
| POST   | `/api/v1/mail/messages/:id/move`          | Move mensagem para outra pasta      |
| GET    | `/api/v1/mail/messages/:id/attachments`   | Lista anexos                        |
| GET    | `/api/v1/mail/messages/:id/attachments/:index` | Download binário de anexo       |
| POST   | `/api/v1/mail/send`                       | Envia mensagem (SMTP relay)         |
| POST   | `/api/v1/mail/send-itip`                  | Envia iTIP (calendar invites)       |
| GET/PUT| `/api/v1/mail/vacation`                   | Auto-reply (Sieve vacation)         |

**Exemplo — listar INBOX:**
```bash
alice "http://192.168.15.125:8001/api/v1/mail/messages?folder=INBOX"
```

---

## 📅 expresso-calendar (`:8002`)

| Método | Path                                             | Descrição                    |
|--------|--------------------------------------------------|-------------------------------|
| GET/POST | `/api/v1/calendars`                            | Lista/cria calendários        |
| GET/DELETE/PATCH | `/api/v1/calendars/:id`                | Detalhe/remove/atualiza       |
| GET    | `/api/v1/calendars/:id/ctag`                     | ETag da coleção (sync)        |
| GET/POST | `/api/v1/calendars/:cal_id/events`             | Lista/cria eventos (iCal)     |
| GET/PUT/DELETE | `/api/v1/calendars/:cal_id/events/:id`   | Detalhe/atualiza/remove       |
| GET    | `/api/v1/calendars/:cal_id/export.ics`           | Export VCALENDAR              |
| POST   | `/api/v1/calendars/:cal_id/import`               | Import VCALENDAR              |
| GET    | `/api/v1/calendars/:cal_id/events/:id/itip/request.ics` | Gera convite iTIP      |
| POST   | `/api/v1/calendars/:cal_id/events/:id/rsvp`      | Registra RSVP (PARTSTAT)      |
| GET    | `/api/v1/calendars/:cal_id/events/:id/attendees` | Lista participantes           |
| POST   | `/api/v1/scheduling/freebusy`                    | Query free/busy               |
| ANY    | `/caldav/*`                                      | CalDAV (RFC 4791)             |

**POST evento** usa `Content-Type: text/calendar` (corpo = VCALENDAR com 1 VEVENT).

**Exemplo:**
```bash
alice -X POST -H 'content-type: application/json' http://192.168.15.125:8002/api/v1/calendars \
  -d '{"name":"Pessoal","color":"#2563eb"}'
```

---

## 👥 expresso-contacts (`:8003`)

| Método | Path                                           | Descrição                    |
|--------|------------------------------------------------|-------------------------------|
| GET/POST | `/api/v1/addressbooks`                       | Lista/cria agendas            |
| GET/DELETE/PATCH | `/api/v1/addressbooks/:id`             | Detalhe/remove/atualiza       |
| GET    | `/api/v1/addressbooks/:id/ctag`                | ETag (sync)                   |
| GET/POST | `/api/v1/addressbooks/:book_id/contacts`     | Lista/cria contatos (vCard)   |
| GET/PUT/DELETE | `/api/v1/addressbooks/:book_id/contacts/:id` | Detalhe/atualiza/remove |
| GET    | `/api/v1/addressbooks/:book_id/export.vcf`     | Export vCard bundle           |
| POST   | `/api/v1/addressbooks/:book_id/import`         | Import vCard bundle           |
| GET    | `/api/v1/gal/search?q=<term>`                  | Busca no GAL (global list)    |
| POST   | `/api/v1/gal/save`                             | Copia entrada do GAL p/ agenda |
| ANY    | `/carddav/*`                                   | CardDAV (RFC 6352)            |

**POST contact** usa `Content-Type: text/vcard` (corpo = VCARD).

---

## 💾 expresso-drive (`:8004`)

| Método | Path                                          | Descrição                     |
|--------|-----------------------------------------------|--------------------------------|
| GET    | `/api/v1/drive/files`                         | Lista arquivos e pastas        |
| GET/DELETE | `/api/v1/drive/files/:id`                 | Detalhe/remove                 |
| PATCH  | `/api/v1/drive/files/:id/metadata`            | Rename/move                    |
| POST   | `/api/v1/drive/files/:id/restore`             | Restaurar da lixeira           |
| GET    | `/api/v1/drive/files/:id/versions`            | Histórico de versões           |
| GET    | `/api/v1/drive/files/:id/versions/:v`         | Download versão específica     |
| GET/POST/DELETE | `/api/v1/drive/files/:id/shares`     | Compartilhamentos por URL      |
| GET    | `/api/v1/drive/shares/:id`                    | Detalhe do share (dono)        |
| GET    | `/api/v1/drive/share/:token`                  | Acesso público ao share        |
| POST   | `/api/v1/drive/files/mkdir`                   | Cria diretório                 |
| POST   | `/api/v1/drive/uploads`                       | **TUS**: criar upload          |
| HEAD/PATCH/DELETE | `/api/v1/drive/uploads/:id`         | **TUS**: status/chunk/abort    |
| GET    | `/api/v1/drive/quota`                         | Quota do usuário               |
| GET    | `/api/v1/drive/trash`                         | Lista lixeira                  |
| ANY    | `/wopi/files/:id`, `/wopi/files/:id/contents` | WOPI (Collabora Online)        |

**Upload via TUS:**
```bash
# 1. Criar upload
LOC=$(alice -si -X POST http://192.168.15.125:8004/api/v1/drive/uploads \
  -H "Tus-Resumable: 1.0.0" -H "Upload-Length: $LEN" \
  -H "Upload-Metadata: filename $(echo -n file.txt | base64 -w0)" \
  | awk -F': ' 'tolower($1)=="location"{gsub(/\r/,"",$2);print $2}')
UPID=${LOC##*/}
# 2. Enviar chunk
alice -X PATCH "http://192.168.15.125:8004$LOC" \
  -H "Tus-Resumable: 1.0.0" -H "Upload-Offset: 0" \
  -H "Content-Type: application/offset+octet-stream" \
  --data-binary @file.txt
```

---

## 🔑 expresso-auth (`:8012`)

| Método | Path              | Descrição                              |
|--------|-------------------|-----------------------------------------|
| GET    | `/auth/login`     | 302 → KC authorization_endpoint (PKCE)  |
| GET    | `/auth/callback`  | Recebe code, troca por tokens, seta cookie |
| POST   | `/auth/refresh`   | Rotaciona refresh_token                 |
| GET    | `/auth/logout`    | Desloga (end_session)                   |
| GET    | `/auth/me`        | Retorna `MeResponse` (Bearer obrigatório) |
| POST   | `/auth/step-up`   | MFA step-up (TOTP/WebAuthn)             |

Todo JWT emitido pelo KC deve conter:
- `sub` (UUID → `user_id`)
- `email`, `preferred_username`
- Custom claim `tenant_id` (UUID)

---

## 📊 expresso-admin (`:8101`)

SSR UI (askama) sem autenticação no MVP. Páginas:

- `GET /` — Dashboard (contagens + tabela de serviços)
- `GET /users` — Lista usuários do realm (via KC Admin API)
- `GET /realm` — Configurações do realm
- `GET /health` — Health check
- `GET /ready` — Readiness check

Env obrigatórios: `KC_URL`, `KC_REALM`, `KC_ADMIN_USER`, `KC_ADMIN_PASS`.

---

## 🌐 expresso-web (`:8090`)

SSR frontend (askama + reqwest proxy). Rotas principais (precisam de sessão):

- `GET /` — Home page (tiles dos módulos)
- `GET /login` — Página de login (link → `/auth/login`)
- `GET /me`, `/me/security` — Perfil do usuário
- `GET /mail`, `/mail/:id`, `/mail/compose` — Webmail SSR
- `GET /calendar` — Agenda
- `GET /contacts` — Contatos
- `GET /drive`, `/drive/:id/edit`, `/drive/:id/share`, `/drive/:id/versions`, `/drive/trash` — Drive
- `GET /static/*` — Assets estáticos

---

## 🧪 Health & Observability

Todos os serviços expõem:

- `GET /health` → `{"service":"...","status":"ok"}`
- `GET /ready`  → `{"ready":true}`
- `GET /metrics` → Prometheus (`expresso-observability`)

---

## 📚 Protocolos padrão

- **CalDAV** (RFC 4791): `/caldav/*` em `expresso-calendar`
- **CardDAV** (RFC 6352): `/carddav/*` em `expresso-contacts`
- **TUS** (tus.io v1.0.0): `/api/v1/drive/uploads/*` em `expresso-drive`
- **WOPI** (MS-WOPI): `/wopi/files/*` em `expresso-drive` (integração Collabora)
- **OIDC** (OpenID Connect 1.0): via Keycloak + `expresso-auth` RP
- **IMAP** (porta 143), **SMTP** (25/587), **Sieve** (4190) — expostos pelo stack mail

---

## 🚀 Scripts de demo/seed

- [scripts/seed-demo.sh](scripts/seed-demo.sh) — popula agenda "Pessoal" (2 eventos) + addressbook (5 contatos)
- [scripts/seed-demo-drive.sh](scripts/seed-demo-drive.sh) — upload TUS de `welcome.txt`
- [scripts/dkim-keygen.sh](scripts/dkim-keygen.sh) — gera chaves DKIM
- [deploy/nginx/gen-tls.sh](deploy/nginx/gen-tls.sh) — gera cert self-signed

---

Gerado em: 2026-04-22 · Expresso v4

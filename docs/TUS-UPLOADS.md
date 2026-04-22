# Drive — Uploads retomáveis (tus.io v1.0.0)

Endpoints `/api/v1/drive/uploads*` implementam o protocolo
[tus.io v1.0.0](https://tus.io/protocols/resumable-upload) — uploads
resumíveis para arquivos grandes (>50 MB recomendado).

## Capabilities

- **Core Protocol**: HEAD/PATCH/POST
- **Creation Extension**: POST cria sessão com `Upload-Length`
- **Termination Extension**: DELETE aborta sessão
- **Max-Size**: 50 GB (hard cap em `MAX_UPLOAD_BYTES`)
- **Expiration**: 30 dias (sessões abandonadas → GC via `expires_at`)

## Headers obrigatórios

Todos os requests devem conter `Tus-Resumable: 1.0.0`.
Também exigimos os headers de contexto Expresso: `x-tenant-id`, `x-user-id`.

## Fluxo

### 1. Criação
```
POST /api/v1/drive/uploads
Tus-Resumable:    1.0.0
Upload-Length:    1048576
Upload-Metadata:  filename aGVsbG8udHh0,filetype dGV4dC9wbGFpbg==
Upload-Parent-Id: <uuid>       # opcional — pasta destino
x-tenant-id: ...
x-user-id:   ...

→ 201 Created
  Location:      /api/v1/drive/uploads/<upload_id>
  Upload-Offset: 0
  Upload-Length: 1048576
```

### 2. Status (retomada após queda)
```
HEAD /api/v1/drive/uploads/<upload_id>
Tus-Resumable: 1.0.0

→ 200 OK
  Upload-Offset: 524288
  Upload-Length: 1048576
  Cache-Control: no-store
```

### 3. Chunk (envio de bytes)
```
PATCH /api/v1/drive/uploads/<upload_id>
Tus-Resumable: 1.0.0
Upload-Offset: 524288
Content-Type:  application/offset+octet-stream
Content-Length: 524288

<bytes do chunk>

→ 204 No Content
  Upload-Offset: 1048576
```

### 4. Abort
```
DELETE /api/v1/drive/uploads/<upload_id>
Tus-Resumable: 1.0.0

→ 204 No Content
```

### 5. Finalização

Quando `Upload-Offset == Upload-Length`, o drive promove automaticamente
o blob `.part` → arquivo final em `drive_files`, aplica quota/versão,
e remove a sessão. Nenhum request adicional necessário.

## Erros comuns

| Status | Causa |
|--------|-------|
| 400 | `Upload-Length` ausente ou > MAX_UPLOAD_BYTES; `Upload-Metadata filename` ausente; Content-Type ≠ `application/offset+octet-stream` no PATCH; chunk ultrapassa `Upload-Length` |
| 401 | `x-tenant-id`/`x-user-id` ausentes |
| 404 | `upload_id` não existe ou expirou |
| 409 | Offset mismatch ou colisão de nome com pasta |
| 413 | Quota do tenant estourada |

## Cliente recomendado

[`tus-js-client`](https://github.com/tus/tus-js-client) — browser + Node:
```js
import { Upload } from 'tus-js-client';

const upload = new Upload(file, {
  endpoint: 'https://drive.exemplo.gov.br/api/v1/drive/uploads',
  headers: {
    'x-tenant-id': meTenantId,
    'x-user-id':   meUserId,
  },
  metadata: {
    filename: file.name,
    filetype: file.type,
  },
  chunkSize: 5 * 1024 * 1024,
  onError:    err => console.error(err),
  onProgress: (u, t) => console.log(Math.round(u*100/t) + '%'),
  onSuccess:  () => console.log('done'),
});
upload.start();
```

## Integração UI (próximo passo)

Atualmente `/drive/upload` (expresso-web) usa multipart tradicional.
Para ativar resumo no browser:
1. Adicionar rota proxy em expresso-web: `POST/HEAD/PATCH/DELETE /drive/tus[/:id]` → forward para drive com context headers injetados
2. OU expor drive diretamente em subdomínio TLS + CORS adequado
3. Incluir `tus-js-client` em `static/` e substituir form tradicional por
   JS que escolhe tus p/ arquivos >50 MB

## Unit tests

- `sanitize_rejects_slash` — rejeita `../etc/passwd`, `a\b`, ""
- `parse_upload_metadata_standard` — base64 filename+filetype
- `parse_upload_metadata_missing_filetype` — filetype opcional
- `parse_upload_metadata_empty` — header ausente → (None, None)

Todos em `services/expresso-drive/src/api/uploads.rs` módulo `tests`.

# WOPI + Collabora Online — Setup

Expresso v4 integra com **Collabora Online CODE** (ou LibreOffice Online) via protocolo WOPI.
O host WOPI é implementado em `expresso-drive` (endpoints `/wopi/files/:id` e `/wopi/files/:id/contents`).
O gerador de `access_token` + iframe vive em `expresso-web` (`GET /drive/:id/edit`).

## 1. Gerar secret compartilhado

```sh
openssl rand -hex 32 > /etc/expresso/wopi.secret
```

## 2. Variáveis de ambiente

### expresso-drive
```yaml
environment:
  WOPI_SECRET: "<conteúdo do /etc/expresso/wopi.secret>"
```

### expresso-web
```yaml
environment:
  WOPI__SECRET:         "<mesmo secret>"
  WOPI__COLLABORA_URL:  "http://localhost:9980"          # URL BROWSER-VISIBLE
  WOPI__DRIVE_URL:      "http://expresso-drive:8004"     # URL que Collabora usa p/ chamar drive
  WOPI__TOKEN_TTL_SECS: "14400"                           # 4h default
```

Em produção atrás de reverse proxy TLS:
```yaml
WOPI__COLLABORA_URL: "https://office.exemplo.gov.br"
WOPI__DRIVE_URL:     "https://drive-internal.exemplo.gov.br"
```

## 3. Adicionar Collabora Online ao compose

```yaml
collabora:
  image: collabora/code:latest
  container_name: expresso-collabora
  restart: unless-stopped
  cap_add: [MKNOD]
  environment:
    domain: "localhost|expresso-web"          # origens WOPI permitidas (regex)
    server_name: "localhost:9980"
    extra_params: "--o:ssl.enable=false --o:ssl.termination=true"
    username: admin
    password: troque_aqui
  ports:
    - "9980:9980"
  networks: [expresso]
```

Ajuste `domain:` para incluir o host do `expresso-web` visto pelo browser
(ex: `intranet\\.gov\\.br|localhost`). Em produção, habilite TLS no proxy
e use `--o:ssl.termination=true`.

## 4. Validar

1. Reinicie drive + web com novos envs
2. Acesse `/drive` → faça upload de um `.odt` ou `.docx`
3. Botão ✎ aparece na linha → click → iframe carrega Collabora
4. Edite e salve (Ctrl+S) → verifique nova versão em `/drive/:id/versions`

## 5. Troubleshooting

| Sintoma | Causa provável |
|---------|----------------|
| `/drive/:id/edit` retorna 503 | `WOPI__SECRET` vazio no web |
| Iframe mostra "Access denied" | domínio do `expresso-web` não está em `domain:` do Collabora |
| WOPI retorna 401 | token expirou ou secrets divergem entre web/drive |
| WOPI retorna 400 | `WOPI_SECRET` vazio no drive |
| Save falha silencioso | Collabora não consegue atingir `WOPI__DRIVE_URL` — revise rede docker |

## 6. Formatos suportados

odt, ods, odp, docx, xlsx, pptx, doc, xls, ppt, rtf, txt, csv.
Filtro em `is_editable_mime()` — `services/expresso-web/src/wopi.rs`.

#!/usr/bin/env bash
# Gera par RSA 2048 bits p/ DKIM + imprime TXT record pronto p/ DNS.
# Uso: ./dkim-keygen.sh <selector> <domain> [out_dir]
#   ex: ./dkim-keygen.sh default expresso.gov.br ./secrets/dkim
set -euo pipefail

SELECTOR="${1:?usage: $0 <selector> <domain> [out_dir]}"
DOMAIN="${2:?usage: $0 <selector> <domain> [out_dir]}"
OUT="${3:-./dkim-${SELECTOR}}"

command -v openssl >/dev/null || { echo "openssl ausente"; exit 1; }

mkdir -p "$OUT"
umask 077

openssl genrsa -out "$OUT/${SELECTOR}.private" 2048 2>/dev/null
openssl rsa -in "$OUT/${SELECTOR}.private" -pubout -out "$OUT/${SELECTOR}.public" 2>/dev/null

PUB_B64=$(openssl rsa -in "$OUT/${SELECTOR}.private" -pubout -outform DER 2>/dev/null \
          | openssl base64 -A)

cat <<EOF
─────────────────────────────────────────────────────────────────
DKIM key gerada:
  Privada: $OUT/${SELECTOR}.private      (aponte DKIM_PRIVATE_KEY_PATH p/ este arquivo)
  Pública: $OUT/${SELECTOR}.public

Publique o TXT record abaixo no DNS do domínio:

  Nome : ${SELECTOR}._domainkey.${DOMAIN}
  Tipo : TXT
  Valor: "v=DKIM1; k=rsa; p=${PUB_B64}"

Após propagar o DNS (pode levar até 24h em alguns provedores), valide:
  dig +short TXT ${SELECTOR}._domainkey.${DOMAIN}
─────────────────────────────────────────────────────────────────
EOF

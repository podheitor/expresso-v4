#!/bin/bash
# Regenerate self-signed TLS cert for nginx with wildcard SAN *.expresso.local.
# Sprint #46 follow-up: tenant subdomains (pilot, pilot2, pilotN) need valid SAN
# to avoid "cert name mismatch" warnings on browsers / stricter TLS clients.
#
# Runs on prod host (125). Requires sudo (writes to /home/debian/expresso/nginx/tls/).
#
# Usage:
#   sudo bash ops/regen-tls-wildcard.sh [days] [key-bits]
# Defaults: days=825  key-bits=4096  (825d = Apple/CAB forum max for cert validity)
set -euo pipefail

DAYS="${1:-825}"
BITS="${2:-4096}"
TLS_DIR="${TLS_DIR:-/home/debian/expresso/nginx/tls}"
BAK_DIR="${TLS_DIR}/backups"
TS=$(date +%s)

[[ -d "$TLS_DIR" ]] || { echo "FAIL TLS_DIR=$TLS_DIR not found"; exit 1; }
mkdir -p "$BAK_DIR"

# Backup existing
cp -a "$TLS_DIR/expresso.crt" "$BAK_DIR/expresso.crt.bak-${TS}"
cp -a "$TLS_DIR/expresso.key" "$BAK_DIR/expresso.key.bak-${TS}"
echo "backup: $BAK_DIR/expresso.{crt,key}.bak-${TS}"

# OpenSSL config with wildcard SAN
CFG=$(mktemp)
cat > "$CFG" <<EOF
[req]
distinguished_name=req_dn
req_extensions=v3_req
prompt=no

[req_dn]
C=BR
ST=SP
L=Sao Paulo
O=Expresso Lab
OU=Dev
CN=expresso.local

[v3_req]
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=@alt

[alt]
DNS.1=expresso.local
DNS.2=*.expresso.local
DNS.3=www.expresso.local
DNS.4=auth.expresso.local
DNS.5=admin.expresso.local
DNS.6=mail.expresso.local
DNS.7=chat.expresso.local
DNS.8=meet.expresso.local
DNS.9=pilot.expresso.local
DNS.10=pilot2.expresso.local
EOF

openssl req -x509 -newkey "rsa:${BITS}" -nodes \
  -keyout "$TLS_DIR/expresso.key.new" \
  -out    "$TLS_DIR/expresso.crt.new" \
  -days "$DAYS" \
  -config "$CFG" -extensions v3_req

rm -f "$CFG"

# Verify SAN present
if ! openssl x509 -in "$TLS_DIR/expresso.crt.new" -noout -text | grep -q '\*.expresso.local'; then
  echo "FAIL generated cert missing wildcard SAN"; exit 2
fi

# Atomic swap
mv "$TLS_DIR/expresso.crt.new" "$TLS_DIR/expresso.crt"
mv "$TLS_DIR/expresso.key.new" "$TLS_DIR/expresso.key"
chmod 0644 "$TLS_DIR/expresso.crt"
chmod 0600 "$TLS_DIR/expresso.key"

echo "new cert SANs:"
openssl x509 -in "$TLS_DIR/expresso.crt" -noout -text | grep -A1 'Subject Alt'

# Reload nginx (container `expresso-nginx` by default)
NGX_CT="${NGX_CT:-expresso-nginx}"
if docker ps --format '{{.Names}}' | grep -qx "$NGX_CT"; then
  docker exec "$NGX_CT" nginx -t && docker exec "$NGX_CT" nginx -s reload
  echo "nginx reloaded: $NGX_CT"
else
  echo "WARN: container $NGX_CT not running — reload manually"
fi

echo "DONE: wildcard TLS active. notAfter=$(openssl x509 -in "$TLS_DIR/expresso.crt" -noout -enddate)"

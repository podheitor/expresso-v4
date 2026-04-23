#!/usr/bin/env bash
# Generate self-signed wildcard cert for *.expresso.local + expresso.local
set -euo pipefail
DIR=${1:-./tls}
mkdir -p "$DIR"
cd "$DIR"
cat > openssl.cnf << 'CNF'
[req]
default_bits = 2048
prompt = no
distinguished_name = dn
req_extensions = v3_req
[dn]
C = BR
ST = SP
L  = Sao Paulo
O  = Expresso Lab
OU = Dev
CN = expresso.local
[v3_req]
keyUsage = keyEncipherment, dataEncipherment, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = @san
[san]
DNS.1 = expresso.local
DNS.2 = www.expresso.local
DNS.3 = auth.expresso.local
DNS.4 = admin.expresso.local
DNS.5 = mail.expresso.local
DNS.6 = chat.expresso.local
DNS.7 = meet.expresso.local
CNF
openssl req -x509 -nodes -days 825 -newkey rsa:2048 \
  -keyout expresso.key -out expresso.crt \
  -config openssl.cnf -extensions v3_req
chmod 644 expresso.crt expresso.key
echo "Generated: $DIR/expresso.{crt,key}"
openssl x509 -in expresso.crt -noout -subject -dates -ext subjectAltName | head -6

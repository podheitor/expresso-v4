#!/bin/bash
# Remove hardcoded-claim mapper `tenant_id` de clients legados (pré-fase4).
# Após fase4 (commit 997c522) tenant_id vem do `iss` → mapper desnecessário.
# Uso:
#   KC_URL=http://127.0.0.1:8080 KC_ADMIN=admin KC_PASS='...' REALM=expresso \
#     bash ops/keycloak/remove-hardcoded-tenant-mapper.sh
set -e
: "${KC_URL:=http://127.0.0.1:8080}"
: "${KC_ADMIN:=admin}"
: "${REALM:=expresso}"
if [[ -z "${KC_PASS:-}" ]]; then echo "KC_PASS nao setado"; exit 1; fi

TOK=$(curl -sS --fail -X POST "$KC_URL/realms/master/protocol/openid-connect/token" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  -d "grant_type=password&client_id=admin-cli&username=$KC_ADMIN&password=$KC_PASS" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["access_token"])')

export TOK KC_URL REALM
python3 <<'PY'
import json, os, urllib.request
tok=os.environ["TOK"]; realm=os.environ["REALM"]
base=f"{os.environ['KC_URL']}/admin/realms/{realm}"
def req(method, path):
    r=urllib.request.Request(f"{base}{path}", method=method,
        headers={"Authorization": f"Bearer {tok}"})
    with urllib.request.urlopen(r) as resp:
        body=resp.read()
        return json.loads(body) if body else None
clients=req("GET","/clients")
targets=[c for c in clients if c["clientId"] in ("expresso-web","expresso-dav","expresso-admin")]
print(f"realm={realm} clients_alvo={len(targets)}")
removed=0
for c in targets:
    ms=req("GET", f"/clients/{c['id']}/protocol-mappers/models") or []
    hc=[m for m in ms if m.get("protocolMapper")=="oidc-hardcoded-claim-mapper" and m.get("name")=="tenant_id"]
    if not hc:
        print(f"  {c['clientId']}: clean")
        continue
    for m in hc:
        req("DELETE", f"/clients/{c['id']}/protocol-mappers/models/{m['id']}")
        print(f"  {c['clientId']}: removed mapper {m['id']}")
        removed+=1
print(f"total_removed={removed}")
PY

#!/usr/bin/env bash
# Seed Keycloak realm `expresso` for dev/lab.
# → creates realm, `expresso-web` public client, tenant_id + audience mappers,
#   declarative user profile (tenant_id), user `alice` with tenant_id attribute.
# Prereqs: Keycloak 25+ reachable at $KC_URL, admin creds.
set -euo pipefail

KC_URL="${KC_URL:-http://192.168.15.125:8080}"
KC_ADMIN="${KC_ADMIN:-admin}"
KC_ADMIN_PASS="${KC_ADMIN_PASS:?set KC_ADMIN_PASS}"
REALM="${REALM:-expresso}"
CLIENT_ID="${CLIENT_ID:-expresso-web}"
ALICE_TENANT="${ALICE_TENANT:-40894092-7ec5-4693-94f0-afb1c7fb51c4}"
ALICE_PASS="${ALICE_PASS:-alice2026!}"

_token() {
  curl -sf -X POST "$KC_URL/realms/master/protocol/openid-connect/token" \
    -d grant_type=password -d client_id=admin-cli \
    -d "username=$KC_ADMIN" -d "password=$KC_ADMIN_PASS" \
    | python3 -c 'import sys,json;print(json.load(sys.stdin)["access_token"])'
}

TOKEN=$(_token)
H=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json")

# 1. realm (idempotent)
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms" \
  -d "{\"realm\":\"$REALM\",\"enabled\":true}" || true

# 2. public client with direct-access
CLIENT_JSON=$(cat <<JSON
{"clientId":"$CLIENT_ID","enabled":true,"publicClient":true,
 "directAccessGrantsEnabled":true,"standardFlowEnabled":true,
 "redirectUris":["*"],"webOrigins":["*"]}
JSON
)
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/clients" -d "$CLIENT_JSON" || true
CUID=$(curl -sf "${H[@]}" "$KC_URL/admin/realms/$REALM/clients?clientId=$CLIENT_ID" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)[0]["id"])')

# 3. mappers: tenant_id (user attribute → claim) + audience
TENANT_MAPPER=$(cat <<JSON
{"name":"tenant_id","protocol":"openid-connect","protocolMapper":"oidc-usermodel-attribute-mapper",
 "config":{"user.attribute":"tenant_id","claim.name":"tenant_id","jsonType.label":"String",
           "id.token.claim":"true","access.token.claim":"true","userinfo.token.claim":"true"}}
JSON
)
AUD_MAPPER=$(cat <<JSON
{"name":"aud-$CLIENT_ID","protocol":"openid-connect","protocolMapper":"oidc-audience-mapper",
 "config":{"included.client.audience":"$CLIENT_ID","access.token.claim":"true","id.token.claim":"false"}}
JSON
)
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/clients/$CUID/protocol-mappers/models" -d "$TENANT_MAPPER" || true
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/clients/$CUID/protocol-mappers/models" -d "$AUD_MAPPER" || true

# 4. user profile: register tenant_id attribute + allow unmanaged attributes
# Keycloak 25 Declarative User Profile drops undeclared attrs by default.
PROFILE=$(cat <<JSON
{"unmanagedAttributePolicy":"ENABLED",
 "attributes":[
  {"name":"username","displayName":"\${username}","permissions":{"view":["admin","user"],"edit":["admin","user"]},"validations":{"length":{"min":3,"max":255}}},
  {"name":"email","displayName":"\${email}","permissions":{"view":["admin","user"],"edit":["admin","user"]},"validations":{"email":{}}},
  {"name":"firstName","displayName":"\${firstName}","permissions":{"view":["admin","user"],"edit":["admin","user"]}},
  {"name":"lastName","displayName":"\${lastName}","permissions":{"view":["admin","user"],"edit":["admin","user"]}},
  {"name":"tenant_id","displayName":"Tenant ID","permissions":{"view":["admin","user"],"edit":["admin"]}}
 ]}
JSON
)
curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/users/profile" -d "$PROFILE"

# 5. user alice
ALICE_JSON=$(cat <<JSON
{"username":"alice","enabled":true,"email":"alice@expresso.local","emailVerified":true,
 "firstName":"Alice","lastName":"Test",
 "attributes":{"tenant_id":["$ALICE_TENANT"]},
 "credentials":[{"type":"password","value":"$ALICE_PASS","temporary":false}]}
JSON
)
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/users" -d "$ALICE_JSON" || true


# 6. realm roles for RBAC
for role in SuperAdmin TenantAdmin User Readonly; do
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/roles" \
    -d "{\"name\":\"$role\",\"description\":\"Expresso $role role\"}" || true
done

# 7. assign User role to alice
ALICE_ID=$(curl -sf "${H[@]}" "$KC_URL/admin/realms/$REALM/users?username=alice" | python3 -c 'import sys,json;print(json.load(sys.stdin)[0]["id"])')
USER_ROLE=$(curl -sf "${H[@]}" "$KC_URL/admin/realms/$REALM/roles/User")
curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/users/$ALICE_ID/role-mappings/realm" \
  -d "[$USER_ROLE]" || true

# 8. enable MFA required actions (operator-driven, ≠ default) — TOTP + WebAuthn
curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/authentication/required-actions/CONFIGURE_TOTP" \
  -d '{"alias":"CONFIGURE_TOTP","name":"Configure OTP","providerId":"CONFIGURE_TOTP","enabled":true,"defaultAction":false,"priority":10,"config":{}}'
curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/authentication/required-actions/webauthn-register" \
  -d '{"alias":"webauthn-register","name":"Webauthn Register","providerId":"webauthn-register","enabled":true,"defaultAction":false,"priority":20,"config":{}}'
curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/authentication/required-actions/webauthn-register-passwordless" \
  -d '{"alias":"webauthn-register-passwordless","name":"Webauthn Register Passwordless","providerId":"webauthn-register-passwordless","enabled":true,"defaultAction":false,"priority":30,"config":{}}'

# 9. realm-level WebAuthn policy (rpName, algorithms)
curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM" \
  -d '{"webAuthnPolicyRpEntityName":"Expresso","webAuthnPolicySignatureAlgorithms":["ES256","RS256"],"webAuthnPolicyUserVerificationRequirement":"preferred","webAuthnPolicyAttestationConveyancePreference":"not specified"}'

# 10. gov.br external IdP — only when GOVBR_CLIENT_ID+GOVBR_CLIENT_SECRET provided.
# Configures OIDC broker pointing to staging or prod (GOVBR_ISSUER env).
if [[ -n "${GOVBR_CLIENT_ID:-}" && -n "${GOVBR_CLIENT_SECRET:-}" ]]; then
  GB_ISSUER="${GOVBR_ISSUER:-https://sso.staging.acesso.gov.br}"
  GB_IDP=$(cat <<JSON
{"alias":"govbr","displayName":"gov.br","providerId":"oidc","enabled":true,
 "trustEmail":true,"storeToken":false,"addReadTokenRoleOnCreate":false,"firstBrokerLoginFlowAlias":"first broker login",
 "config":{
   "clientId":"$GOVBR_CLIENT_ID","clientSecret":"$GOVBR_CLIENT_SECRET",
   "authorizationUrl":"$GB_ISSUER/authorize","tokenUrl":"$GB_ISSUER/token",
   "userInfoUrl":"$GB_ISSUER/userinfo","jwksUrl":"$GB_ISSUER/jwk",
   "issuer":"$GB_ISSUER","defaultScope":"openid email profile govbr_confiabilidades",
   "validateSignature":"true","useJwksUrl":"true","pkceEnabled":"true","pkceMethod":"S256",
   "syncMode":"FORCE","clientAuthMethod":"client_secret_post"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/identity-provider/instances" -d "$GB_IDP" ||     curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/identity-provider/instances/govbr" -d "$GB_IDP" || true

  # Mapper: gov.br sub (hashed CPF) → user attribute + access_token claim.
  CPF_MAPPER=$(cat <<JSON
{"name":"govbr-cpf-hash","identityProviderAlias":"govbr","identityProviderMapper":"oidc-user-attribute-idp-mapper",
 "config":{"claim":"sub","user.attribute":"govbr_cpf_hash","syncMode":"FORCE"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/identity-provider/instances/govbr/mappers" -d "$CPF_MAPPER" || true

  # Mapper: gov.br confiabilidades claim → attribute (JSON array preserved).
  CONF_MAPPER=$(cat <<JSON
{"name":"govbr-confiabilidades","identityProviderAlias":"govbr","identityProviderMapper":"oidc-user-attribute-idp-mapper",
 "config":{"claim":"amr","user.attribute":"govbr_confiabilidades","syncMode":"FORCE"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/identity-provider/instances/govbr/mappers" -d "$CONF_MAPPER" || true

  # Client protocol mappers: copy user attributes into access_token.
  GB_CPF_CLAIM=$(cat <<JSON
{"name":"claim-govbr-cpf-hash","protocol":"openid-connect","protocolMapper":"oidc-usermodel-attribute-mapper",
 "config":{"user.attribute":"govbr_cpf_hash","claim.name":"govbr_cpf_hash","jsonType.label":"String",
           "access.token.claim":"true","id.token.claim":"false","userinfo.token.claim":"true"}}
JSON
  )
  GB_CONF_CLAIM=$(cat <<JSON
{"name":"claim-govbr-confiabilidades","protocol":"openid-connect","protocolMapper":"oidc-usermodel-attribute-mapper",
 "config":{"user.attribute":"govbr_confiabilidades","claim.name":"govbr_confiabilidades","jsonType.label":"String",
           "multivalued":"true","access.token.claim":"true","id.token.claim":"false","userinfo.token.claim":"true"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/clients/$CUID/protocol-mappers/models" -d "$GB_CPF_CLAIM"  || true
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/clients/$CUID/protocol-mappers/models" -d "$GB_CONF_CLAIM" || true

  echo "OK: gov.br IdP seeded (issuer=$GB_ISSUER)"
else
  echo "SKIP: gov.br IdP (set GOVBR_CLIENT_ID/GOVBR_CLIENT_SECRET to enable)"
fi

# 12. Generic SAML 2.0 external IdP — only when SAML_IDP_SSO_URL + SAML_IDP_CERT
# provided. Registers a per-realm SAML broker (Okta/Azure AD/ADFS/etc). Keycloak
# parses + signature-verifies the assertion; the Rust side never touches XML.
# SAML_IDP_ALIAS lets one realm host several IdPs; it is the kc_idp_hint value
# the login flow forwards (sprint C). Attribute mappers copy the IdP's email +
# display name into KC user attributes, and a session-note mapper stamps the
# broker alias as the `identity_provider` access-token claim so JIT provisioning
# (sprint C) can tell a SAML login from a local one.
if [[ -n "${SAML_IDP_SSO_URL:-}" && -n "${SAML_IDP_CERT:-}" ]]; then
  SAML_ALIAS="${SAML_IDP_ALIAS:-saml}"
  SAML_ENTITY="${SAML_IDP_ENTITY_ID:-$SAML_IDP_SSO_URL}"
  SAML_NAMEID="${SAML_IDP_NAMEID_FORMAT:-urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress}"
  SAML_ATTR_EMAIL="${SAML_IDP_ATTR_EMAIL:-email}"
  SAML_ATTR_NAME="${SAML_IDP_ATTR_NAME:-displayName}"
  SAML_IDP=$(cat <<JSON
{"alias":"$SAML_ALIAS","displayName":"${SAML_IDP_DISPLAY:-SAML SSO}","providerId":"saml","enabled":true,
 "trustEmail":true,"storeToken":false,"addReadTokenRoleOnCreate":false,"firstBrokerLoginFlowAlias":"first broker login",
 "config":{
   "singleSignOnServiceUrl":"$SAML_IDP_SSO_URL",
   "singleLogoutServiceUrl":"${SAML_IDP_SLO_URL:-}",
   "idpEntityId":"$SAML_ENTITY",
   "signingCertificate":"$SAML_IDP_CERT",
   "nameIDPolicyFormat":"$SAML_NAMEID",
   "validateSignature":"true","wantAssertionsSigned":"true","wantAuthnRequestsSigned":"false",
   "postBindingResponse":"true","postBindingAuthnRequest":"true","syncMode":"FORCE"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/identity-provider/instances" -d "$SAML_IDP" || \
    curl -sf "${H[@]}" -X PUT "$KC_URL/admin/realms/$REALM/identity-provider/instances/$SAML_ALIAS" -d "$SAML_IDP" || true

  # Mapper: SAML email attribute → KC email attribute.
  SAML_EMAIL_MAPPER=$(cat <<JSON
{"name":"saml-email","identityProviderAlias":"$SAML_ALIAS","identityProviderMapper":"saml-user-attribute-idp-mapper",
 "config":{"attribute.name":"$SAML_ATTR_EMAIL","user.attribute":"email","syncMode":"FORCE"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/identity-provider/instances/$SAML_ALIAS/mappers" -d "$SAML_EMAIL_MAPPER" || true

  # Mapper: SAML display-name attribute → KC firstName (best-effort full name).
  SAML_NAME_MAPPER=$(cat <<JSON
{"name":"saml-display-name","identityProviderAlias":"$SAML_ALIAS","identityProviderMapper":"saml-user-attribute-idp-mapper",
 "config":{"attribute.name":"$SAML_ATTR_NAME","user.attribute":"firstName","syncMode":"FORCE"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/identity-provider/instances/$SAML_ALIAS/mappers" -d "$SAML_NAME_MAPPER" || true

  # Client protocol mapper: expose the broker alias as `identity_provider` claim
  # (session note set by KC on every brokered login) so sprint C's JIT flow can
  # detect a SAML login and pick the right saml_idp_config row.
  IDP_CLAIM=$(cat <<JSON
{"name":"claim-identity-provider","protocol":"openid-connect","protocolMapper":"oidc-usersessionmodel-note-mapper",
 "config":{"user.session.note":"identity_provider","claim.name":"identity_provider","jsonType.label":"String",
           "access.token.claim":"true","id.token.claim":"false","userinfo.token.claim":"true"}}
JSON
  )
  curl -sf "${H[@]}" -X POST "$KC_URL/admin/realms/$REALM/clients/$CUID/protocol-mappers/models" -d "$IDP_CLAIM" || true

  echo "OK: SAML IdP seeded (alias=$SAML_ALIAS sso=$SAML_IDP_SSO_URL)"
else
  echo "SKIP: SAML IdP (set SAML_IDP_SSO_URL/SAML_IDP_CERT to enable)"
fi

echo "OK: realm=$REALM client=$CLIENT_ID alice/alice2026! tenant=$ALICE_TENANT"

# 11. SuperAdmin bootstrap (opt-in via SA_PASS). Idempotent; syncs KC + DB when
# DB_HOST is set. See deploy/keycloak/seed-super-admin.sh.
if [[ -n "${SA_PASS:-}" ]]; then
  SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  "$SCRIPT_DIR/seed-super-admin.sh"
fi

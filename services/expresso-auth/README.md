# expresso-auth

OIDC Relying Party — bridge between SPA frontends and the Keycloak IdP.

## Endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET  | `/auth/login` | 303 → IdP `authorization_endpoint`, PKCE S256 + state + nonce |
| GET  | `/auth/callback` | Exchange `code` + verifier at `token_endpoint`; validate access_token via JWKS; return `TokenResponse` JSON |
| POST | `/auth/refresh` | `{ refresh_token }` → new `TokenResponse` |
| GET  | `/auth/logout` | 303 → IdP `end_session_endpoint` |
| GET  | `/auth/me` | `Authorization: Bearer …` → validated `AuthContext` JSON |
| GET  | `/health`, `/ready` | liveness/readiness |

## Configuration (env)

| Var | Required | Description |
|-----|----------|-------------|
| `AUTH_RP__ISSUER` | yes | Realm URL, e.g. `http://kc:8080/realms/expresso` |
| `AUTH_RP__CLIENT_ID` | yes | Public client w/ direct-access + redirect_uri registered |
| `AUTH_RP__REDIRECT_URI` | yes | Must match Keycloak client config exactly |
| `AUTH_RP__POST_LOGOUT_REDIRECT_URI` | no | SPA landing after logout |
| `HOST`, `PORT` | no | bind address (default `0.0.0.0:8100`) |
| `RUST_LOG` | no | tracing filter |

Keycloak realm seed: `deploy/keycloak/seed-realm.sh` — creates realm, public
client, `tenant_id` mapper, declarative user profile w/ `unmanagedAttributePolicy=ENABLED`,
seed user `alice/alice2026!`.

## Architecture

- Public client (PKCE) — no client_secret; works for SPAs + mobile.
- Pending logins kept in-memory (`HashMap<state, {verifier, redirect, expires_at}>`)
  with TTL eviction on access. **Production: replace with Redis or signed cookie**
  for multi-instance deployments.
- Discovery (`.well-known/openid-configuration`) fetched at boot; URIs cached.
- `OidcValidator` (libs/expresso-auth-client) handles JWKS rotation + RS256/ES256.
- `RpError` → stable `{error, message}` JSON contract.

## Flow (SPA integration)

1. SPA → `GET /auth/login?redirect_uri=/inbox` → 303 to Keycloak.
2. User authenticates at Keycloak → IdP 302s back to `/auth/callback?code=…&state=…`.
3. RP validates state, exchanges code, validates the returned JWT, returns
   `{ access_token, refresh_token, id_token, post_login_redirect, … }`.
4. SPA stores tokens (httpOnly cookie recommended), navigates to `post_login_redirect`.
5. Subsequent calls to `expresso-chat`/`expresso-meet`/etc. use
   `Authorization: Bearer <access_token>`.
6. Before expiry, SPA POSTs `/auth/refresh` with `refresh_token`.

## Tests

```
cargo test -p expresso-auth          # 3 PKCE unit tests (RFC 7636 §B vector)
cargo test -p expresso-auth-client   # 6 claims + validator tests
```

E2E recipe (requires running Keycloak): `deploy/test-rp.sh` (lab only).

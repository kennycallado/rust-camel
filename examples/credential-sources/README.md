# Credential Sources Example

One `http://` route whose `security_policy` extracts the credential from
four transports, tried in declared order (ADR-0059):

1. `cookie: { name: session }` — the browser tile-service transport
2. `query_param: { param: token }`
3. `header: { name: X-Api-Key }`
4. `authorization_header` — the Bearer default (ADR-0033)

The first source that supplies a credential wins; later entries are
fallbacks. No credential anywhere fails with `401` before policy
evaluation.

## Why cookies

Browser map tiles (`<img src>` from Leaflet, MapLibre, OpenLayers) cannot
set the `Authorization` header. A session cookie is the only viable
transport for that shape.

## Running

```bash
cargo run -p credential-sources
```

The demo token `demo-tile-token-0f1e2d3c` maps to a principal holding the
required `tile-user` role via `StaticTokenAuthenticator` (constant-time
store lookup). Production replaces it with an introspection endpoint or a
JWT validator — see `docs/src/services/auth.md`.

```bash
# cookie
curl -H 'Cookie: session=demo-tile-token-0f1e2d3c' http://127.0.0.1:8090/tiles
# query parameter
curl 'http://127.0.0.1:8090/tiles?token=demo-tile-token-0f1e2d3c'
# custom API-key header
curl -H 'X-Api-Key: demo-tile-token-0f1e2d3c' http://127.0.0.1:8090/tiles
# default Bearer header
curl -H 'Authorization: Bearer demo-tile-token-0f1e2d3c' http://127.0.0.1:8090/tiles
# no credential -> 401 with a generic reason
curl -i http://127.0.0.1:8090/tiles
```

## Cookie hardening (required in production)

- Issue the cookie with `HttpOnly` and `SameSite=Lax` (or stricter) where
  the session is issued. `SameSite=None` needs `Secure` and still opens
  cross-site sending.
- Cookie auth on state-changing verbs still requires a CSRF defense.
- Tokens in query parameters can leak into access logs and referrers;
  prefer them for short-lived, low-scope credentials only.

Diagnostics never render a declared credential value (ADR-0051); the `401`
body carries a generic reason only.

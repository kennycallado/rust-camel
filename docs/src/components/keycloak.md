# Keycloak

The Keycloak component connects routes to a Keycloak realm. One crate covers admin API writes, event polling, token introspection, JWKS lookup, and UMA permission evaluation. The component is a security adapter, not a messaging adapter. It does not move messages between queues.

## Consumer (Events)

`keycloak:events?realm=...&eventType=events|admin-events` polls the Keycloak events or admin-events endpoint. The consumer delivers one exchange per event. The body is the event JSON. The `CamelKeycloak*` headers carry indexed fields.

```yaml
routes:
  - id: user-events
    from: "keycloak:events?realm=myrealm&eventType=events&pollDelay=10000"
    steps:
      - log: "user=${header.CamelKeycloakUserId} type=${header.CamelKeycloakEventType}"
```

```yaml
routes:
  - id: admin-events
    from: "keycloak:admin-events?realm=myrealm&eventType=admin-events&operationTypes=CREATE,DELETE"
    steps:
      - to: "log:info"
```

## Producer (Admin API)

`keycloak:admin?operation=...&realm=...&userId=...` sends a request to the Keycloak Admin REST API. The request body is the exchange input. The response body replaces the exchange input. A bearer token is fetched from the configured client credentials before each request.

```yaml
routes:
  - id: create-user
    from: "timer:tick?period=60000"
    steps:
      - set-body: '{"username": "alice", "email": "alice@example.com", "enabled": true}'
      - to: "keycloak:admin?operation=createUser&realm=myrealm"
```

```yaml
routes:
  - id: get-user
    from: "timer:tick?period=60000"
    steps:
      - to: "keycloak:admin?operation=getUser&realm=myrealm&userId=${header.userId}"
```

The exchange property `camel.keycloak.userId` overrides the URI parameter when set. The component obtains a fresh bearer token from the configured client credentials before each request.

## URI

```text
keycloak:<kind>?<params>
```

| `kind` | Description | Reference |
| --- | --- | --- |
| `admin` | Admin REST API Producer | [operations table](#producer-admin-api) |
| `events` | Events Consumer (user and admin events) | [events table](#consumer-events) |
| `admin-events` | Alias of `events` with `eventType=admin-events` preset | [events table](#consumer-events) |

The component rejects any other path with `unknown keycloak endpoint kind`. The admin producer does not support consumers. The events consumer does not support producers.

## Camel.toml configuration

`Camel.toml` configures the realm under `[security.keycloak]`. The component reads `server_url`, `realm`, `client_id`, and `client_secret` from this section. Sub-sections tune validation, JWKS caching, and introspection caching.

```toml
[security.keycloak]
server_url = "https://kc.example.com"
realm = "myrealm"
client_id = "my-service"
client_secret = "${KEYCLOAK_CLIENT_SECRET}"

[security.keycloak.validation]
method = "local"
audience = ["camel-api"]
clock_skew_secs = 30

[security.keycloak.jwks]
cache_ttl_secs = 3600
refresh_skew_secs = 60

[security.keycloak.introspection]
max_entries = 10000
default_ttl_secs = 60
negative_ttl_secs = 5
```

`allow_internal = true` opts into HTTP and loopback addresses for local development against a Keycloak instance bound to `127.0.0.1`. Production must keep it `false`. The default blocks private IP ranges to prevent SSRF.

## Producer (Admin API)

| Operation | HTTP | Requires `userId` | Path |
| --- | --- | --- | --- |
| `createUser` | POST | no | `/admin/realms/{realm}/users` |
| `deleteUser` | DELETE | yes | `/admin/realms/{realm}/users/{userId}` |
| `getUser` | GET | yes | `/admin/realms/{realm}/users/{userId}` |
| `createRole` | POST | no | `/admin/realms/{realm}/roles` |
| `assignRole` | POST | yes | `/admin/realms/{realm}/users/{userId}/role-mappings/realm` |
| `createClient` | POST | no | `/admin/realms/{realm}/clients` |
| `createRealm` | POST | no | `/admin/realms` |

A non-2xx response returns `Err`. The pipeline catches it and the route `ErrorHandler` owns the operational signal. The Admin Producer reads the request body from `Body::Text` or `Body::Json`. The component serializes JSON bodies to the wire format. The component parses JSON response bodies into `Body::Json`. The component leaves non-JSON response bodies as `Body::Text`.

## Consumer (Events)

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `realm` | yes | — | Realm to poll |
| `eventType` | yes | — | `events` or `admin-events` |
| `pollDelay` | no | `5000` | Milliseconds between polls |
| `maxResults` | no | `100` | Max events per poll |
| `lookbackWindow` | no | `300000` | Initial lookback in ms (5 min) |
| `dedupCapacity` | no | `10000` | Max tracked event IDs |
| `maxAuthErrors` | no | `3` | Consecutive auth errors before stop |
| `type` | no | — | Filter by event type string |
| `client` | no | — | Filter by client ID |
| `operationTypes` | no | — | Filter admin events by operation (comma-separated) |
| `resourcePath` | no | — | Filter by resource path |

The consumer deduplicates events using a bounded `IndexSet` keyed by event ID. The set evicts the oldest ID when the count exceeds `dedupCapacity`. The consumer tracks the highest event timestamp and resumes polling from `last_event_time + 1` on the next cycle. Three consecutive 401 or 403 responses stop the consumer with a `system-broken` log (ADR-0012).

| Header | Type |
| --- | --- |
| `CamelKeycloakEventId` | both |
| `CamelKeycloakEventTime` | both |
| `CamelKeycloakRealmId` | both |
| `CamelKeycloakEventType` | both |
| `CamelKeycloakClientId` | user |
| `CamelKeycloakUserId` | user |
| `CamelKeycloakSessionId` | user |
| `CamelKeycloakIpAddress` | user |
| `CamelKeycloakResourceType` | admin |
| `CamelKeycloakResourcePath` | admin |
| `CamelKeycloakAuthUserId` | admin |
| `CamelKeycloakAuthClientId` | admin |

`both` means both `events` and `admin-events`. `user` is set on `events`. `admin` is set on `admin-events`.

## Claim mapping

`keycloak_claim_paths(client_id)` returns the `ClaimPaths` struct for the realm. The subject is `/sub`. The role locations are `/realm_access/roles` and `/resource_access/{client}/roles`. The scope is `/scope`. The component RFC 6901 escapes the `client_id` segment (`/` becomes `~1`, `~` becomes `~0`) before it is substituted into the path. An empty `client_id` produces `/resource_access//roles`. The caller must validate the client ID.

`KeycloakRealmConfig::introspection_authenticator()` builds an `IntrospectionAuthenticator` that wraps a `CachingTokenIntrospector` against the realm's `/protocol/openid-connect/token/introspect` endpoint. The introspector caches positive responses for `default_ttl_secs` and negative responses for `negative_ttl_secs`. The HTTP client is DNS-pinned to the introspection host and uses the SSRF policy from `[security.keycloak]`.

## JWKS and validation

The realm's `/protocol/openid-connect/certs` endpoint is the JWKS source. The JWKS cache holds keys for `cache_ttl_secs` and refreshes `refresh_skew_secs` before expiry. The component DNS-pins the HTTP client to the JWKS host to close the TOCTOU window between SSRF validation and the first request.

`[security.keycloak.validation]` controls local JWT validation. `method = "local"` performs signature and claim checks against the cached JWKS. `audience` lists the accepted `aud` claims. `clock_skew_secs` tolerates the difference between the local clock and the issuer clock.

## UMA permission evaluation

`KeycloakRealmConfig::uma_evaluator()` returns a `PermissionEvaluator` that uses Keycloak's UMA ticket flow. The evaluator obtains a service-account token, then POSTs `grant_type=urn:ietf:params:oauth:grant-type:uma-ticket` to the realm's token endpoint. The `claim_token` form field carries the requesting principal's claims as a base64-encoded JSON string. A 200 response grants permission. A 403 response returns `Denied` with the Keycloak `error_description`. A 401 response signals rejected client credentials and surfaces as `ProviderUnavailable`.

```toml
[security.keycloak.uma]
provider = "keycloak"

[security.keycloak.uma.cache]
positive_ttl_secs = 30
negative_ttl_secs = 5
max_entries = 10000
```

The UMA evaluator pins its DNS to the realm's token endpoint. The connect timeout is 5 seconds. The request timeout is 30 seconds. The evaluator fails closed on transport errors and non-200, non-403, non-401 responses.

## Transport hardening

The Keycloak HTTP client is hardened. It follows no redirects. A 302 or 303 response is treated as a misconfiguration or attack signal. Connect timeout is 10 seconds. Request timeout is 30 seconds. `validate_server_url` rejects non-HTTP schemes and rejects hosts that resolve to blocked IP ranges. The component redacts `client_secret` to `REDACTED` in `Debug` output and skips the secret in Serde serialization.

## Error handling

The Admin Producer returns `Err` on HTTP failure. The route's `ErrorHandler` owns the operational signal. ADR-0012 classifies these as category a. The Event Consumer logs transient HTTP errors at `warn!` and increments no metric. Auth retries log at `warn!` and increment the `e:keycloak:auth-material` metric. Channel-closed send failures inside the consumer increment the `b-prime:keycloak:response-body` metric and log at `error!` as `outside-contract` (ADR-0012 category b'). The max-auth-errors arm stops the consumer with `error!` as `system-broken` (ADR-0012 category c). The component redacts request and response bodies that contain secrets.

**Reference**: [Keycloak component CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-keycloak/CONTEXT.md). Example: [`examples/security-keycloak`](https://github.com/kennycallado/rust-camel/tree/main/examples/security-keycloak) shows the native auth pipeline that consumes Keycloak-issued JWTs. ADRs: [0010 Security policy pre-pipeline authz](../adr/0010-security-policy-pre-pipeline-authorization.md), [0012 Log-level convention](../adr/0012-log-level-convention-handler-contract-boundaries.md), [0033 Security defaults fail-closed startup validation](../adr/0033-security-defaults-fail-closed-startup-validation.md).

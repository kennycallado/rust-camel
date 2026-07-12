# camel-component-keycloak

Keycloak integration component for rust-camel. Provides admin API operations, event consumption, UMA permission evaluation, and token introspection via Keycloak.

## Features

### KeycloakRealmConfig

Central configuration struct holding Keycloak connection details. Derives endpoint URLs from the server URL and realm name.

```rust
let config = KeycloakRealmConfig::new(
    "https://kc.example.com".into(),
    "myrealm".into(),
    "my-client".into(),
).with_client_secret("secret".into());
```

Derives the following endpoints automatically:
- `realm_url()` -- `{server_url}/realms/{realm}`
- `token_endpoint()` -- `{realm_url}/protocol/openid-connect/token`
- `introspection_endpoint()` -- `{realm_url}/protocol/openid-connect/token/introspect`
- `jwks_uri()` -- `{realm_url}/protocol/openid-connect/certs`
- `admin_url()` -- `{server_url}/admin/realms/{realm}`

`client_secret` is serialized as `REDACTED` in Debug output and excluded from Serde serialization.

For local Keycloak (HTTP + loopback), add `.with_allow_internal(true)`:

```rust
let config = KeycloakRealmConfig::new(
    "http://localhost:8080".into(),
    "myrealm".into(),
    "my-client".into(),
)
.with_client_secret("secret".into())
.with_allow_internal(true);
```

This propagates `SsrfPolicy::AllowInternal` to all derived HTTP clients (token, introspection, JWKS, UMA). DNS pinning stays active; only private/loopback IPs and HTTP scheme are permitted.

### Admin API Producer

Sends CRUD requests to the Keycloak Admin REST API. Created as a producer (outbound) from the `keycloak:admin` endpoint. Supports the following operations:

| Operation | HTTP Method | Description |
|-----------|-------------|-------------|
| `createUser` | POST | Create a user in a realm |
| `deleteUser` | DELETE | Delete a user (requires `userId`) |
| `getUser` | GET | Get a user by ID (requires `userId`) |
| `createRole` | POST | Create a realm role |
| `assignRole` | POST | Assign a realm role to a user (requires `userId`) |
| `createClient` | POST | Create a client in a realm |
| `createRealm` | POST | Create a new realm |

The request body is taken from the exchange input (JSON or text). The response body is set on the exchange output. The `userId` can be provided via URI parameter or the `camel.keycloak.userId` exchange property.

### Event Consumer

Polls the Keycloak Admin Events API on a configurable interval. Supports two event types:

- `events` -- user login/logout/token events
- `admin-events` -- admin operations (CREATE, DELETE, etc.)

Each event is delivered as an exchange with the following headers:

| Header | Description |
|--------|-------------|
| `CamelKeycloakEventId` | Unique event ID |
| `CamelKeycloakEventTime` | Event timestamp (epoch millis) |
| `CamelKeycloakRealmId` | Realm ID |
| `CamelKeycloakEventType` | Event type string |
| `CamelKeycloakClientId` | Client ID |
| `CamelKeycloakUserId` | User ID |
| `CamelKeycloakIpAddress` | Source IP address |

The consumer deduplicates events using a bounded `IndexSet` and stops after a configurable number of consecutive auth errors.

### UMA Permission Evaluator

`KeycloakUmaEvaluator` implements `PermissionEvaluator` using Keycloak's UMA (User-Managed Access) ticket flow. Obtains a client-credentials token, then POSTs a permission request to the token endpoint with `grant_type=urn:ietf:params:oauth:grant-type:uma-ticket`.

Built from `KeycloakRealmConfig::uma_evaluator()` (requires `client_secret`).

### Introspection Authenticator Builder

`KeycloakRealmConfig::introspection_authenticator()` builds a `CachingTokenIntrospector` configured for Keycloak's token introspection endpoint. Maps Keycloak-specific claim paths (`realm_access.roles`, `resource_access.{client}.roles`, `scope`) into a standardized `Principal`.

Requires `client_secret`.

## URI Format

### Admin Endpoint

```
keycloak:admin?operation=<operation>&realm=<target-realm>[&userId=<user-id>]
```

| Parameter | Required | Description |
|-----------|----------|-------------|
| `operation` | yes | One of: `createUser`, `deleteUser`, `getUser`, `createRole`, `assignRole`, `createClient`, `createRealm` |
| `realm` | no | Target realm (overrides config realm) |
| `userId` | depends | Required for `deleteUser`, `getUser`, `assignRole` |

### Events Endpoint

```
keycloak:events?realm=<realm>&eventType=<type>[&pollDelay=<ms>][&maxResults=<n>][&lookbackWindow=<ms>][&dedupCapacity=<n>][&maxAuthErrors=<n>][&type=<filter>][&client=<filter>][&operationTypes=<filter>][&resourcePath=<filter>]
```

| Parameter | Required | Default | Description |
|-----------|----------|---------|-------------|
| `realm` | yes | -- | Realm to poll events from |
| `eventType` | yes | -- | `events` or `admin-events` |
| `pollDelay` | no | `5000` | Milliseconds between polls |
| `maxResults` | no | `100` | Max events per poll |
| `lookbackWindow` | no | `300000` | Initial lookback in ms (5 min) |
| `dedupCapacity` | no | `10000` | Max tracked event IDs for dedup |
| `maxAuthErrors` | no | `3` | Consecutive auth errors before stopping |
| `type` | no | -- | Filter by event type string |
| `client` | no | -- | Filter by client ID |
| `operationTypes` | no | -- | Filter by operation types (comma-separated) |
| `resourcePath` | no | -- | Filter by resource path |

## Quick Example

```yaml
# Poll Keycloak login events and log them
routes:
  - from: "keycloak:events?realm=myrealm&eventType=events&pollDelay=10000"
    pipeline:
      - log: "Keycloak event: ${header.CamelKeycloakEventType} user=${header.CamelKeycloakUserId}"
```

```yaml
# Create a user via the admin API
routes:
  - from: "timer:create-user?period=60000"
    pipeline:
      - set-body: '{"username": "alice", "email": "alice@example.com", "enabled": true}'
      - to: "keycloak:admin?operation=createUser&realm=myrealm"
```

## License

Apache-2.0

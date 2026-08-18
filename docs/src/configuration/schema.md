# Camel.toml schema

`Camel.toml` is the operator surface for rust-camel. The file is TOML, parsed by `CamelConfig::from_file`, and deserialized into the [`CamelConfig`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md) struct. Most fields live under `[default]` and merge with named profiles (`[production]`, `[staging]`, ...) per the profile rules described in [Configuration](index.md).

A minimal file sets the route discovery glob and the log level. Everything else has a runtime default.

```toml
{{#include ../../../examples/config-basic/Camel.toml:minimal-config}}
```

A fuller file adds supervision, tracing, and shared component defaults. Profiles override per environment.

```toml
{{#include ../../../examples/config-basic/Camel.toml:full-config}}
```

## Top-level fields

The fields below live directly under `[default]`. They are the spine of the file. Every other section in this page is optional.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `routes` | array of strings | `[]` | Glob patterns for route files (YAML or JSON). Discovery runs at startup and on every file change when watch is enabled. |
| `watch` | bool | `false` | Enable the file watcher. When `true`, route file changes trigger a hot reload. See [Hot reload](hot-reload.md). |
| `watch_debounce_ms` | integer (ms) | `300` | Delay after the last file event before a reload. Increase if one save triggers several reloads. |
| `log_level` | string | `"INFO"` | One of `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR`. |
| `timeout_ms` | integer (ms) | `5000` | Per-exchange timeout enforced by the runtime. Must be `> 0`. |
| `drain_timeout_ms` | integer (ms) | `10000` | Maximum time the runtime waits for in-flight exchanges to finish on shutdown. Must be `> 0`. |
| `include` | array of strings | — | Paths to other TOML files merged before the profile pass. See [Configuration](index.md#profiles-and-includes). |

The watcher and the file are wired through `camel-core::reload_watcher::watch_and_reload` (see ADR-0004). The `--watch` and `--no-watch` CLI flags override this field at startup.

## [components]

Component configuration is untyped TOML keyed by component name. Each component bundle parses its own block. The schema below is the union of fields the runtime and core components recognize.

```toml
[components.http]
connect_timeout_ms = 5000
response_timeout_ms = 30000
allow_internal = false

[components.kafka]
brokers = "localhost:9092"
group_id = "camel"
```

The full per-component schema lives in each component's documentation. The Kafka block is the most complex and supports a `[components.kafka.brokers_named.<name>]` sub-table for pre-configured clusters referenced from URIs with `?brokerName=<name>`. See [Kafka](../components/kafka.md) for the named-broker fields.

| Common field | Type | Default | Description |
|--------------|------|---------|-------------|
| `brokers` | string or list of strings | component-specific | Connection string for the underlying client. Kafka accepts a comma-separated string. |
| `host`, `port` | string, integer | component-specific | Listen address for components that open servers. |
| `connect_timeout_ms` | integer | component-specific | Timeout for establishing a connection. |
| `allow_internal` | bool | `false` | Allow private/loopback endpoints. Set `true` only for local development. |

Custom component bundles follow the same convention. A bundle named `echo` reads its config from `[components.echo]`. See [Custom component](../extending/custom-component.md) for the bundle contract.

## [supervision]

The runtime uses these knobs to restart a failed Consumer with capped exponential backoff. Defaults match a one-second start, doubling each attempt, capped at one minute, with five attempts.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_attempts` | integer or `null` | `5` | Maximum restart attempts. `null` retries forever. |
| `initial_delay_ms` | integer (ms) | `1000` | Delay before the first attempt. Must be `> 0`. |
| `backoff_multiplier` | float | `2.0` | Multiplier applied to the delay after each failure. Must be `>= 1.0`. |
| `max_delay_ms` | integer (ms) | `60000` | Cap on the per-attempt delay. Must be `> 0`. |

## [observability]

The observability block holds four optional sub-tables. The `[observability.tracer]` block configures the built-in tracing layer. The other three activate optional exporters.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tracer` | table | (built-in defaults) | Built-in tracing layer config. |
| `otel` | table | absent | OpenTelemetry exporter. Absent disables OTLP. |
| `prometheus` | table | absent | Prometheus scrape endpoint. Absent disables the endpoint. |
| `health` | table | absent | HTTP health/readiness endpoint. Absent disables the endpoint. |

### [observability.otel]

OTLP export configuration. The protocol and sampler accept the values listed below; values not in the enum fail at load.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Master switch for OTLP export. |
| `endpoint` | string | `"http://localhost:4317"` | OTLP collector endpoint. |
| `service_name` | string | `"rust-camel"` | Resource attribute identifying the service. |
| `protocol` | string | `"grpc"` | OTLP transport: `grpc` or `http`. |
| `sampler` | string | `"always_on"` | Sampling strategy: `always_on`, `always_off`, `ratio`. |
| `sampler_ratio` | float | `null` | Sampling probability for the `ratio` strategy. Range 0.0-1.0. |
| `metrics_interval_ms` | integer (ms) | `60000` | Period for the metrics export loop. Must be `> 0`. |
| `logs_enabled` | bool | `true` | Include log records in the OTLP stream. |
| `resource_attrs` | table | `{}` | Extra resource attributes attached to every export. |

### [observability.prometheus]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Start the scrape endpoint. |
| `host` | string | `"0.0.0.0"` | Bind address. |
| `port` | integer | `9090` | Bind port. |

### [observability.health]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Start the HTTP health endpoint. |
| `host` | string | `"0.0.0.0"` | Bind address. |
| `port` | integer | `8081` | Bind port. |
| `handler_timeout_ms` | integer (ms) | `6000` | Per-probe timeout. Must exceed the internal 5s registry tick. |
| `forced_ttl_ms` | integer (ms) or `null` | `null` | Optional TTL for forced-unhealthy entries. Disabled by default. |

## [security]

The security block holds five optional sub-tables. Pick one. Most deployments choose exactly one of `oidc`, `native`, or `keycloak`. The `permissions` and `policies` sub-tables extend the chosen identity layer with authorization.

Placeholders in the `[security]` block use `{{env:VAR}}` and `{{env:VAR:default}}` (single colon). The `{{env:VAR:-default}}` form is rejected at startup. Route files use a different `${env:...}` system. See [Environment variable interpolation](env-interpolation.md).

```toml
[security.keycloak]
server_url = "https://kc.example.com"
realm = "my-realm"
client_id = "camel"
client_secret = "{{env:KC_SECRET}}"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `oidc` | table | absent | Generic OIDC token validation. |
| `native` | table | absent | Built-in static credential store. |
| `keycloak` | table | absent | Keycloak realm with validation, JWKS, introspection, and UMA. |
| `permissions` | table of tables | absent | Named permission evaluators keyed by policy name. |
| `policies` | table | absent | Registry of WASM security policies referenced by route configuration. |

### [security.oidc]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `issuer` | string | (required) | OIDC issuer URL used to discover endpoints. |
| `jwks_uri` | string | (required) | JWKS endpoint. |
| `audience` | array of strings | `[]` | Required `aud` claim values. |
| `client_id` | string | `null` | OAuth2 client ID. |
| `client_secret` | string | `null` | OAuth2 client secret. Resolves `{{env:VAR}}`; unset variable without default fails startup. |
| `token_endpoint` | string | `null` | Token endpoint for client credentials flows. |
| `introspection_endpoint` | string | `null` | Token introspection endpoint. |

### [security.native]

The native block is a static credential store with no external identity provider. Set at least one credential: a scalar `bearer_token` or `api_key`, or one or more `[[security.native.credentials]]` entries. A config with no credential fails startup.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `subject` | string | (required) | Principal name for the scalar `bearer_token` and `api_key` identities. |
| `issuer` | string | `"native"` | Issuer recorded on synthesized principals. `null` falls back to `"native"`. |
| `bearer_token` | string | `null` | Pre-issued bearer token. Resolves `{{env:VAR}}`; unset variable without default fails startup. |
| `api_key` | string | `null` | Pre-shared API key. Resolves `{{env:VAR}}`; unset variable without default fails startup. |
| `roles` | array of strings | `[]` | Roles granted to the scalar identities. |
| `scopes` | array of strings | `[]` | Scopes granted to the scalar identities. |
| `credentials` | array of tables | `[]` | Static credentials, each with its own subject, roles, and scopes. |

`[[security.native.credentials]]` array elements:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `subject` | string | (required) | Principal name for this credential. Must not be empty. |
| `secret_env` | string | absent | Environment variable holding the secret. Read at startup; fails closed if unset or empty. |
| `secret` | string | absent | Plaintext secret. Logged with a warning at startup; use `secret_env` in production. |
| `roles` | array of strings | `[]` | Roles granted to this credential's principal. |
| `scopes` | array of strings | `[]` | Scopes granted to this credential's principal. |

Each credentials entry must set exactly one of `secret_env` or `secret`; setting both or neither fails at load. The block uses `deny_unknown_fields`, so unknown keys are rejected at load.

### [security.keycloak]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server_url` | string | (required) | Keycloak base URL. |
| `realm` | string | (required) | Realm name. |
| `client_id` | string | (required) | Client ID. |
| `client_secret` | string | (required) | Client secret. Resolves `{{env:VAR}}`; unset variable without default fails startup. |
| `validation` | table | (defaults below) | Token validation options. |
| `jwks` | table | (defaults below) | JWKS cache tuning. |
| `introspection` | table | (defaults below) | Introspection cache tuning. |
| `uma` | table | absent | UMA authorization provider. |
| `allow_internal` | bool | `false` | Allow HTTP and private addresses. Set `true` only for local Keycloak. |

`[security.keycloak.validation]` fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `method` | string | `"local"` | Validation method. `"local"` validates signature and claims locally. |
| `audience` | array of strings | `[]` | Required `aud` claim values. |
| `clock_skew_secs` | integer (s) | `30` | Tolerance for `exp` and `nbf` claims. |

`[security.keycloak.jwks]` fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cache_ttl_secs` | integer (s) | `3600` | How long the JWKS cache holds keys. |
| `refresh_skew_secs` | integer (s) | `60` | Refresh the cache this many seconds before expiry. |

`[security.keycloak.introspection]` fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_entries` | integer | `10000` | Maximum cached introspection results. |
| `default_ttl_secs` | integer (s) | `60` | TTL for positive results. |
| `negative_ttl_secs` | integer (s) | `5` | TTL for negative results. |

### `[security.permissions.<name>]`

Named permission evaluators. A route references the name through route configuration.

```toml
[security.permissions.invoice-policy]
provider = "wasm"
path = "./policies/invoice-policy.wasm"

[security.permissions.invoice-policy.config]
mode = "enforce"

[security.permissions.invoice-policy.cache]
positive_ttl_secs = 60
negative_ttl_secs = 10
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | string | (required) | Provider key. Currently `wasm`. |
| `path` | string | `null` | Provider-specific path. WASM providers need a `.wasm` file. |
| `config` | table | absent | Key-value pairs passed to the provider. |
| `cache` | table | (defaults below) | Result cache tuning. |
| `limits` | table | absent | WASM limits. See [WASM limits](#wasm-limits). |

`[security.permissions.<name>]` cache fields:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `positive_ttl_secs` | integer (s) | `30` | TTL for allow decisions. |
| `negative_ttl_secs` | integer (s) | `5` | TTL for deny decisions. |
| `max_entries` | integer | `10000` | Cache size. |

### `[security.policies.wasm.<name>]`

Registry of named WASM security policies. Each entry is a path plus limits plus a config map.

```toml
[security.policies.wasm.corp-auth]
path = "plugins/authz.wasm"

[security.policies.wasm.corp-auth.limits]
timeout-secs = 30

[security.policies.wasm.corp-auth.config]
ldap_url = "ldap://corp"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | (required) | Path to the `.wasm` file. Relative to the project root or absolute. |
| `limits` | table | absent | WASM limits. See [WASM limits](#wasm-limits). |
| `config` | table | `{}` | Key-value pairs passed to the guest `init()`. |

## [runtime_journal]

The runtime event journal. When unset, runtime state is ephemeral and lost on restart. Set a path to enable a redb-backed journal.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | (required) | Path to the `.db` file. Created if it does not exist. Must not be empty. |
| `durability` | string | `"immediate"` | `immediate` fsyncs on every commit. `eventual` skips fsync for throughput. |
| `compaction_threshold_events` | integer | `10000` | Trigger compaction after this many events. Must be `> 0`. |

## [idempotent_repo]

Persistent idempotent repository for the [Idempotent Consumer](../eip/idempotent-consumer.md) EIP. When unset, the runtime uses the in-memory `MemoryIdempotentRepository`, which is bounded and ephemeral.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `path` | string | (required) | Path to the `.redb` file. Must not be empty. |
| `durability` | string | `"immediate"` | `immediate` fsyncs on every key. `eventual` skips fsync. |

## [stream_caching]

The [Stream Cache](../steps/stream-cache.md) step buffers stream bodies past a threshold so the body can be read more than once. The cache applies to the whole runtime, not per route.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `threshold` | integer (bytes) | `camel_api::stream_cache::DEFAULT_STREAM_CACHE_THRESHOLD` | Bodies below this size pass through unread. Bodies above this size are buffered. |

## [platform]

Platform selection for leader election, readiness, and identity. Defaults to `noop`, which always reports leader and ready. Set `type = "kubernetes"` to enable leader election through a Kubernetes lease.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | string | `"noop"` | `noop` or `kubernetes`. |

`[platform]` with `type = "kubernetes"` accepts:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `namespace` | string | `null` | Namespace for the lease object. Defaults to the pod's namespace. |
| `lease_name_prefix` | string | `"camel-"` | Prefix on the lease object name. |
| `lease_duration_secs` | integer (s) | `15` | Lease lifetime. Must be `> 0`. |
| `renew_deadline_secs` | integer (s) | `10` | Maximum time the leader can hold the lease between renewals. Must be `> 0`. |
| `retry_period_secs` | integer (s) | `2` | How often a non-leader retries acquisition. Must be `> 0`. |
| `jitter_factor` | float | `0.2` | Randomisation applied to the retry period. Range 0.0-1.0. |

## [languages]

The languages block tunes the resource limits for the in-process scripting engines. Each sub-block is optional. Unset fields fall back to the rust-camel runtime default, never to the upstream engine's unlimited default.

### [languages.rhai.limits]

Rhai sandbox limits.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max-operations` | integer | runtime default | Maximum operations per script. Counter resets each call. |
| `max-string-size` | integer (bytes) | runtime default | Maximum string size in bytes. |
| `max-array-size` | integer | runtime default | Maximum array size in elements. |
| `max-map-size` | integer | runtime default | Maximum map size in key-value pairs. |
| `max-expression-depth` | integer | runtime default | Maximum expression nesting depth. |
| `max-function-expression-depth` | integer | runtime default | Maximum nesting depth for function call expressions. |
| `execution-timeout-ms` | integer (ms) | runtime default | Wall-clock timeout enforced by the consuming code. |

### [languages.js.limits]

Boa JavaScript engine limits.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `execution-timeout-ms` | integer (ms) | runtime default | Wall-clock timeout enforced by the consuming code. |
| `max-loop-iterations` | integer | runtime default | Maximum loop iterations before Boa terminates. |
| `max-recursion-depth` | integer | runtime default | Maximum recursion depth for function calls. |
| `max-stack-size` | integer (slots) | runtime default | Maximum VM stack size in slots, not bytes. |

### [languages.minijinja.limits]

MiniJinja template engine limits.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max-template-source-size` | integer (bytes) | runtime default | Maximum compiled template source size. |
| `max-context-size` | integer (bytes) | runtime default | Maximum serialised context size. |
| `max-output-size` | integer (bytes) | runtime default | Maximum rendered output size. |
| `fuel` | integer | runtime default | MiniJinja VM instruction budget. |
| `max-recursion-depth` | integer | runtime default | Maximum recursion depth for includes and blocks. |
| `execution-timeout-ms` | integer (ms) | runtime default | Wall-clock timeout enforced by the consuming code. |

## [components.template.limits]

The external [Template](../components/template.md) component's resource limits. Set any subset of fields. Unset fields fall back to the resolved defaults listed below.

```toml
[components.template.limits]
max-total-source-bytes = 16777216
max-include-count = 64
max-include-depth = 16
max-template-size = 1048576
reload-timeout-ms = 5000
```

| Field | Type | Resolved default | Description |
|-------|------|------------------|-------------|
| `max-total-source-bytes` | integer (bytes) | `16777216` (16 MiB) | Maximum total source bytes across a template dependency closure. |
| `max-include-count` | integer | `64` | Maximum number of included or imported templates per closure. |
| `max-include-depth` | integer | `16` | Maximum include and extends nesting depth. |
| `max-template-size` | integer (bytes) | `1048576` (1 MiB) | Maximum size of a single template file. |
| `reload-timeout-ms` | integer (ms) | `5000` | Wall-clock budget for a full reload build. |

Zero values are rejected. The block uses `deny_unknown_fields`, so a typo is caught at load.

## `[beans]`

Named beans are WASM plugins exposed to routes as lookup targets. Each bean is keyed by name and carries a plugin path plus an optional config map and WASM limits.

```toml
[beans.auth]
plugin = "my-auth"

[beans.auth.config]
api_key = "{{env:API_KEY}}"

[beans.auth.limits]
timeout-secs = 600
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `plugin` | string | (required) | Plugin identifier or `.wasm` path. Must be non-empty. |
| `config` | table | `{}` | Key-value pairs passed to the plugin. |
| `limits` | table | absent | WASM limits. See [WASM limits](#wasm-limits). |

### WASM limits

WASM limits appear in three places: `[beans.<name>.limits]`, `[security.permissions.<name>.limits]`, and `[security.policies.wasm.<name>.limits]`. The struct is the same in all three. The fields use `kebab-case`. Unset fields fall back to the rust-camel runtime default. The defaults are finite: 50 MiB max memory, 10 MB max wasm size, 10 000 max instances, 10 000 max tables.

| Field | Type | Runtime default | Description |
|-------|------|-----------------|-------------|
| `timeout-secs` | integer (s) | runtime default | Maximum execution time per guest call. |
| `max-memory` | integer (bytes) | `52428800` (50 MiB) | Maximum linear memory the guest can allocate. Enforced by `wasmtime`. |
| `max-concurrent-calls` | integer | runtime default | Maximum concurrent invocations against this plugin. |
| `max-wasm-size` | integer (bytes) | `10485760` (10 MB) | Maximum `.wasm` file size. |
| `allow-call-schemes` | string | `null` (deny all) | Comma-separated URI schemes the guest may call. Empty or null fails closed. |
| `max-stream-bytes` | integer (bytes) | runtime default | Maximum body bytes streamed between host and guest. |
| `max-instances` | integer | `10000` | Maximum core instances per store. |
| `max-tables` | integer | `10000` | Maximum tables per store. |
| `max-table-elements` | integer | unlimited | Maximum elements per table. |

## [datasources]

Named datasource pools. Each entry is keyed by a name that routes reference through the `datasource` URI parameter (for example `sql:SELECT * FROM users?datasource=my-db`). The block is a map of [`DatasourceConfig`](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-api/CONTEXT.md) values.

```toml
{{#include ../../../examples/sql-datasource-example/Camel.toml}}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `db_url` | string | (required) | Connection URL. `postgresql://`, `postgres://`, and `ws://` (SurrealDB) are recognised. Must not be empty. |
| `provider` | string | `null` | Provider override. Defaults from the URL scheme. |
| `max_connections` | integer | `null` | Maximum pool size. |
| `min_connections` | integer | `null` | Minimum pool size. |
| `idle_timeout_secs` | integer (s) | `null` | Idle connection timeout. |
| `max_lifetime_secs` | integer (s) | `null` | Maximum connection lifetime. |
| `ssl_mode` | string | `null` | TLS mode. Provider-specific values. |
| `ssl_root_cert` | string | `null` | Path to the root CA. |
| `ssl_cert` | string | `null` | Path to the client certificate. |
| `ssl_key` | string | `null` | Path to the client key. |
| `extra` | table | `{}` | Provider-specific key-value pairs. SurrealDB reads `namespace` and `database` from here. |

## Profile overrides

`[default]` always applies. A named profile like `[production]` is selected through `CAMEL_PROFILE=production` or the loader API. Profile blocks deep-merge on top of `[default]`. The example below shows a `[production]` block tightening timeouts and pointing Kafka at the production cluster.

```toml
{{#include ../../../examples/config-profiles/Camel.toml:profile-config}}
```

The `[development]` and `[production]` blocks set a `log_level` and override shared component defaults. Shared component defaults can also live in a separate file pulled through `include`. See [Configuration](index.md) for the merge rules and the `include` order.

## Environment variable overrides

A small allowlist of `CAMEL_*` environment variables overrides specific fields when the runtime calls `CamelConfig::from_file_with_env`. The allowlist is the security boundary: any `CAMEL_*` variable not in the list is ignored at load and logged as a warning.

| Variable | Field |
|----------|-------|
| `CAMEL_TIMEOUT_MS` | `timeout_ms` |
| `CAMEL_DRAIN_TIMEOUT_MS` | `drain_timeout_ms` |
| `CAMEL_WATCH` | `watch` |
| `CAMEL_WATCH_DEBOUNCE_MS` | `watch_debounce_ms` |
| `CAMEL_LOG_LEVEL` | `log_level` |
| `CAMEL_RUNTIME_JOURNAL_PATH` | `runtime_journal.path` |
| `CAMEL_RUNTIME_JOURNAL_DURABILITY` | `runtime_journal.durability` |
| `CAMEL_RUNTIME_JOURNAL_COMPACTION_THRESHOLD_EVENTS` | `runtime_journal.compaction_threshold_events` |
| `CAMEL_IDEMPOTENT_REPO_PATH` | `idempotent_repo.path` |
| `CAMEL_IDEMPOTENT_REPO_DURABILITY` | `idempotent_repo.durability` |
| `CAMEL_CACHE_REPO_BACKEND` | `cache_repo.backend` |
| `CAMEL_CACHE_REPO_PATH` | `cache_repo.path` |
| `CAMEL_CACHE_REPO_MAX_CAPACITY` | `cache_repo.max_capacity` |
| `CAMEL_CACHE_REPO_STALE_RETENTION` | `cache_repo.stale_retention` |
| `CAMEL_CACHE_REPO_MAX_ENTRIES` | `cache_repo.max_entries` |
| `CAMEL_SUPERVISION_INITIAL_DELAY_MS` | `supervision.initial_delay_ms` |
| `CAMEL_SUPERVISION_MAX_ATTEMPTS` | `supervision.max_attempts` |

`CAMEL_CONFIG_FILE` and `CAMEL_PROFILE` are also read, but outside the allowlist. `CAMEL_CONFIG_FILE` selects the file path before load. `CAMEL_PROFILE` selects the profile section.

**Reference**: [Config crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)

# Implementation Tasks — component-metadata-coverage

## Universal pattern (every Phase-1 + P2-1 task)

Each Component gains a **private metadata-descriptor struct** in a new `src/metadata.rs`
file. The descriptor holds ONLY `#[uri_param]` fields (no "path" field — the macro
allows zero path fields). Concrete example (kafka's descriptor, P1-1):

```rust
use camel_component_api::UriConfig;

#[allow(dead_code)] // descriptor is metadata-only; parse_uri_components is generated but never called
#[derive(UriConfig)]
#[uri_scheme = "kafka"]
#[uri_config(
    skip_impl,
    metadata(scheme = "kafka", description = "Apache Kafka consumer/producer", producer, consumer),
    crate = "camel_component_api"
)]
pub(super) struct KafkaMetadataDescriptor {
    #[uri_param(name = "brokers", required)]
    pub _brokers: String,
    #[uri_param(name = "saslPassword", secret)]
    pub _sasl_password: String,
    // ... one field per accepted URI query key
}
```

Notes:
- `#[allow(dead_code)]` on the descriptor struct is REQUIRED: `skip_impl` generates a
  `pub fn parse_uri_components` that is never called on a metadata-only descriptor, and
  without the allow, `cargo clippy -- -D warnings` fails on the dead associated function.
  (Verified: the derive-generated inherent methods inherit the struct's lint context, so
  struct-level `#[allow(dead_code)]` suppresses the warning. Confirm with
  `cargo clippy -p the-affected-crate -- -D warnings`.)
- Fields use **accurate scalar types** matching the param's semantics: `bool` for flags,
  `u32`/`u64` for counts/timeouts, `f64` for floats, `String` for free-form text,
  `Option<T>` where the parser accepts absence. The macro's `infer_option_kind`
  (`uri_config.rs`) maps the field type to each `UriOption`'s advertised `kind`
  (bool→Bool, int→Int, float→Float, String→String, Duration→Duration, Vec→List). A
  String-everywhere descriptor would advertise `kind=String` for numeric/bool params,
  producing inaccurate catalog metadata. Read each key's parser handling to pick the type
  (e.g. `.parse::<u32>()` → u32; matches "true"/"false" → bool; `.cloned()` string →
  String). `parse_uri_components` is generated but never called, so parsing-fit is
  irrelevant; only `infer_option_kind` reads the type.
- Underscore-prefixed names (`_brokers`) silence field-level dead-code warnings since the
  struct is never instantiated.
- `skip_impl` emits an inherent impl block on the descriptor with `parse_uri_components`,
  `uri_options`, and `metadata` — NOT a trait impl. `Component::metadata()` delegates to
  the descriptor's inherent `metadata()`. No `impl UriConfig` is authored.
- The runtime config struct, its `from_uri`, and any existing `impl UriConfig` (kafka has
  one) are NOT modified.
- Valid `metadata` keys: `scheme`, `description`, `producer`, `consumer`,
  `polling_consumer`, `streaming`. Use `producer, consumer` for bidirectional components;
  `producer` for producer-only. There is no `both` key.

Each task then wires delegation in `lib.rs`:
`fn metadata(&self) -> ComponentMetadata { NameMetadataDescriptor::metadata() }` replacing any
existing `ComponentMetadata::minimal(scheme)` body.

## Phase 1: High-value Components metadata

### Task P1-1: kafka metadata descriptor

- **Files**:
  - `crates/components/camel-kafka/src/metadata.rs` (new) — `KafkaMetadataDescriptor`.
  - `crates/components/camel-kafka/src/lib.rs` (modified) — `mod metadata;` + delegate `KafkaComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `KafkaMetadataDescriptor` (private, `#[derive(UriConfig)]`, `#[uri_scheme = "kafka"]`, `#[uri_config(skip_impl, metadata(scheme = "kafka", description = "Apache Kafka consumer/producer", producer, consumer), crate = "camel_component_api")]`).
  2. Read the kafka `from_uri` parser (`src/config.rs` ~L485-600). For each query key the parser reads via `parts.params.get`, add a `#[uri_param(name = "the-key")]` accurately-typed field to the descriptor. Known keys: `brokers` (`required`), `groupId`, `autoOffsetReset`, `sessionTimeoutMs`, `heartbeatIntervalMs`, `pollTimeoutMs`, `maxPollRecords`, `acks`, `requestTimeoutMs`, `securityProtocol`, `saslAuthType`, `saslUsername`, `saslPassword` (`secret`), `sslKeystoreLocation`, `sslKeystorePassword` (`secret`), `sslTruststoreLocation`, `sslTruststorePassword` (`secret`), `brokerName`, `clientId`, `allowManualCommit`, `commitTimeoutMs`, `dlqTopic`, `dlqMaxRetries`, `isolationLevel`, `partitionAssignmentStrategy`. For keys whose parser applies a default, set `default = "the-parser-default"` matching the parser.
  3. In `lib.rs`: add `mod metadata;` and `use metadata::KafkaMetadataDescriptor;`. Change `KafkaComponent::metadata()` to `fn metadata(&self) -> ComponentMetadata { KafkaMetadataDescriptor::metadata() }`.
  4. Do NOT modify `KafkaEndpointConfig`, its `from_uri`, or the existing `impl UriConfig for KafkaEndpointConfig` at config.rs:1144.
- **Tests**:
  - `name`: `kafka_metadata_uri_options_parity`
  - `setup`: the kafka `from_uri` parser at `config.rs` ~L485 is unchanged.
  - `action`: collect `KafkaMetadataDescriptor::metadata().uri_options` names; compare against the set of keys the parser reads (step 2 list).
  - `assert`: the two sets are equal; `brokers` carries `required`; `saslPassword`/`sslKeystorePassword`/`sslTruststorePassword` carry `secret`.
  - `command`: `cargo test -p camel-component-kafka --lib kafka_metadata_uri_options_parity`
  - `expected`: fails before (no descriptor), passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-kafka --all-targets -- -D warnings` exits 0.
  - `cargo test -p camel-component-kafka --lib` passes (existing tests unchanged + new parity test).
- [x] P1-1

### Task P1-2: jms metadata descriptor

- **Files**:
  - `crates/components/camel-jms/src/metadata.rs` (new) — `JmsMetadataDescriptor`.
  - `crates/components/camel-jms/src/lib.rs` (modified) — `mod metadata;` + delegate `JmsComponent::metadata()` (the Component impl is in `component.rs`; edit there if `metadata()` lives there).
- **Steps**:
  1. Create `src/metadata.rs` with `JmsMetadataDescriptor` (`#[uri_scheme = "jms"]`, metadata `producer, consumer`).
  2. Read the jms query-param parse loop (`src/config.rs` L291-364). For each query key it matches, add a `#[uri_param(name = "the-key")]` field. Worker enumerates the keys by reading the loop.
  3. Wire `JmsComponent::metadata()` delegation.
  4. Do NOT modify `JmsEndpointConfig` or its `from_uri`.
- **Tests**:
  - `name`: `jms_metadata_uri_options_parity`
  - `setup`: jms `from_uri` parser unchanged.
  - `action`: assert `JmsMetadataDescriptor::metadata().uri_options` names equal the query keys the L291-364 loop reads.
  - `assert`: sets equal; no missing/extra.
  - `command`: `cargo test -p camel-component-jms --lib jms_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-jms -- -D warnings` exits 0.
  - `cargo test -p camel-component-jms --lib` passes.
- [x] P1-2

### Task P1-3: mqtt metadata descriptor

- **Files**:
  - `crates/components/camel-mqtt/src/metadata.rs` (new) — `MqttMetadataDescriptor`.
  - `crates/components/camel-mqtt/src/lib.rs` (modified) — `mod metadata;` + delegate `MqttComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `MqttMetadataDescriptor` (`#[uri_scheme = "mqtt"]`, metadata `producer, consumer`).
  2. Parser at `src/uri.rs:37-87` reads: `topics`, `qos` (default `0`), `ackMode`, `cleanSession`, `retain`, `keepAliveSecs`, `maxPayloadBytes`, `clientId`. Add a `#[uri_param(name = "the-key")]` field for each; set `default = "0"` on `qos` (and parser defaults on others where present).
  3. Wire `MqttComponent::metadata()` delegation.
  4. Do NOT modify `MqttEndpointConfig`, `parse_mqtt_uri`, or `from_uri`.
- **Tests**:
  - `name`: `mqtt_metadata_uri_options_parity`
  - `setup`: mqtt parser unchanged.
  - `action`: assert names == `{topics, qos, ackMode, cleanSession, retain, keepAliveSecs, maxPayloadBytes, clientId}`.
  - `assert`: `qos` carries `default = "1"` (parser default AtLeastOnce) (matches parser default — exercises the default-parity scenario).
  - `command`: `cargo test -p camel-component-mqtt --lib mqtt_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-mqtt -- -D warnings` exits 0.
  - `cargo test -p camel-component-mqtt --lib` passes.
- [x] P1-3

### Task P1-4: redis metadata descriptor

- **Files**:
  - `crates/components/camel-redis/src/metadata.rs` (new) — `RedisMetadataDescriptor`.
  - `crates/components/camel-redis/src/lib.rs` (modified) — `mod metadata;` + delegate `RedisComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `RedisMetadataDescriptor` (`#[uri_scheme = "redis"]`, metadata `producer, consumer`).
  2. Parser keys (`src/config.rs` ~L583-628): `command`, `channels`, `key`, `timeout` (default from parser), `password` (`secret`), `db` (default from parser), `ssl`. Add a `#[uri_param]` field for each; set `default` to the parser's value where one exists.
  3. Wire `RedisComponent::metadata()` delegation.
  4. Do NOT modify `RedisEndpointConfig`, its manual redacting `impl Debug`, or `from_uri`.
- **Tests**:
  - `name`: `redis_metadata_uri_options_parity`
  - `setup`: redis parser unchanged.
  - `action`: assert names == `{command, channels, key, timeout, password, db, ssl}`.
  - `assert`: `password` carries `secret`; `db`/`timeout` carry the parser's defaults.
  - `command`: `cargo test -p camel-component-redis --lib redis_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-redis -- -D warnings` exits 0.
  - `cargo test -p camel-component-redis --lib` passes.
- [x] P1-4

### Task P1-5: grpc metadata descriptor

- **Files**:
  - `crates/components/camel-component-grpc/src/metadata.rs` (new) — `GrpcMetadataDescriptor`.
  - `crates/components/camel-component-grpc/src/lib.rs` (modified) — `mod metadata;` + delegate `GrpcComponent::metadata()` (Component impl may be in `component.rs`).
- **Steps**:
  1. Create `src/metadata.rs` with `GrpcMetadataDescriptor` (`#[uri_scheme = "grpc"]`, metadata `producer, consumer`).
  2. Read `parse_grpc_query_params` (`src/config.rs` L426-614) + required-param checks in `component.rs`. Enumerate every query key and add a `#[uri_param(name = "the-key")]` field: `transport` (`required`), `protoFile` (`required`), `service`, `method`, TLS/auth params, `max_receive_message_length`, `deadline_ms`, `connectTimeoutMs`, `defaultDeadlineMs`, and any others read. The legacy `tls` param is REMOVED (errors) — do NOT include it.
  3. Wire `GrpcComponent::metadata()` delegation.
  4. Do NOT modify `GrpcConfig` or its parsing.
- **Tests**:
  - `name`: `grpc_metadata_uri_options_parity`
  - `setup`: grpc parser unchanged.
  - `action`: assert metadata names equal the query keys `parse_grpc_query_params` reads (worker enumerates by reading L426-614).
  - `assert`: `transport` + `protoFile` carry `required`; `tls` is absent.
  - `command`: `cargo test -p camel-component-grpc --lib grpc_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-grpc -- -D warnings` exits 0.
  - `cargo test -p camel-component-grpc --lib` passes.
- [x] P1-5

### Task P1-6: keycloak metadata descriptor

- **Files**:
  - `crates/components/camel-component-keycloak/src/metadata.rs` (new) — `KeycloakMetadataDescriptor`.
  - `crates/components/camel-component-keycloak/src/lib.rs` (modified) — `mod metadata;` + delegate `KeycloakComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `KeycloakMetadataDescriptor` (`#[uri_scheme = "keycloak"]`, metadata `producer, consumer`).
  2. Fields = union of admin + events URI params (admin from `keycloak_endpoint.rs:56-64`; events from `events_endpoint_config.rs:73-114`): `operation` (`required`), `realm`, `userId`, `eventType` (`required` for events), `pollDelay`, `maxResults`, `lookbackWindow`, `dedupCapacity`, `maxAuthErrors`, `type`, `client`, `operationTypes`, `resourcePath`.
  3. Wire `KeycloakComponent::metadata()` delegation.
  4. Do NOT modify `AdminEndpointConfig`, `EventsEndpointConfig`, `KeycloakRealmConfig`, or `KeycloakEndpointConfig`.
- **Tests**:
  - `name`: `keycloak_metadata_uri_options_parity`
  - `setup`: keycloak configs + parsing unchanged.
  - `action`: assert names == union of admin keys (`operation`, `realm`, `userId`) ∪ events keys (`realm`, `eventType`, `pollDelay`, `maxResults`, `lookbackWindow`, `dedupCapacity`, `maxAuthErrors`, `type`, `client`, `operationTypes`, `resourcePath`).
  - `assert`: `operation` + `eventType` carry `required`; existing keycloak tests pass (configs unchanged).
  - `command`: `cargo test -p camel-component-keycloak --lib keycloak_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-keycloak -- -D warnings` exits 0.
  - `cargo test -p camel-component-keycloak --lib` passes.
- [x] P1-6

### Task P1-7: llm metadata descriptor

- **Files**:
  - `crates/components/camel-component-llm/src/metadata.rs` (new) — `LlmMetadataDescriptor`.
  - `crates/components/camel-component-llm/src/lib.rs` (modified) — `mod metadata;` + delegate `LlmComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `LlmMetadataDescriptor` (`#[uri_scheme = "llm"]`, metadata `producer`).
  2. Parser keys (`src/config.rs` ~L416-428): `stream`, `provider`, `model`, `temperature`, `max_tokens`, `system_prompt`. The `operation` (chat/embed) is the URI path, not a query param — do not include it.
  3. Wire `LlmComponent::metadata()` delegation.
  4. Do NOT modify `LlmEndpointConfig` or `from_uri`.
- **Tests**:
  - `name`: `llm_metadata_uri_options_parity`
  - `setup`: llm parser unchanged.
  - `action`: assert names == `{stream, provider, model, temperature, max_tokens, system_prompt}`.
  - `assert`: sets equal.
  - `command`: `cargo test -p camel-component-llm --lib llm_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-llm -- -D warnings` exits 0.
  - `cargo test -p camel-component-llm --lib` passes.
- [x] P1-7

### Task P1-8: surrealdb metadata descriptor

- **Files**:
  - `crates/components/camel-component-surrealdb/src/metadata.rs` (new) — `SurrealDbMetadataDescriptor`.
  - `crates/components/camel-component-surrealdb/src/lib.rs` (modified) — `mod metadata;` + delegate `SurrealDbComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `SurrealDbMetadataDescriptor` (`#[uri_scheme = "surrealdb"]`, metadata `producer, consumer`).
  2. Read `from_uri` in `src/config.rs` + param reads in `producer.rs`. Known keys: `datasource`, `query`, `table`, `id`, `op`, `allow_dynamic_query`, `edge`, `from`, `to`, `tb`/`to_table`/`from_table`, `function`, `limit`, `metric`, `output`, `path`, `value`, `vector_field`, `top_k`, `retryEnabled`, `retryMaxAttempts`, `retryInitialDelayMs`, `retryMaxDelayMs`, `retryJitter`, `retryMultiplier`. Add a `#[uri_param]` field per key; set `default` to parser defaults for the retry family + any other defaulted key.
  3. Wire `SurrealDbComponent::metadata()` delegation.
  4. Do NOT modify `SurrealDbEndpointConfig` or `from_uri`.
- **Tests**:
  - `name`: `surrealdb_metadata_uri_options_parity`
  - `setup`: surrealdb parser unchanged.
  - `action`: assert metadata names equal the keys `from_uri` reads.
  - `assert`: sets equal; `datasource` carries `required` if the parser requires it.
  - `command`: `cargo test -p camel-component-surrealdb --lib surrealdb_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-surrealdb -- -D warnings` exits 0.
  - `cargo test -p camel-component-surrealdb --lib` passes.
- [x] P1-8

### Task P1-9: wasm metadata descriptor

- **Files**:
  - `crates/components/camel-component-wasm/src/metadata.rs` (new) — `WasmMetadataDescriptor`.
  - `crates/components/camel-component-wasm/src/lib.rs` (modified) — `mod metadata;` + delegate `WasmComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `WasmMetadataDescriptor` (`#[uri_scheme = "wasm"]`, metadata `producer, consumer`).
  2. Read `from_uri` (`src/config.rs` L149-273) in full. Enumerate every query key it reads (the parser uses a non-`.get("...")` pattern — read the whole function body). Add a `#[uri_param(name = "the-key")]` field per key.
  3. Wire `WasmComponent::metadata()` delegation.
  4. Do NOT modify `WasmConfig` or `from_uri`.
- **Tests**:
  - `name`: `wasm_metadata_uri_options_parity`
  - `setup`: wasm parser unchanged.
  - `action`: assert metadata names equal the keys `from_uri` L149-273 reads (worker enumerates them).
  - `assert`: sets equal.
  - `command`: `cargo test -p camel-component-wasm --lib wasm_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-component-wasm -- -D warnings` exits 0.
  - `cargo test -p camel-component-wasm --lib` passes.
- [x] P1-9

### Task P1-10: controlbus metadata descriptor

- **Files**:
  - `crates/components/camel-controlbus/src/metadata.rs` (new) — `ControlBusMetadataDescriptor`.
  - `crates/components/camel-controlbus/src/lib.rs` (modified) — `mod metadata;` + delegate `ControlBusComponent::metadata()`.
- **Steps**:
  1. Create `src/metadata.rs` with `ControlBusMetadataDescriptor` (`#[uri_scheme = "controlbus"]`, metadata `producer`) and three fields: `#[uri_param(name = "routeId", required)]`, `#[uri_param(name = "action", required)]`, `#[uri_param(name = "authorizedRoutes", required)]`. (Per ADR-0032/0034 the target route + allowlist must be in the URI.)
  2. Wire `ControlBusComponent::metadata()` delegation.
  3. Do NOT modify the existing control-message production logic (routeId/action/authorizedRoutes enforcement stays in the producer).
- **Tests**:
  - `name`: `controlbus_metadata_uri_options_parity`
  - `setup`: controlbus producer unchanged.
  - `action`: assert `ControlBusMetadataDescriptor::metadata().uri_options` names == `{routeId, action, authorizedRoutes}`.
  - `assert`: all three carry `required`; existing controlbus tests (self-targeting denial, allowlist enforcement) pass.
  - `command`: `cargo test -p camel-controlbus --lib controlbus_metadata_uri_options_parity`
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy -p camel-controlbus -- -D warnings` exits 0.
  - `cargo test -p camel-controlbus --lib` passes.
- [x] P1-10

### Task P1-11: catalog integration test for Phase-1 schemes

- **Files**:
  - `crates/camel-core/src/component_metadata_catalog.rs` (modified) — add a test in the existing `#[cfg(test)]` module (pattern: see `catalog_exposes_registered_metadata` at L54).
  - `crates/camel-core/Cargo.toml` (modified) — add the 10 Phase-1 component crates as `[dev-dependencies]`.
- **Why here**: the catalog lives in `camel-core` (`RuntimeComponentMetadataCatalog` + `Registry`), NOT a `camel-catalog` crate (which does not exist).
- **Steps**:
  1. In `Cargo.toml` `[dev-dependencies]`, add: `camel-component-kafka`, `camel-component-jms`, `camel-component-mqtt`, `camel-component-redis`, `camel-component-grpc`, `camel-component-keycloak`, `camel-component-llm`, `camel-component-surrealdb`, `camel-component-wasm`, `camel-controlbus` (paths into `../components/`).
  2. In the `#[cfg(test)]` module, `use` the 10 Component structs (verify each component's public `Component` type name AND its constructor — several components do NOT have a zero-arg `new()`: e.g. `JmsComponent::with_scheme(scheme, pool)`, keycloak/llm/wasm may require config or provider args. Use each component's actual pub constructor or a test fixture from its own test module. If a component's constructor is too heavy for a catalog unit test, register only its metadata by constructing the descriptor directly — but prefer the real Component when feasible).
  3. Add `phase1_schemes_expose_uri_options`: `let registry = Arc::new(Mutex::new(Registry::new()));` then `let mut reg = registry.lock().unwrap();` (allow-unwrap in test), register all 10 via `reg.register(Arc::new(component));` (drop the guard after), build `let catalog = RuntimeComponentMetadataCatalog::new(Arc::clone(&registry));`, and for each scheme assert `catalog.get_metadata(scheme)` is `Some` with non-empty `uri_options`.
- **Tests**:
  - `name`: `phase1_schemes_expose_uri_options`
  - `setup`: all 10 components registered in a fresh `Registry`.
  - `action`: call `catalog.get_metadata(SCHEME)` for each of the 10 schemes.
  - `assert`: each returns `Some` and `uri_options` is non-empty.
  - `command`: `cargo test -p camel-core --lib phase1_schemes_expose_uri_options`
  - `expected`: fails until components wire `Component::metadata()` (after P1-1..P1-10); passes after.
- **Acceptance**:
  - `cargo test -p camel-core --lib component_metadata_catalog` passes (whole module).
  - `cargo clippy -p camel-core -- -D warnings` exits 0.
- [x] P1-11

## Phase 2: triage Components (explicit disposition)

Per the blessed spec: advisory = "legitimately query-minimal — `minimal(scheme)` is
correct, **no work**" (no `#[uri_param]` added); schema-blocked-deferred = wait for
namespace support (no metadata authored). Only the two annotate-disposition Components
(cxf, validator) get descriptor work.

### Task P2-1: cxf + validator annotate (descriptor structs)

- **Files**:
  - `crates/components/camel-cxf/src/metadata.rs` (new) — `CxfMetadataDescriptor`.
  - `crates/components/camel-cxf/src/lib.rs` (modified) — `mod metadata;` + delegate `metadata()`.
  - `crates/components/camel-validator/src/metadata.rs` (new) — `ValidatorMetadataDescriptor`.
  - `crates/components/camel-validator/src/lib.rs` (modified) — `mod metadata;` + delegate `metadata()`.
- **Steps**:
  1. For each component, read its `src/lib.rs` (and `src/config.rs` if present) to locate `from_uri` and enumerate its query keys.
  2. Author a `NameMetadataDescriptor` with `#[uri_param]` fields for each query key (same universal pattern as Phase 1).
  3. Wire `Component::metadata()` delegation for each.
  4. If a component genuinely has zero query keys (path-only), then its disposition is advisory (see P2-2) rather than annotate — report this and skip the descriptor.
- **Tests**:
  - `name`: `cxf_metadata_uri_options_parity` (and `validator_metadata_uri_options_parity`)
  - `setup`: each component's parser unchanged.
  - `action`: assert descriptor names equal the parser's query keys.
  - `assert`: sets equal.
  - `command`: `cargo test -p camel-component-cxf --lib cxf_metadata_uri_options_parity` (verify crate name in its `Cargo.toml`); same for validator.
  - `expected`: fails before, passes after.
- **Acceptance**:
  - `cargo clippy` clean for both crates (verify exact crate names in their `Cargo.toml` `[package] name`).
  - Existing tests pass.
- [x] P2-1

### Task P2-2: record advisory dispositions (master, template, exec) — documentation only

- **Files**:
  - `openspec/changes/component-metadata-coverage/design.md` (modified) — no source change; the disposition is already recorded here (L61).
- **Steps**:
  1. Confirm the advisory disposition for `camel-master`, `camel-template`, `camel-component-exec` is recorded (it is — design.md L61: "legitimately query-minimal; `minimal(scheme)` correct; exec is profile-driven and ignores query strings").
  2. Do NOT author any descriptor, `#[uri_param]`, or source change for these three. The spec (L85, L90-94) explicitly defines advisory as "no work" and "no `#[uri_param]` is added."
  3. These three components keep returning `ComponentMetadata::minimal(scheme)` (or whatever they return today). They remain query-minimal by design.
- **Tests**:
  - `name`: `advisory_dispositions_recorded`
  - `setup`: design.md L61 documents the three advisory components.
  - `action`: assert the three component crates have NO new `metadata.rs` file and NO `#[uri_param]` annotations introduced by this change.
  - `assert`: no source diff in `camel-master`, `camel-template`, `camel-component-exec`.
  - `command`: `git diff --name-only $(git merge-base HEAD main)...HEAD -- crates/components/camel-master crates/components/camel-template crates/components/camel-component-exec` (expect empty output)
  - `expected`: empty diff (no changes to these crates).
- **Acceptance**:
  - No code change in the three advisory crates.
  - design.md records the disposition (already present).
- [x] P2-2

### Task P2-3: record schema-blocked-deferred dispositions (xj, xslt) — file follow-ups, no code

- **Files**: none modified in source. bd follow-ups filed from the repo root.
- **Steps**:
  1. From the repo root, file two bd follow-ups: `bd create "xj metadata: open param.* namespace needs macro support" -t task -p 3 --deps discovered-from:rc-qbdt --json` and the same for xslt.
  2. Confirm the deferred disposition is recorded in design.md L62 (it is: "schema-blocked (`param.*` open-ended namespace unsupported...); deferred, out of scope").
  3. Do NOT author any descriptor or source change for xj/xslt. The spec (L86-88, L96-100) defines deferred as waiting for namespace support.
- **Tests**:
  - `name`: `deferred_dispositions_recorded`
  - `setup`: two bd follow-ups filed.
  - `action`: assert no source diff in `camel-xj`/`camel-xslt`; assert the two bd issues exist.
  - `assert`: empty source diff; bd issues created.
  - `command`: `git diff --name-only $(git merge-base HEAD main)...HEAD -- crates/components/camel-xj crates/components/camel-xslt` (expect empty); the two bd issue ids from step 1 are confirmed via `bd show`.
  - `expected`: empty diff; bd issues present.
- **Acceptance**:
  - Two bd follow-up issues created.
  - No source change in xj/xslt.
- [x] P2-3

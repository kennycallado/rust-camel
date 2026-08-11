# Design: component-metadata-coverage

## Approach

Additive metadata annotation, mirroring the rc-4cos pattern on `timer`, `direct`,
`sql`, `http`. For each Component:

1. Author a **metadata-descriptor struct** (small, private) holding only the
   `#[uri_param]` URI fields for that Component's scheme. Derive `UriConfig` on the
   descriptor — NOT on the runtime config struct. This is the universal pattern: the
   `camel-endpoint-macros` derive (`uri_config.rs:856-865`) enforces a **single**
   non-`#[uri_param]` "path" field per struct, so any runtime config with two or more
   non-URI fields (path-derived names, resolved values, injected handles, connection
   state) fails to compile when derived directly. The descriptor sidesteps this entirely
   and leaves the runtime config struct untouched. (camel-timer derives directly because
   `TimerConfig` has exactly one path field + Duration companions; production components
   with mixed fields cannot.)
2. Add the **required** scheme attribute on the descriptor: `#[uri_scheme = "<scheme>"]`.
3. Add `#[uri_config(skip_impl, metadata(scheme = "<scheme>", description = "..",
   producer, consumer), crate = "camel_component_api")]`. Valid `metadata` keys are
   `scheme`, `description`, `producer`, `consumer`, `polling_consumer`, `streaming` —
   there is NO `both` key (use `producer, consumer` for bidirectional components).
   `skip_impl` makes the derive emit ONLY the inherent `metadata()` / `uri_options()`
   helpers (plus `parse_uri_components`) as an `impl #struct_name { .. }` block — NOT a
   trait `impl UriConfig` (`uri_config.rs:998-1010`). So `Component::metadata()` can call
   `XxxMetadataDescriptor::metadata()` (inherent) with no trait impl required.
4. `#[uri_param(name = "..", default = "..", required|secret|deprecated|aliases|kind)]`
   on each descriptor field mapped to a real URI query param the Component parses today.
   The `name`/`alias` MUST equal the key the existing `from_uri` accepts (manually
   enforced parser/metadata parity — the spec makes this an executable per-Component test).
5. Wire `Component::metadata()` to delegate: `XxxMetadataDescriptor::metadata()` (the
   inherent fn the derive generates), replacing any `ComponentMetadata::minimal(scheme)`
   placeholder.

**Runtime config + parsing are NOT touched.** The existing runtime config struct, its
manual `from_uri`, and any existing `impl UriConfig for XxxConfig` (kafka has one at
`config.rs:1144`) stay byte-identical. The descriptor is metadata-only; it is never
instantiated and its `parse_uri_components` is never called. No new trait impl is
authored for any clean-slate Component. Zero behavioral risk to route execution;
existing tests stay green.

**Param sourcing:** read each Component's accepted keys from its own `from_uri` /
`parts.params.get(...)` code. Apache Camel docs enrich `desc`/`deprecated`/`aliases`
only — the code is the source of truth for which params exist.

## Affected crates

Each Phase-1 Component gains a private metadata-descriptor struct (e.g.
`KafkaMetadataDescriptor`) in its `src/` tree, with `#[derive(UriConfig)]` +
`#[uri_param]` fields for that scheme's accepted query keys. The runtime config struct
and `from_uri` are not modified.

Phase 1 (10, real params):
- `camel-kafka` (brokers, groupId, autoOffsetReset, securityProtocol, sasl*, ssl* …)
- `camel-jms` (broker, destinationType, acknowledgementMode, …)
- `camel-mqtt` (topics, qos, ackMode, cleanSession, clientId, …)
- `camel-redis` (command, key, channels, timeout, password, db, ssl)
- `camel-component-grpc` (transport, protoFile, service, method, …)
- `camel-component-keycloak` (operation, realm, userId, eventType, pollDelay, …)
- `camel-component-llm` (provider, model, temperature, max_tokens, stream, system_prompt)
- `camel-component-surrealdb` (datasource, query, table, id, op, retry*, …)
- `camel-component-wasm` (parser keys read by `from_uri`)
- `camel-controlbus` (routeId, action, authorizedRoutes — security-critical capability-authz params)

Phase 2 (explicit disposition):
- `camel-cxf`, `camel-validator` — annotate (real params).
- `camel-master`, `camel-template`, `camel-component-exec` — advisory (legitimately query-minimal; `minimal(scheme)` correct). exec is profile-driven and ignores query strings.
- `camel-xj`, `camel-xslt` — schema-blocked (`param.*` open-ended namespace unsupported by exact `UriOption` names); deferred, out of scope.

No change to `camel-endpoint-macros` (derive), `camel-api` (`ComponentMetadata`),
`camel-core` (catalog/harvesting), or `camel-lint` (consumer). Components-layer only.

## Architecture boundaries

Components-layer only. Enriches component **self-description** (control plane: catalog,
lint, docs); does not touch the data-plane route execution path. No Runtime, DSL,
Services, or Languages change. The consumer (`camel-lint`) reads metadata through the
existing `ComponentMetadataCatalog` trait in a separate worktree with zero coupling.

## Phases

### Phase 1: high-value Components (10)
- **Goal:** each production-critical Component scheme reports non-empty `uri_options`
  for its meaningful params, with executable parser/metadata parity tests.
- **Dependency:** none (rc-4cos archived; macro + catalog stable).
- **Exit criteria:** for each of the 10 schemes, `get_metadata(s).uri_options` non-empty
  with names matching a reviewed parser-key fixture; per-crate
  fmt/clippy/test/lint-non-exhaustive green.

### Phase 2: triage Components (7, explicit disposition)
- **Goal:** each of the 7 has a recorded disposition — annotate (cxf, validator),
  advisory/minimal (master, template, exec), or schema-blocked-deferred (xj, xslt).
- **Dependency:** Phase 1 complete (pattern routine by then).
- **Exit criteria:** each annotated Component meets the Phase-1 gate bar; each advisory
  one documented with the reason `minimal(scheme)` is correct; each deferred one records
  the open-namespace blocker.

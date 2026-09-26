# runtime-boot Specification

## Purpose
TBD - created by archiving change integration-tier-contract. Update Purpose after archive.
## Requirements
### Requirement: Bundle cascade parity

The system SHALL register the same component bundles from the same
`Camel.toml` whether boot flows through `camel run` or through the embedded
harness, through a single cascade owned by `camel-bundles`.

#### Scenario: identical registration through both boots

- **GIVEN** a `Camel.toml` configuring http, file, container, and template
- **WHEN** the runtime boots through `camel run` and through the harness boot
- **THEN** both CamelContexts hold the same registered component set and the
  same per-bundle configuration

#### Scenario: feature forwarding

- **GIVEN** a consumer that enables the `kafka` bundle feature
- **WHEN** the boot registers bundles
- **THEN** the forwarded feature selection decides bundle availability,
  identical for CLI and harness consumers

### Requirement: BootHandle lifecycle

The system SHALL own bridge cleanup and pool teardown in a `BootHandle`
returned by the shared boot, with an explicit `shutdown()`, and the CLI
SHALL keep the watcher, signal handling, exec guard, and operator logging.

#### Scenario: explicit teardown

- **GIVEN** a boot that started jms and cxf pools
- **WHEN** `BootHandle::shutdown()` completes
- **THEN** pools and bridge cleanup are closed and no leaked tasks remain

#### Scenario: CLI ownership unchanged

- **GIVEN** a `camel run` process under the extracted boot
- **WHEN** the file watcher fires or the user sends Ctrl+C twice
- **THEN** behavior is identical to the pre-extraction path

### Requirement: Shared security and startup wiring

camel-bundles SHALL own the composition-root wiring that `camel run`
performs before route loading: (a) building the security compile context
from `CamelConfig` (authenticator resolution, provider registration,
keycloak/UMA evaluator wiring, and — when the wasm feature is enabled —
wasm security-policy and permission registries) behind a `security`
feature gate that adds the camel-auth, camel-component-keycloak, and
camel-dsl dependencies, and (b), always available without any feature
gate, installing `[binds]` public-exposure acknowledgements into the
context-level gate and — under the existing mcp/wasm component features —
the MCP registry gate and the wasm source-bind gate (ADR-0061 Rule 4,
fail-closed), and (c), always available without any feature gate,
registering ADR-0033 fail-closed SQL startup checks derived from the
discovered route definitions. `camel run` SHALL delegate to these
helpers, SHALL enable the security feature in its default feature set so
default-build behavior is unchanged, and its externally observable
behavior SHALL NOT change.

#### Scenario: camel run delegates without behavior change

- **GIVEN** a project with security, binds, and sql configuration
- **WHEN** `camel run` boots through the camel-bundles helpers
- **THEN** the booted context matches the pre-change behavior on every
  existing run test (security policies resolve, bind acks gate identically,
  SQL startup checks reject identically)

#### Scenario: both callers boot a security_policy route

- **GIVEN** one CamelConfig with native security whose route declares a
  `security_policy`
- **WHEN** `camel run` boots it and the integration harness boots the same
  project through the shared helpers
- **THEN** both boots succeed in starting the route, and in both boots a
  request with valid native credentials passes the policy while a request
  without credentials is refused

#### Scenario: both callers refuse an unacknowledged public bind

- **GIVEN** one CamelConfig declaring a non-loopback bind serving a
  `Public` route without `allow_public_exposure`
- **WHEN** `camel run` boots it and the integration harness boots the same
  project through the shared installer
- **THEN** both boots fail closed at context start with the ADR-0061
  acknowledgement error

#### Scenario: security feature absent fails closed

- **GIVEN** a `CamelConfig` declaring `[security]` sections and a build
  where the camel-bundles security feature is disabled
- **WHEN** a caller runs the ungated `ensure_security_supported` check
  before booting
- **THEN** the check returns a configuration error naming the required
  feature, before any route compiles

#### Scenario: ungated helpers build without the security feature

- **GIVEN** a build of camel-bundles with the security feature disabled
- **WHEN** a caller uses the bind-ack installer or the SQL startup-check
  installer
- **THEN** both compile and install their wiring (the helpers use only
  camel-bundles' existing hard dependencies)

#### Scenario: feature forwarding with default enablement

- **GIVEN** camel-cli built with its default feature set
- **WHEN** it depends on camel-bundles
- **THEN** the camel-bundles security feature is enabled (the forwarding
  feature is part of the CLI defaults) and the shared helpers are
  available to the CLI boot

### Requirement: Context drop terminates controller tasks

The system SHALL terminate the route-controller actor task and the
supervision task when a `CamelContext` is dropped. Dropping the context
is a NON-graceful termination: it does not replace route and service
teardown, which callers SHALL drive with `stop()` before dropping.
`stop()` SHALL continue to keep the actor alive for a subsequent
`start()`; only `abort()` and dropping the context SHALL be destructive.

#### Scenario: scenario-tier batch does not accumulate tasks

- **GIVEN** a process that boots, runs, stops, and drops one full
  `CamelContext` per scenario document, for N documents
- **WHEN** the batch completes
- **THEN** the process thread count and open file-descriptor count are
  the same (±1) as after the first document

#### Scenario: registered components drop with the context

- **GIVEN** a booted `CamelContext` and a probe component registered in
  the context's component registry, owned exclusively by that registry
  (its `Drop` sets an `Arc<AtomicBool>` flag)
- **WHEN** the context is stopped and dropped
- **THEN** the probe flag is set within a 5-second bounded wait

#### Scenario: stop-start restart is unaffected

- **GIVEN** a booted `CamelContext`
- **WHEN** `stop()` then `start()` complete
- **THEN** routes restart through the still-alive controller actor and
  subsequent stop-then-drop still terminates the actor

### Requirement: Component-context registry reference is non-owning

`RegistryComponentContext` SHALL hold a non-owning (weak) reference to
the component registry. Resolution through the context SHALL return
`None` when no strong reference to the registry remains. No component
bundle construction SHALL be able to keep the component registry alive
after its owning context is dropped. Programs that build a standalone
registry for the context SHALL keep a strong anchor alive for as long as
they need resolution to work.

#### Scenario: wasm bundle does not pin the component registry

- **GIVEN** a booted `CamelContext` whose component registry includes
  the wasm-bundle-registered component and a probe component owned
  exclusively by that registry
- **WHEN** the context is stopped and dropped
- **THEN** the probe component's `Drop` runs (the registry→component→
  context→registry cycle does not outlive the context)

#### Scenario: standalone registry resolves while anchored

- **GIVEN** a standalone `Registry` with a registered component, a
  `RegistryComponentContext` built over it, and a strong anchor `Arc`
  to the registry still held
- **WHEN** `resolve_component` is called for the registered scheme
- **THEN** it returns the component

#### Scenario: resolution after registry drop returns None

- **GIVEN** a `Registry` with a registered component and a
  `RegistryComponentContext` built over it, and the last strong
  registry reference dropped
- **WHEN** `resolve_component` is called for the registered scheme
- **THEN** it returns `None`

### Requirement: Auto-startup routes start on every boot with a durable journal

When a durable runtime journal is configured, the system SHALL start every
route whose definition has `auto_startup` enabled on every boot, regardless
of route lifecycle events or command IDs recorded in the journal by earlier
boots. Context lifecycle command IDs SHALL be unique per boot by deriving a
boot nonce from the recovered durable dedup store at journal recovery; the
nonce SHALL be a deterministic function of the recorded command IDs (no wall
clock), and no issued command ID SHALL equal a command ID still recorded in
the store. When the deterministic nonce value space is exhausted, the system
SHALL fail the boot with an explicit startup error naming the offending
recorded ID instead of issuing a command ID. Journals written by earlier
versions (four-segment command IDs, including route IDs that themselves
contain colons) SHALL interoperate without suppression.

#### Scenario: Full lifecycle sequence on every boot

- **GIVEN** a durable journal at a fresh path and one route with
  `auto_startup` enabled
- **WHEN** the context boots, stops, and is dropped, then boots again on the
  same journal path, and this cycle repeats once more
- **THEN** every boot appends `RouteRegistered`, `RouteStartRequested`, and
  `RouteStarted` to the journal for that route, and the route reports the
  `Started` state at the end of every boot

#### Scenario: Stop commands are accepted on every boot

- **GIVEN** a durable journal that has already recorded one full boot cycle
  (start and stop) for an auto-startup route
- **WHEN** the context boots again on the same journal and later shuts down
- **THEN** the second boot's `StopRoute` command is accepted (not classified
  as a duplicate) and `RouteStopped` is appended to the journal

#### Scenario: Boot nonce derives from the recorded dedup store

- **GIVEN** a journal whose dedup store records command IDs from prior boots
- **WHEN** a new boot recovers the journal and issues its first context
  command
- **THEN** the command's ID differs from every ID recorded in the store,
  and two recoveries over identical journal state derive the same nonce
  (identical state yields identical nonces, without reading the wall clock;
  full IDs additionally depend on the per-process issuance order)

#### Scenario: Legacy recorded IDs never collide

- **GIVEN** a journal written before this change whose dedup store records
  four-segment context command IDs (`context:{op}:{route_id}:{seq}`),
  including at least one whose route ID contains a colon so the recorded
  string's final segments are numerically ambiguous (for example
  `context:start:foo:0:0` for route `foo:0`)
- **WHEN** a new boot derives its boot nonce and issues five-segment IDs
  (`context:{op}:{route_id}:{nonce}:{seq}`)
- **THEN** the chosen nonce is strictly greater than the penultimate segment
  of every recorded ID whose final two segments both parse as integers, so
  no issued ID equals a recorded ID, and every auto-startup route's
  `StartRoute` command is accepted rather than classified as a duplicate

#### Scenario: Exhausted deterministic nonce space

- **GIVEN** a journal whose dedup store records a command ID whose final two
  segments both parse as integers and whose penultimate segment equals the
  maximum 64-bit unsigned integer value
- **WHEN** a new boot derives its boot nonce
- **THEN** the boot fails closed: an explicit startup error names the
  offending recorded ID and tells the operator to clean or rotate the
  journal, and no context command ID is issued for that boot

#### Scenario: No journal configured changes lifecycle outcomes

- **GIVEN** a context built without a runtime journal
- **WHEN** the context boots
- **THEN** auto-startup proceeds through the same command path with the same
  lifecycle outcomes as before this change (no journal recovery runs; the
  command ID string gains only a constant zero nonce segment)


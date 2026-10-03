# Integration Test

Scenario-tier test support for rust-camel. This crate owns the scenario
document model and parser for `.test.yaml` documents that declare a
`scenario:` section, with the vocabulary ban, the provisioning gate, and
the reserved env-key rule. The tier derivation, the layered environment
source, the action runner, the partner adapters, and the embedded
FULL-tier boot are built on these model types.

> **Scope boundary.** This file defines only the scenario document
> vocabulary and its parse-time rules. The unit-tier document vocabulary
> (`inputs`, `expects`, `intercepts`, `settle`) stays with
> [`crates/camel-cli/CONTEXT.md`](../camel-cli/CONTEXT.md) until the
> runner work re-homes it. Boot and bundle registration terms live in
> [`crates/camel-bundles/CONTEXT.md`](../camel-bundles/CONTEXT.md).

## Language

**scenario document**:
A `.test.yaml` (or `.test.yml`) document that declares a `scenario:`
section. Tier derivation reads the same file format; a `scenario:`
section forces the FULL tier (ADR-0069 section 1).
_Avoid_: integration test file (the file is not integration-specific),
scenario spec

**action**:
One ordered step of a `scenario:` list: `send`, `receive` (mandatory
`deadline`), `sleep`, or `validate`. Each list item is a single-key map
(`- send: {...}`); dispatch runs in validation, not serde, so every
failure carries the action index.
_Avoid_: step (steps belong to routes), command

**partner**:
The far end of the wire under the harness's control. The harness owns a
listener on the other side of the connection; what arrives there is the
normative proof (ADR-0069 section 5).
_Avoid_: mock (mocks are unit-tier tools), remote service

**tier**:
The derived execution profile of a test document (LEAN or FULL), a pure
function of document content. No field declares it.
_Avoid_: mode (filters, not modes), level

**endpoint reference**:
`EndpointRef`: an endpoint URI plus optional `provisioning` and
`bindVar`. Deserializes from a bare string or a map with `endpoint`,
`provisioning`, and `bindVar` keys.
_Avoid_: endpoint URI (the reference carries more than the URI),
logical endpoint

**provisioning**:
Who owns the partner lifecycle. Only `harness` (in-process listener on
`127.0.0.1:0`) is implemented in v1; `testcontainer` and
`user-provided` are reserved grammar values the parser rejects
(`DocError::UnsupportedProvisioning`, `infra-unavailable` class).
_Avoid_: provider, backend

**bind variable**:
The `bindVar` scenario variable the harness fills with an endpoint's
bound address when provisioning is `harness`.
_Avoid_: port variable, env override

**partner validate target**:
The `ScenarioTarget::Partner` case of a `validate` target. The
assertion reads the partner's recorded request traffic. The endpoint
URI must equal a harness endpoint reference declared by the scenario's
own `send`/`receive` actions, or self-declare the reference: an object
form with `provisioning: harness` on an `http` URI that also has a `partners:` entry
naming it. A self-declared target wires the partner like a
`send`/`receive` reference: the driver binds it, fills the `bindVar`,
and exposes it through the harness-provisioned env fold, so a
validate-only proxy scenario (per-request-varying queries, no literal
arrival lane) runs with no sacrificial receive. The expectation's
`requests` list adds per-request shape asserts, positional over the
filtered recorded sequence: each entry carries the filter trio
(`method`, one `path` form, a `query` subset) plus a `body`
expectation in the dual grammar. Declaring `requests` excludes the
count bound keys — the list length synthesizes the exact count — and
any present mismatched element fails the poll immediately: the
recorder is append-only, so a mismatch never heals.
_Avoid_: mock expectation (a partner target asserts recorded wire traffic)

**request shape**:
One entry of a partner expectation's `requests` list
(`camel_matchers::RequestShape`): the same filter trio a partner
expectation carries — `method`, one `path` form, a `query` subset —
plus an optional `body` expectation over the projected body value
(JSON when the bytes parse, lossy text otherwise). Entries zip
positionally with the filtered recorded sequence; a mismatch names
the failed request (one-based within the filtered sequence) and the
failed aspect (`method`, `path`, `query`, `body`); every entry key is
optional — the empty map asserts only existence.
_Avoid_: request template, per-request mock

**expectation**:
The matcher grammar of a `validate` action. Keys mirror the mock-testkit
matcher rules: `equals`, `regex`, `contains`, `startsWith`, `endsWith`,
`exists`, `jsonSubset`. A bare value is a literal `equals`; an object
with one recognized matcher key is that matcher; any other object is a
literal `equals`.
_Avoid_: assertion (assertions are the runner's verdict), matcher map

**surreal prepare action**:
The `surreal:` state action: a `datasource` name plus a non-empty
ordered `prepare` list of SurrealQL write statements, run in order
over the boot's catalog client. The load-time read gate rejects a
`select`-prefixed statement (whitespace trimmed, one leading
parenthesis group unwrapped) and an empty list, in both feature
configurations. SurrealQL has no other read prefix, so `select` is
the only banned one. A build without the `surreal` feature fails a
declaring document at load with the named demand-gate error.
_Avoid_: SQL prepare (the family keys are distinct), seed action

**surreal validate target**:
The `ScenarioTarget::Surreal` case of a `validate` target: exactly
one `select`-prefixed read statement over a named datasource, the
shared rows and count-bound expectation grammar, and `deadline`
validity. The read is single-statement: a `;` separator followed by
further non-whitespace text is a load error. One surreal deviation: the driver
returns key-sorted objects, so a `rows` expectation without
`columns` is a load error. Count bounds do not need `columns`.
_Avoid_: SQL validate target (the family keys are distinct), record
query

**elapsedAtLeast**:
The `validate` timing assertion on a `lastReceived` target: the last
received message's wire arrival must be at least this long (humantime)
after the scenario start. Anchored to wire arrival, never the
consumption time — a message consumed late can still have arrived
early. The not-before-X control `run.sh` expressed with `awk`.
_Avoid_: minimum age, delay assertion, consume-time check

**vocabulary ban**:
The load-time rule that a document with `scenario:` must not declare
`inputs`, `expects`, or `intercepts` (`DocError::MixedVocabulary`,
`doc-validation` class). One vocabulary per tier, and per document.
_Avoid_: mixing check, format split

**reserved env key**:
A document `env` key that equals any `bindVar` declared by the
document's own endpoints (`DocError::ReservedEnvKey`). The reserved set
is static and document-derivable; the harness binding wins.
_Avoid_: env conflict, shadowed variable

**scenario boot**:
`boot_scenario(doc, root, env)`: the embedded FULL-tier composition
root, delegating to the shared `camel run` wiring in the same order
(ADR-0069 section 10): sealed config load (`from_file_sealed`, pinned
profile, no ambient `CAMEL_*` overrides), `configure_context_with_beans`,
the offline security gate, the security compile-context build (the shared
builder behind the `security` feature; the fail-closed guard without it),
`install_bind_exposure_acks`, `camel_bundles::boot`, route discovery
through `discover_routes_with_threshold_security_and_env` with `${env:}`
resolution through the `LayeredEnv`, `install_sql_startup_checks`, route
registration, `ctx.start()`. Returns `ScenarioRun { ctx, boot }`;
partners stay caller-owned.
_Avoid_: full boot (the tier name is FULL, the function is the scenario
boot), embedded runner

**route stimulus**:
A scenario `send` addressed to a context component endpoint
(`direct:`): it reaches the booted system under test through the
context's own producer path (`DirectStimulus`, the camel-test / camel
run mechanism), not through a partner. Partner-scheme sends dispatch
through the `PartnerRouter`.
_Avoid_: input delivery (unit-tier term), trigger

**arrival lane**:
The per-request-path queue a partner listener feeds and a server-role
`receive` drains. The path is the part of the endpoint URI a listener
can discriminate; lane depth is capped (`ARRIVAL_LANE_CAPACITY`), and a
full lane drops the queue entry while the recorder keeps it.
Resolution is per authority: a dynamic reference resolves the
registered partner by authority and keeps its path on the wire, so one
declared endpoint with one `bindVar` serves every path a route dials.
Lanes exist per path on that single listener. A dynamic-reference
receive drains its own path's lane: a receive `from:
http://${MOCK}/billing` drains the billing lane on the already-bound
listener. The registered key remains the adapter-lookup key; the path
on the wire picks the lane. A dynamic receive must name a path — a
bare authority is an apparatus error.
_Avoid_: inbox, backlog

**selector**:
The dotted grammar of `receive.extract` reads: heads `body`, `headers`,
`status`, `method`, `path`. Header lookup is ASCII-case-insensitive
(hyper lowercases wire names; the fake preserves author casing). The
transport-scalar heads carry no sub-path.
_Avoid_: path expression, jsonpath

**partner body encoding**:
The wire encoding of a partner script's `response.body`, mirroring the
client send path (`value_to_wire`, one encoding for both wire roles):
a string serves as its exact bytes (no surrounding quotes, no
escaping), null serves empty, any other value serves as compact JSON,
and an absent body serves empty.
_Avoid_: JSON serialization (strings are not quoted), double encoding

**traffic adapter**:
An adapter that owns the harness side of the wire: what arrives at the
harness listener is the normative proof. The partner family is the only
traffic adapter; the `http` feature activates it (ADR-0069 section 14).
_Avoid_: wire adapter, partner adapter

**state adapter**:
An adapter that asserts over data at rest through a datasource catalog
pool, observed through the same boot the system under test runs on. The
SQL and Surreal families are state adapters, activated by the `sql` and
`surreal` features (ADR-0069 section 14).
_Avoid_: database adapter, DB tier

**steering axis**:
The shared datasource seam (`src/steering.rs`): it steers a datasource
name to an env-steered URL and then to a typed pool handle through one
resolver. It carries the identifier law (errors name the datasource,
never its URL) and the ADR-0051 redaction law (ADR-0069 section 14).
_Avoid_: datasource lookup, connection helper

**poll driver**:
The shared deadline poll seam (`src/runner/poll.rs`): one discipline for
partner, SQL, and Surreal polling. The expiry instant is fixed before
the first snapshot, a snapshot error stops the poll at once, early
judgment precedes the expiry decision, and a sleep never exceeds the
remaining window (ADR-0069 section 14).
_Avoid_: retry loop, wait helper

## `#[non_exhaustive]` posture

ADR-0049 governs public enums. Every public enum in this crate is
`#[non_exhaustive]` from birth: `RouteSource`, `ScenarioAction`,
`ScenarioTarget`, `Provisioning`, `Expectation`, and `DocError`. The
public structs (`ScenarioDocument`, `EndpointRef`) are out of the enum
mandate.

## Architecture notes

### Two-stage parse with index-carrying errors

Serde deserializes into a raw form where action lists stay raw values
and durations stay strings. Validation then converts to the public
model. This split exists so a missing `deadline` or a bad duration can
name the action index (`scenario[2]`), which a serde field error cannot
do. The `settle` field of the unit-tier parser is the precedent for
string-then-parse durations.

### `RouteSource` paths stay as declared

`routeFiles` and `routeFilesFromRoot` are stored as written. Resolving
them against the document directory or the project root is the runner's
job, the same split the unit-tier parser keeps. Inline `routes` parse
eagerly through the shared `camel_dsl::parse_yaml`, wrapped back into a
top-level `routes:` key like the unit-tier runner does.

### `RouteDefinition` is neither `Debug` nor `Clone`

`RouteSource` therefore has a manual `Debug` (the inline form reports
its route count) and no `Clone`. Wrapping or deriving would change the
shared route model, which this crate must not do. The same limitation
bounds the scenario boot: `boot_scenario` receives the document by
reference, so inline route definitions cannot move into the context and
the boot rejects `RouteSource::Inline` (`v1`; declare `routeFiles`).

### Error classes map to exit codes by variant

Classification is by `DocError` variant, never by message text; the CLI
adapter owns the mapping and every variant maps to exit 2. Variants
carrying the `doc-validation:` token in Display: `NotTestDocument`,
`MissingScenario`, `MixedVocabulary`, `Validation`, `ReservedEnvKey`,
`InlineRoutes`, `InlineRoutesRejected`, `ProvisioningWithoutAuthority`,
`ExpectReplyOnUnsupportedSend`. `UnsupportedProvisioning` and the
boot's `AuthProviderUnavailable`
rejection (keycloak/oidc config) name the `infra-unavailable` class
(ADR-0069 section 7). `RouteSourceMissing`
and `RouteSourceConflict` render the unit-tier messages verbatim,
without the token, and map to exit 2 as doc parse errors, exactly as
the unit tier maps them today. This crate never exits.

v1 inbound bound: `inbound:` documents are boot-owning library callers
only — the CLI `test` command cannot run them (a document-side
`${INBOUND}` action endpoint fails the BOOT_SCHEMES gating into the
named infra-unavailable smoke path, exit 2).

### Regex expectations compile-verify at load

`regex` patterns compile-verify at parse time through the `regex`
crate, aligned with the unit-tier matcher rules. Payload shapes
(string for text matchers, null for `exists`, object for `jsonSubset`)
are load-time rules too.

### Dependency direction

`camel-core` supplies `RouteDefinition` (and, for later phases,
`InterceptAction`); camel-dsl does not re-export them. ADR-0069 section
10 permits this direction: testing crates depend on core, never the
reverse. ADR-0055 forbids depending on `camel-test`, the publish-order
leaf sink. The scenario boot additionally depends on `camel-config`
(the sealed loader) and `camel-bundles` (the `camel run` registration
cascade and the `security_boot` wiring) — the composition-root direction
ADR-0069 section 10 fixes: testing crates consume the boot, the engine
never consumes the testing crates.

### The scenario boot seals hermeticity twice

`boot_scenario` loads `<root>/Camel.toml` through
`CamelConfig::from_file_sealed`: the profile is pinned by value
(`&str`, defaulting to the document's `profile` or `"default"`), and
the `CAMEL_*` allowlist override merge is off. `${env:}` placeholders
in the config and in route files resolve through the `LayeredEnv`
(`interpolate_env_with`), never the process environment — the harness
must not read global state (ADR-0069 section 4). Route files load
through `discover_routes_with_threshold_security_and_env`, the
env-injected discovery entry: the injected lookup is the `LayeredEnv`,
so the process environment is never consulted, and the full discovery
contract is preserved (glob patterns, the reserved test-suffix gate,
JSON explicit-pattern gating, file size caps, two-pass template
materialization) with the config's `stream_caching.threshold` and the
built security compile context.

The boot also guards the sqlite hermetic default. A `sqlite::memory:`
datasource URL without `cache=shared` is rejected during boot, before
context preparation, with the `sql-memory-not-shared` error
(`ensure_sqlite_memory_shared`): the
`:memory:` database is per-connection, so pooled statements would read
different databases and a validation could pass against state the
document never seeded.

### Boot teardown closes the datasource pools

`BootHandle::shutdown_with_deadline` ends with a deadline-wrapped
`datasource_catalog.close_all()` (bd rc-25lup.4): the sqlx factory's
`close` drains each pool, so an in-memory sqlite database dies with its
boot and a later document booting the same alias in the same process
starts empty. The `boot_freshness` tests pin the guarantee in its
load-bearing shape — a named shared-memory URI
(`sqlite:file:<name>?mode=memory&cache=shared`), where connections
share one named database and a lingering boot-A connection demonstrably
leaks rows into boot B without the close (count 2, red-able). The
named-memory regression fixture derives its URI name from the test's
temporary project identity. Every invocation owns a fresh alias, boot A
and boot B share the exact same URI, and a sibling test can never hold
that alias open across the boot A teardown window. The happens-before
proof is the awaited `shutdown` future plus the landed SQL pool drain:
close disables in-memory pool resurrection, awaits pool close, and
observes the drain of every pooled connection. Boot B cannot create its
pool before boot A's close future resolves, so the fixture needs no
wall-clock sleep as a state signal and stays deterministic without
delays. A failure in that fixture belongs to the distinct SQLite
named-memory teardown class, not to the JWKS cooldown wall-clock class
(bd rc-7dfyq), the camel-http consumer readiness class, or the macOS
errno class (bd rc-62me6). The tests also pin the opposite contract for
file-backed datasources, whose rows persist and whose cleanup is the
document author's clean-first prepare (ADR-0069 section 9 mirror).
The default-no-op `close`/`close_all` trait methods
live in `camel-api`; only pool-owning providers override them, and
close resolves factories by NAME, never by registry key (the handle
stores `factory.name()`). The scenario-tier convention for memory
fixtures is the named shared-memory URI (bd rc-gcf9n):
`sqlite:file:memdb_demo?mode=memory&cache=shared`. Every pool
connection shares one named in-memory database, so `max_connections`
is selected for the workload, and the pool-factory probe
`named_shared_memory_uri_probe` pins the multi-connection sharing.
The named form also pins `provider = "sqlx"` because `sqlite:file:`
matches no automatic factory prefix, and the boot lint
`sql-memory-not-shared` steers authors to it.
The bare `sqlite::memory:?cache=shared` form is still accepted, but
with the Any driver each pooled connection can hold a private in-memory
database there, so that form pins `max_connections = 1`. Parallel
document execution is a known limitation (bd rc-gcf9n): two
concurrently booted documents sharing one memory name could collide;
the remedy when parallel lands is a per-boot unique suffix
(`memdb_{scenario}_{boot}`). This note is informational, not a rule
for the sequential runner.

### The surreal state family mirrors the sql family

The `surreal:` prepare action and the surreal validate target reuse
the sql family's shapes: ordered non-reading prepare statements, the
single-catalog resolve, a select-prefix read gate, the shared rows
and count-bound expectation grammar, the ORDER BY advisory, and the
poll semantics (no early settle, the fixed interval, the immediate
ceiling breach). Two deviations are load-bearing:

- The surreal read gate bans only the `select` prefix, and the
  validate query is additionally single-statement: a `;` separator
  followed by further non-whitespace text is a load error. The check is textual. A
  `;` inside a string literal false-positives, and the author
  restructures the query.
- A `rows` expectation requires `columns`: the driver returns
  key-sorted objects, so projection order is not recoverable. Record
  ids project as `table:key` strings. The value mapping fails closed
  on every kind without an exact matcher form (decimals, non-finite
  floats, Bytes, Duration, Geometry, Table, File, Range, Regex,
  Set), naming the field and its SurrealQL type. Recursion never
  launders a nested unsupported kind.

Teardown joins the family: the surreal factory's close hook issues
`invalidate()` on every client the boot built (hygiene on the
auth-free embedded `mem://` tier, session termination on remote
tiers), so a later boot over the same alias resolves a new client
against a fresh empty instance. The `mem://` scheme skips signin and
defaults `namespace` and `database` to `test` and `test`. Pinned by
the [integration-tier spec](../../openspec/specs/integration-tier/spec.md)
(State prepare actions, Surreal state assertion, Scenario datasource
teardown) and ADR-0069 section 8.

### The scenario tier gates security offline

The tier runs offline: no network. Keycloak/oidc security configuration
(network-prefetching auth providers) is rejected before any builder call
with `CamelError::AuthProviderUnavailable`; the CLI adapter classifies
that variant as the `infra-unavailable` document-error class (exit 2),
never `full-boot-failure`. Wasm `security.policies`/`security.permissions`
are rejected fail-closed with a configuration error naming the v1 tier
limitation. The remaining `[security.*]` sections (native) build through
the shared `camel_bundles::security_boot` builder behind the `security`
feature; without the feature, `ensure_security_supported` rejects any
configured section before a route compiles. Like `security`, the `sql`
feature forwards `camel-bundles/sql` (the harness is a camel-bundles
consumer, and the gate-forwarding lint requires a shadow-named feature
to forward its gate).

### Partner receives resolve the wire role by dispatch state

The router's `receive` first probes the client lane for a roundtrip
parked by a client-role `send`; the parked response wins (client
role), and the probe keys on the registered partner key joined with
the wire path-and-query of the interpolated reference — parking is
path-aware, bounded FIFO per key, oldest-first within one path. A
probe miss falls to the partner adapter, which awaits the next
arrival queued on the endpoint's request path (server role), bounded
by the deadline. Server-role arrivals queue per path. Arrivals map
into `IncomingMessage` with the request line (`method`, `path`) and
`status: None` — requests carry no status; status validation is
inbound work.

### Partner lanes key on strict wire bytes

Partner arrival lanes key on the strict wire `path_and_query` bytes of each
recorded request; receive paths derive from the declared partner endpoint URI.
The key is never canonicalized, so producer-side byte drift stays detectable.
When matching fails the harness reports the wire evidence: receive-timeout
errors list the wire paths that arrived in the lane group
(`ReceiveTimeout.lanes_recorded`), and partner-count mismatches list the
recorded paths.

### Partner validate matches by bound-aware windows

A partner `validate` expectation carries exactly one count bound: `count`
(exact), `atLeast`, `atMost`, or `atLeast` combined with `atMost` (a range).
Optional filters narrow the counted requests. `method` compares
ASCII-case-insensitively. One path filter reads the recorded path-and-query:
`path` compares strict bytes, `pathContains` matches a substring,
`pathMatches` matches a regular expression. The `query` subset requires every
declared pair among the request's percent-decoded query pairs, in any
position order. All filters compose by logical AND. Arrival-lane keys keep
their strict raw wire bytes.

Without a deadline one snapshot decides, for every bound. With a deadline the
poll re-reads at 100 ms intervals. `count` settles at equality and `atLeast`
at its floor. Arrivals only add, so early success is sound. `atMost` and a
range are absence claims over the window: they wait the full deadline, fail
on the first snapshot above the ceiling, and decide on the final snapshot.
Mismatch details name the bound in its own grammar (`expected at least 3`),
the filters by kind, and both counts. Filter payloads follow the redaction
law: recorded paths pass through the redactor, `pathContains` and
`pathMatches` render kind only, and secret query pairs render `<redacted>`.

### Diagnostic redaction uses the positive secret rule

Wire-path diagnostics redact only query values whose DECODED key is in the
secret set, per the ADR-0051 positive secret rule. The secret set is installed
through `PartnerRouter::set_secret_query_keys` (and the `HttpPartner`
adapter-level entry point). A percent-encoded secret key matches its decoded
form; the raw key span stays as authored, only the value is masked. Unknown
keys keep their authored bytes. When `raw_query_pairs` rejects the query (a
malformed key escape), the ENTIRE query portion is masked fail-safe — the
redactor never panics and never prints an undecodable secret.

### Empty harness path is an apparatus error

A harness HTTP target whose authored path is empty or absent fails parse as an
apparatus-class transport error naming the declaration — never a silent `/`
fallback lane. The check scans the authored target tail: it must start with
`/`.

### Document execution stops at the first failure

`run_scenario_document` executes actions in order through the shared
`run_action` primitive, records one outcome per executed action, and
stops at the first failure. `DocumentOutcome.verdict` is `Some(Pass)`
only when every action passed; `final_failure` is the post-verdict slot
the boot-owning caller fills after `BootHandle::shutdown` (a
`ShutdownFailure` there never masks the recorded verdict — exit path 2
at the CLI mapping). Validation mismatch details name the subject (the
variable, or the receiving endpoint), so a corrupted-header regression
is diagnosable from the failure text.

## Related decisions

- ADR-0069: integration-tier testing contract (format, vocabulary ban,
  provisioning sources, failure taxonomy, crate layout, shared
  composition root).
- [integration-tier spec](../../openspec/specs/integration-tier/spec.md):
  the scenario boot shares the `camel run` composition root (sealed
  config, security compile-context build, bind acknowledgements, bundle
  cascade, env-injected discovery, SQL startup checks), gates
  keycloak/oidc and wasm security offline, and pins the state prepare
  actions (`sql:`, `surreal:`), the surreal state assertion, and the
  scenario datasource teardown.
- ADR-0064: runtime-profile boundary that content-derived tiering
  measures.
- ADR-0049: `#[non_exhaustive]` posture for public enums.
- ADR-0055: publish-order constraints (no dependency on camel-test).

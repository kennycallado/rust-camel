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

**expectation**:
The matcher grammar of a `validate` action. Keys mirror the mock-testkit
matcher rules: `equals`, `regex`, `contains`, `startsWith`, `endsWith`,
`exists`, `jsonSubset`. A bare value is a literal `equals`; an object
with one recognized matcher key is that matcher; any other object is a
literal `equals`.
_Avoid_: assertion (assertions are the runner's verdict), matcher map

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
root — sealed config load (`from_file_sealed`, pinned profile, no
ambient `CAMEL_*` overrides), `configure_context_with_beans`,
`camel_bundles::boot`, route source load with `${env:}` resolution
through the `LayeredEnv`, `ctx.start()`. Returns `ScenarioRun { ctx,
boot }`; partners stay caller-owned.
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
_Avoid_: inbox, backlog

**selector**:
The dotted grammar of `receive.extract` reads: heads `body`, `headers`,
`status`, `method`, `path`. Header lookup is ASCII-case-insensitive
(hyper lowercases wire names; the fake preserves author casing). The
transport-scalar heads carry no sub-path.
_Avoid_: path expression, jsonpath

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
`InlineRoutes`. `UnsupportedProvisioning` names the
`infra-unavailable` class (ADR-0069 section 7). `RouteSourceMissing`
and `RouteSourceConflict` render the unit-tier messages verbatim,
without the token, and map to exit 2 as doc parse errors, exactly as
the unit tier maps them today. This crate never exits.

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
cascade) — the composition-root direction ADR-0069 section 10 fixes:
testing crates consume the boot, the engine never consumes the testing
crates.

### The scenario boot seals hermeticity twice

`boot_scenario` loads `<root>/Camel.toml` through
`CamelConfig::from_file_sealed`: the profile is pinned by value
(`&str`, defaulting to the document's `profile` or `"default"`), and
the `CAMEL_*` allowlist override merge is off. `${env:}` placeholders
in the config and in route files resolve through the `LayeredEnv`
(`interpolate_env_with`), never the process environment — the harness
must not read global state (ADR-0069 section 4). Route files load
through `camel_dsl::parse_yaml`, the same per-file parser under `camel
run` discovery, with the 16 MiB cap preserved.

### Partner receives resolve the wire role by dispatch state

`HttpPartner::receive` first checks for a response parked by its own
client-role `send`; the parked response wins (client role). Otherwise
the call awaits the next arrival queued on the endpoint's request path
(server role), bounded by the deadline. The v1 bound: server-role
arrivals queue per path; the client role keeps one response in flight
per endpoint URI. Arrivals map into `IncomingMessage` with the request
line (`method`, `path`) and `status: None` — requests carry no status;
status validation is inbound work.

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
  provisioning sources, failure taxonomy, crate layout).
- ADR-0064: runtime-profile boundary that content-derived tiering
  measures.
- ADR-0049: `#[non_exhaustive]` posture for public enums.
- ADR-0055: publish-order constraints (no dependency on camel-test).

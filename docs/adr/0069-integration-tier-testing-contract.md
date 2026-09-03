# ADR-0069: Integration-Tier Testing Contract

- Status: Accepted (human-ratified 2026-09-03; e_opus + e_gpt BLESS-WITH-FIXES, fixes applied)
- Date: 2026-09-03
- Supersedes: none. Binds the sketch in ADR-0064 section 4.
- Epic: rc-kk69. Authoring path: human grill + ste-writing (same path as ADR-0064, per rc-379d precedent). Not a conductor-light change.

## Context

ADR-0064 fixed the two-tier testing boundary. It named the integration tier as a
non-binding sketch: full runtime boot, real adapters, receive-with-timeout
assertions at transport boundaries. This ADR converts that sketch into binding
contract.

Three inputs shaped the decisions below:

1. Two expert consultations (e_opus, e_gpt) over the code, the ADRs, and the bd
   history. Both returned GO-WITH-CONDITIONS. All rulings were verified against
   source files.
2. A grill session with the human on 2026-09-03. Seven questions, all sealed.
3. Verified demand. The HTTP bridge header corruption (rc-eoft, rc-f0cn), the
   consumer readiness defect (rc-w1u9, since closed through Explicit startup
   and `mark_ready`), and the WS reconnect work (rc-cl7, rc-39d6) are real
   regressions that hand-written tests did not catch.

The pain is concrete. `crates/camel-test/tests/` holds roughly 30 hand-written
integration tests. They probe for free ports, sleep fixed intervals, and assert
by hand. The scenario runner replaces that pattern with declarative documents.

## Decision

### 1. One format, derived tier

All test documents are `.test.yaml`. There is no second schema and no
integration-specific suffix.

A document's tier is a pure function of its content. No field declares the
tier. A declaration field can only repeat what the machine computes, or
contradict it. The contradiction class is removed by not having the field.

The function is total through a conservative default:

```text
tier(document):
  1. The document has a `scenario:` section          -> FULL
  2. Else, compute the component closure over:
       the parsed RouteDefinitions from any route source
       (routeFiles, routeFilesFromRoot, or inline routes),
       with nested steps traversed recursively,
       MINUS endpoints replaced by `intercepts`,
       PLUS the schemes named in `inputs` and `expects`:
       closure within {direct, log, mock, seda, timer} -> LEAN
       any non-lean literal, placeholder-in-scheme,
       or dynamic dispatch step                        -> FULL
```

Dynamic dispatch steps are `recipient_list`, `routingSlip`,
`dynamic_router`, and `toD`-style targets computed from the exchange at run
time. Their target scheme is not knowable before run time, so they force FULL.

`scenario:` forces FULL without condition. A "lean scenario" would need an
action interpreter inside the lean boot. That is runtime-profile creep, which
ADR-0064 fences. Unit documents already have an action vocabulary:
`inputs`, `expects`, `intercepts`.

This conforms to ADR-0064 in spirit. That ADR fixes the boundary by inbound
stimulus plus runtime profile. It does not fix the boundary by file name.
Content-derived tiering measures the real stimulus. A label only claims it.

The lean registry stays byte-identical to today. Tiering routes documents to a
boot. It never grows the lean set. The creep rule and its amendment gate stay
in force without change.

### 2. Mixed vocabulary is forbidden in v1

A document with `scenario:` must not declare `inputs`, `expects`, or
`intercepts`. The runner rejects such a document at load time.

Reasons: one vocabulary per tier, and per document. `intercepts` exists to fake
transports in the lean boot. The full boot has the real transport. If a demand
for mixing emerges, reopen this rule with evidence.

The full boot also registers `mock`, `direct`, and `log`. The ban is for
clarity, not for lack of capacity.

`camel-lint` does not parse test documents today. When it does, it becomes the
static enforcement surface for this rule and for tier derivation. Until then,
the runner enforces at load time.

### 3. Filters, not modes

`camel test --unit` runs only lean documents. `camel test --integration` runs
only full-tier documents. Default `camel test` runs everything, each document
at its derived tier.

Tier filters are symmetric and exclude by scope:

- A nonmatching document found through directory expansion is excluded from
  the run.
- A nonmatching document named explicitly on the command line fails with
  `tier-filter-collision`. The explicit name is the assertion.
- Supplying both `--unit` and `--integration` is misuse, exit 2.

A CI job for the fast suite pins the fleet to lean through the filter, without
any per-document field.

A name or pattern filter may come later. It will compose with the tier filters
as one filter surface. A `--watch` mode is filed separately (rc-hi9y) and is
out of scope here.

### 4. Environment parity and hermeticity

Today `camel run` resolves `${env:NAME}` and `${env:NAME:-default}` in
Camel.toml and in route URIs. `camel test` resolves nothing. The full-boot test
path closes this gap by construction: both commands boot through the same
composition root, so the resolver behaves the same.

Hermeticity is primary. The test document is the source of truth, not the CI
machine.

- An optional `env:` section in the document fixes fixture values for the
  scenario. The resolver reads these first.
- Ambient environment inheritance is off by default.
- A document may list specific variables that pass through from the ambient
  environment.
- `CAMEL_PROFILE` is pinned per document. An ambient profile would break
  hermeticity.
- The document `env:` section does not couple to the `CAMEL_*` config-override
  allowlist. That allowlist governs TOML overrides. It is a different resolver
  from the `${env:}` route placeholder path.

The current resolvers read process-global state directly. The harness must
not mutate the process environment: concurrent documents, and a future
`parallel`, would race on it. Instead, the boot path receives an explicit
layered environment source: document `env` first, allowlisted ambient second,
defaults third, otherwise unresolved. The pinned profile is passed the same
way. This layered source is an input to the DSL and config loaders, not a
process-global rewrite.

Known limitation, recorded not solved here: `resolve_tree_walk` visits only
string leaves. Placeholders in int-typed TOML fields do not resolve. Ports in
URIs resolve today. The coercion enhancement is filed as rc-v1sw on its own
merits.

### 5. Partner-side assertions are the only normative proof

The harness owns a listener on the other side of the wire. For an outbound
route, the harness binds an HTTP server on `127.0.0.1:0` and validates what
the route producer sends. For an inbound route, a harness client drives the
real consumer and validates the wire response. What arrives there is the
proof: bytes, headers, status, timing.

Transport interception and `mock:` expectations are secondary diagnostics.
They never produce a green integration result. Mocks and interception are unit
tier tools by ADR-0064 design.

Loopback on `127.0.0.1:0` inside one process does not violate the no-IPC
invariant. The invariant forbids a test-control channel to a deployed
`camel run` process. Loopback traffic is the subject under test.

The readiness prerequisite is satisfied by rc-w1u9. `CamelContext::start()`
waits until the HTTP consumer binds or fails: the consumer opts into
`ConsumerStartupMode::Explicit` and calls `mark_ready()` after bind. The
current signal does not expose an address selected from port `0`. Inbound
scenarios therefore use an explicitly configured loopback port in v1.
OS-selected consumer ports require a separate operator-facing bound-address
API, filed on its own merits. The integration tier consumes only
operator-facing signals.

### 6. Core purity fences

`camel-core` is the engine. The testing program does not touch it.

1. No crate the testing program introduces may appear in `camel-core`
   `[dependencies]` or `[dev-dependencies]`. The ADR-0055 lint machinery
   enforces this.
2. No virtual clock in core. Integration deadlines use real monotonic time.
   Paused Tokio time stays a unit-harness concern.
3. No observability tap or test-only event surface in core. Readiness
   (rc-w1u9) is an operator signal designed on its own merits.
4. Tier derivation lives outside core. The function reads DSL and lint
   surfaces. Core has no concept of tiers.
5. Partner readiness polling wants a "consumer bound" event. That want is the
   rc-w1u9 temptation. The fix stays an operator signal, not a test callback.
   The harness consumes the readiness signal only through the same public
   surface an operator uses: health or readiness state, or the bound address
   a future boot handle reports. It never subscribes to a core-internal
   event or a callback added for the test. If partner readiness needs more
   than the operator surface exposes, the gap is an operator-facing engine
   feature, filed on its own merits. It is not a harness hook into core.
6. Bugs the tier exposes are engine bugs. They are filed and fixed in the
   domain on their own merits.

Any new core API proposed for testability must answer one question: would this
API exist without tests? If not, it does not land.

### 7. Failure taxonomy

Exit codes are inherited from `camel test` today: 0 all pass, 1 any failure,
2 any parse or misuse error. The split is epistemic:

Exit 1, the scenario ran and the system under test failed it:

- `receive-timeout`: nothing reached the partner before the deadline.
- `validation-mismatch`: the message arrived and failed the validator.
- `scenario-var-unresolved`: a referenced variable was never set. When the
  defect is statically detectable at load, it is exit 2 instead.

Exit 2, the scenario never got a meaningful answer:

- `doc-validation`: mixed vocabulary, broken grammar.
- `tier-filter-collision`: an explicitly named document did not match the
  tier filter.
- `partner-bind-failure`: the harness could not bind its listener.
- `partner-startup-failure`: the partner listener bound but its handler
  failed to start.
- `action-transport-failure`: a send or receive action failed at the
  transport before any assertion ran.
- `infra-unavailable`: an adapter needs a broker or Docker that is absent.
  The error names the requirement. It never hangs.
- `full-boot-failure`: the embedded boot failed.
- `shutdown-failure`: teardown of the boot or a partner timed out or erred
  after the verdict was recorded.

Every adapter operation carries a deadline, not only `infra-unavailable`. A
cancelled `parallel` sibling reports as cancelled, not as a verdict failure.

`infra-unavailable` fails loud by default. With demand-gated adoption, a
silent skip would hide the demand signal itself. A skip mechanism, if ever
needed, is a separate decision.

### 8. Demand gate and activation order

The tier activates adapters only when a concrete regression justifies them.

1. HTTP, both directions. Outbound bridge and proxy regressions justify it
   (rc-eoft, rc-f0cn). The rc-w1u9 readiness work is already satisfied.
2. WS, after the consumer-client role lands (rc-39d6).
3. gRPC is a loopback candidate. It needs no Docker. It activates on demand.
4. Kafka, JMS, and other broker adapters wait for an adapter-specific
   regression.

Each adapter is a Cargo feature. There is no all-components feature. CI runs a
dedicated `integration-http` job with path filters. Broker scenarios run in an
isolated or scheduled job. Loopback tests carry no `#[ignore]` marker. That is
the ADR-0054 rule, not a new one. The loopback budget is seconds.

### 9. Partner provisioning sources

The grammar names three sources for a partner endpoint address. The axis is
who owns the lifecycle.

1. `harness`: the harness binds an in-process listener on `127.0.0.1:0`. This
   is the only source implemented in v1.
2. `testcontainer`: the harness manages an ephemeral container and destroys it
   with the scenario. Reserved grammar value. The v1 runner rejects it as
   unsupported.
3. `user-provided`: the document receives an address through a variable or
   passthrough environment value. How that infrastructure exists is not the
   harness's concern. Docker Compose, a CI service container, or a staging
   broker are all the same to the grammar. Reserved grammar value in v1. The
   runner rejects it as unsupported until an adapter activation needs it.

The system under test is always the embedded boot. The harness never drives a
deployed `camel run`. A live `camel run --watch` next to a test run is two
independent processes. They share nothing. Port conflicts are the only
interaction, and section 4 covers them: URI ports resolve through `${env:}`.

### 10. Crates

Two new crates, both depending on core, never the reverse.

`camel-bundles` owns the component-bundle registration cascade extracted from
`camel run`, plus the lifecycle handle. The name is the project's own noun:
the crate's sole responsibility is running `ComponentBundle::register_all`.
`camel-config` keeps context composition. The extraction does not re-home it.

The shared boot boundary is enumerated, not blanket. `camel-bundles` owns the
bundle registration and the lifecycle handle with explicit `shutdown()`. The
handle owns the bridge cleanup and the JMS and CXF pool teardown. The CLI
keeps the watcher, signal handling, the second-Ctrl+C path, the conditional
exec guard, and operator logging. Security setup, bind acknowledgements,
datasources, and startup checks move to the shared boot when, and only when,
both consumers need the same semantics. Until then each path states what it
owns. The handle type is `BootHandle`, following the `...Handle` suffix
precedent.

`camel run` and the harness both register bundles through this one cascade.
Feature flags for bundles forward from the CLI and from the harness into
`camel-bundles`.

`camel-integration-test` owns the scenario model, the parser, the action
executor, validators, partner adapters, and the Rust API. `camel-cli` depends
on it and provides a thin command adapter. `camel-test` stays unchanged: it is
the publish-order leaf sink, and ADR-0055 forbids publishable dependencies on
it.

### 11. Citrus divergences

Citrus is inspiration, not authority. This is the ADR-0046 rule applied to
Citrus.

Adopt: ordered action lists. Logical endpoints bound to typed transport
drivers. `send`, `receive` with a mandatory deadline, `sleep`, `validate`.
Scenario variables with extraction. Exact body, header, and status
validators.

Defer: `iterate`. Validator registries. A negative expect-timeout action.
Protocol-specific action families. Structured `parallel` with sibling
cancellation on failure. The cancellation semantics are sealed. The timing is
deferred until a scenario demands it.

Reject, binding:

- Citrus conformance in files, API, or literal semantics. Assertion
  translation is re-derivation, not porting. This is the ADR-0046
  anti-pattern.
- XML, Spring Bean, JUnit, and TestNG formats and runner coupling. This
  runtime is embedded Rust.
- Standalone mode against a deployed `camel run`. The frozen no-IPC
  invariant stands.
- Universal symmetric client-server endpoints. Each adapter declares the
  roles it supports.
- `repeat-on-error`. It masks non-determinism and subverts the conditional
  determinism of ADR-0054.

### 12. bd hygiene

rc-i2qf closes with a reason, not a supersede. Its acceptance criterion is
already satisfied by the recorded rejection: a producer is a write/send sink.
Partner reply behavior belongs to the typed partner adapters in
`camel-integration-test`. `camel-component-mock` does not change.

## Consequences

### Positive

- One test document format across the route lifecycle.
- The tier boundary stays machine-checked with no label field to drift.
- Engine defects get an honest detector without engine pollution.
- The env gap between `camel run` and `camel test` closes by construction.

### Negative

- The tier function must stay correct as the DSL grows. New dynamic-dispatch
  steps must register as FULL-forcing.
- Content-derived tier removes the file name as tier metadata. CI selects
  through the tier filters, not a glob. The runner's tier report records each
  document's tier for audit.
- Two new crates raise the publish surface.

## Alternatives considered

- A separate `*.integration.test.yaml` schema. Rejected in grill. Citrus
  separates because it is an external framework against a deployed system.
  The harness here is first-party and embedded. The separate schema also
  duplicated `routeFiles` references across two files per route.
- A declared `tier:` field. Rejected. Redundant or contradictory, never
  informative. The filter flag carries the assertion role.
- Growing the lean boot with more components. Rejected by ADR-0064 creep
  rule. Unchanged here.
- Interception or mock expectations as integration proof. Rejected. rc-w1u9
  shows the in-process view lies about readiness. Only the wire is honest.

## Self-grill record

- Grill session 2026-09-03, seven questions, all sealed by the human.
- Expert consultations: e_gpt (first round, 8 rulings), e_opus (verdict and
  P1-P4 adjudication, both code-verified). e_gpt second round did not respond;
  e_opus arbitration covered the naming deadlock.
- Divergence labels for ADR-0046 bookkeeping: unified format vs Citrus file
  separation (`divergence`), filter flags vs mode flags (`divergence`),
  repeat-on-error reject (`divergence`), scenario-implies-FULL (`pin-invariant`).

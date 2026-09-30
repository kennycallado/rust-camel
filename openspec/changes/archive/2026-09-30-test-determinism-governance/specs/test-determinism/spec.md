## ADDED Requirements

### Requirement: No sleep-as-sync in test bodies

A test SHALL NOT use a fixed sleep as a synchronization point. When a
test needs state that another task produces asynchronously, it SHALL
wait on an observable state (a `wait_until`-style barrier with a
deadline) instead of assuming the state after N milliseconds. Post-start
sleeps are vestigial since the rc-w1u9 explicit startup handshake. The
flake class governed here is `sleep-as-sync`. Advisory detection of
sleep calls in test bodies is the contract of the `lint-test-sleep`
capability; this requirement is the design rule that detection serves.

#### Scenario: Readiness waits on observable state

- **GIVEN** a test that starts a consumer or server asynchronously and
  needs it ready before asserting
- **WHEN** the test waits for readiness
- **THEN** it polls or awaits an observable readiness signal under a
  deadline, and no fixed-duration sleep stands between start and
  assertion

#### Scenario: Fixed sleep cannot carry an assertion

- **GIVEN** a test body that contains `sleep(...)` (blocking or async)
  followed by an assertion on asynchronously produced state
- **WHEN** the sleep is the only mechanism that orders the assertion
  after the state change
- **THEN** the test violates this requirement, whatever delay value it
  uses, because the assertion rests on timing rather than observation

#### Scenario: Explicit handshake removes post-start sleeps

- **GIVEN** a component that implements the rc-w1u9 explicit startup
  handshake
- **WHEN** a test starts that component
- **THEN** the test synchronizes on the handshake signal and needs no
  post-start sleep

### Requirement: No free-port probing for test addresses

Test infrastructure SHALL NOT select a network address through
bind, inspect, close, and rebind (the `port-toctou` flake class). The
component that owns the bind SHALL accept port zero, retain the live
listener, and report its bound address through a production or
operator-facing API. A child process MAY bind port zero and report the
address to its parent. A reservation socket is not an ownership
handoff. The staged-listener contract that implements this rule is the
`staged-listener-binding` capability (ADR-0070). Named exceptions carry
their own bd issues (rc-1dgvg in-lib residue, rc-s7dyw
external-process handoff) plus ADR-0070's reserved-address and
oneshot-placeholder exceptions.

#### Scenario: Staged listener replaces the free-port probe

- **GIVEN** an inbound component under test that needs a loopback
  address
- **WHEN** the test provisions that address
- **THEN** the component binds `{host}:0`, the live listener is staged
  through the ADR-0070 surface, and the test reads the bound address
  from that surface — the port is never read from a socket that is then
  closed and rebound

#### Scenario: Child process binds and reports

- **GIVEN** a test that exercises an external process which owns its
  listener
- **WHEN** the process starts
- **THEN** it may bind port zero and report the bound address to its
  parent through an explicit handoff, and the parent uses the reported
  address directly

#### Scenario: Exceptions are named and tracked

- **GIVEN** a call site that cannot yet stage its listener (in-lib
  residue or external-process handoff)
- **WHEN** it remains on a legacy pattern
- **THEN** the exception is one of the ADR-0070 named exceptions and an
  open bd issue tracks its conversion

### Requirement: Real loopback servers for outbound HTTP client tests

A test of outbound HTTP client behavior SHALL use a real loopback
server that implements the connection semantics the scenario requires
(prefer axum, Hyper, or wiremock; `oneshot` only for server-handler
behavior below the network boundary). Raw TCP is allowed only for
malformed-message, framing, disconnect, and other protocol-fault
tests. A scenario that exercises connection reuse (redirects,
keep-alive, pooling) SHALL drive a server with a complete request loop
for persistent connections. One-response-and-close semantics are
permitted only in single-transaction scenarios where no reuse can
occur. This closes the `pooled-race` flake class: a pooled client never
races a server-side FIN on a connection it is about to reuse. Consumer
readiness in the shared test harness — registry-poll readiness with a
bounded budget and registry-mutation serialization — is the contract
of the `http-test-harness` capability; the loopback-server obligation
above is this capability's rule.

#### Scenario: Pooled reuse is served by a complete request loop

- **GIVEN** an HTTP client under test that pools connections and a
  scenario that issues a second request over the same connection (for
  example after a redirect)
- **WHEN** the pooled client reuses the connection
- **THEN** the loopback server runs a complete request loop and serves
  the second request on that connection, so the reuse never meets a
  server-side close

#### Scenario: One-response-and-close stays single-transaction

- **GIVEN** a raw-TCP or minimal server that closes after its first
  response
- **WHEN** a test uses it
- **THEN** the scenario issues exactly one request per connection and
  asserts no connection reuse, so the close cannot race a second
  request

#### Scenario: Raw TCP is reserved for protocol faults

- **GIVEN** a test that needs malformed messages, framing errors, or
  intentional mid-connection disconnects
- **WHEN** the test builds its server
- **THEN** raw TCP is permitted as a protocol-fault scenario, and the
  test states the fault it produces explicitly

#### Scenario: Readiness comes from the registry, not a probe

- **GIVEN** a staged listener and a spawned consumer in the shared
  harness
- **WHEN** the test waits for readiness
- **THEN** readiness follows the `http-test-harness` capability's
  registry-poll readiness contract within its bounded budget, rather
  than any TCP probe sent by the test

### Requirement: Guarded process-environment mutation in tests

A test SHALL inject configuration directly when the API permits it. A
test OF process-environment behavior SHALL run in a dedicated child
process with an explicit environment. In-process mutation of the
process environment is a documented legacy exception: it SHALL use one
crate-wide RAII guard (the EnvGuard pattern), one lock, and SHALL
restore the prior value on drop. Async or multi-threaded mutation is
forbidden unless all readers and writers are proven to use that same
lock (the `global-state` flake class). The future enforcing lint
recognizes the canonical guard type, not variable names.

#### Scenario: Direct injection wins

- **GIVEN** a component API that accepts configuration values as
  parameters or builders
- **WHEN** a test needs those values set
- **THEN** the test injects them through that API and does not touch
  the process environment at all

#### Scenario: Environment-behavior tests run out of process

- **GIVEN** a test whose subject is how code reads the process
  environment (variable presence, precedence, escaping)
- **WHEN** the test sets those variables
- **THEN** it runs in a dedicated child process with an explicit
  environment, so no other test in the binary observes the mutation

#### Scenario: Legacy in-process mutation restores prior value

- **GIVEN** an in-process test that must mutate an environment variable
  under the documented legacy exception
- **WHEN** the mutation begins
- **THEN** the canonical crate-wide RAII guard holds one lock, sets the
  value, and restores the prior value on drop, and no unsynchronized
  concurrent reader or writer of that variable exists in the same
  process

### Requirement: Runner pollution hygiene in test execution

A test SHALL NOT leak child processes, bound listeners, or firewall
state beyond its own execution (the `runner-pollution` flake class).
Teardown SHALL be bounded and SHALL reap or abort everything the test
spawned; a test that spawns long-lived service loops bounds every
readiness assertion and bounds teardown. Test-suite invocations on
shared runners run under resource scopes so orphaned processes are
collected when a run ends, and an orphan count of zero after a run is
the pass condition.

#### Scenario: Teardown reaps what the test spawned

- **GIVEN** a test that spawns child processes or service tasks
- **WHEN** the test finishes (pass or fail)
- **THEN** teardown joins or aborts every spawn under a deadline and no
  process from the test outlives it

#### Scenario: Scope collection catches orphans

- **GIVEN** a hung or killed test run inside a resource scope
- **WHEN** the scope ends
- **THEN** the scope's collector terminates the run's remaining
  processes, so later runs on the same runner start clean

#### Scenario: Zero-orphan pass condition

- **GIVEN** a full test battery on a shared runner
- **WHEN** the battery completes
- **THEN** an orphan-process count of zero for the battery's known
  process names is required to call the run clean

### Requirement: Quarantine registry for confirmed flaky tests

Normal CI MAY retry a test for diagnosis only, and a pass-on-retry
KEEPS the gating job red (`flaky-result = "fail"` semantics). A
confirmed flaky test MAY enter a checked-in quarantine registry. Each
registry entry SHALL name the exact test ID, a bd issue, an owner, and
an ISO expiry date. A gating xtask lint SHALL reject missing,
malformed, or expired entries. A separate non-gating job MAY run
quarantined tests with retries. Maximum quarantine lifetime is 14
days. `#[ignore]` and test-name suffixes are not quarantine
mechanisms; the `ignore-test-policy` closed vocabulary (ADR-0054)
governs `#[ignore]` and is unchanged by this requirement.

#### Scenario: Pass-on-retry stays red

- **GIVEN** a gating job where a test fails and then passes on retry
- **WHEN** the job reports its verdict
- **THEN** the job is red, because a retry-pass does not clear a gating
  failure

#### Scenario: Registry entry carries the full contract

- **GIVEN** a confirmed flaky test accepted into quarantine
- **WHEN** its registry entry is written
- **THEN** the entry names the exact test ID, an open bd issue, an
  owner, and an ISO expiry date at most 14 days ahead

#### Scenario: Gating lint rejects bad entries

- **GIVEN** a quarantine registry containing an entry that is missing a
  field, malformed, or past its expiry date
- **WHEN** the gating lint runs
- **THEN** it fails the gate and names the offending entry

#### Scenario: Quarantined tests keep running off-gate

- **GIVEN** tests with live quarantine entries
- **WHEN** CI runs the separate non-gating job
- **THEN** the quarantined tests execute with retries so their health
  stays observable without blocking the gate

#### Scenario: Ignore is not quarantine

- **GIVEN** a flaky test someone marks `#[ignore]` or renames with a
  suffix instead of registering it
- **WHEN** that change is reviewed against this requirement
- **THEN** it does not satisfy quarantine, because only a registry
  entry with the full field contract and expiry constitutes quarantine

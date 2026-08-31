# benchmark-suite delta — bench-node

## ADDED Requirements

### Requirement: Contender completeness

The suite MUST enforce contender completeness for completeness-declaring
families: a family that declares completeness SHALL implement every
ACTIVE scenario selected for the run. Enforcement is selection-scoped —
a family with at least one fixture among the run's selected active
scenarios that does not cover ALL selected active scenarios is a hard
error listing the family and each missing scenario. Families that do not
declare completeness are outside the rule (today: the YAML artifact-set
pair excluded from bridge scenarios by the documented scenario-side
`SCENARIO_ARTIFACT_SET` reduction). The node family declares
completeness across all 7 active scenarios. Inactive scenarios (no
harness wiring, no contender set — currently `multi-step`) are outside
the rule until activated; a fixture present for an inactive scenario
triggers a warning, not an error.

#### Scenario: contender family registered with a missing fixture

- **Given** the node contender family (completeness-declaring) and a run
  selecting all 7 active scenarios
- **When** the harness wires cells and `node-fastify` lacks a fixture
  directory for `split-aggregate`
- **Then** the run aborts before any measurement with an error listing
  `node-fastify` and every scenario it is missing (here
  `split-aggregate`)

#### Scenario: fixture for an inactive scenario

- **Given** a fixture directory `scenarios/multi-step/node-native/`
- **When** the harness wires cells
- **Then** the run emits a warning naming `multi-step` as inactive and
  continues without registering a cell for it

### Requirement: Node contender family

The suite SHALL measure two Node.js contenders, `node-native` (no web
framework; external libraries only where the Node stdlib has no
equivalent capability, i.e. the XML scenarios) and `node-fastify` (same
route logic behind Fastify), across all active scenarios. Both honor the
scenario's existing contract: shared canonical payload, marker line,
latency file, and per-scenario protocol identical to the JVM contenders
in the same scenario. In the six protocol-B scenarios `node-fastify`
boots the Fastify application WITHOUT binding a listener — the module
and init cost is the measured framework tax; only `http-server` binds a
listener. Node XML fixtures execute their libraries in-process
(`saxon-js`, `xmllint-wasm`), reuse only the scenario's shared
`bench-payload`/`.xsl`/`.xsd` assets (digest parity by construction),
and are exempt from the compiled-bridge subprocess/wrapper/PID
contract.

#### Scenario: cell registration

- **Given** a host with the pinned Node runtime resolved
- **When** the harness wires a scenario that has `node-native` and
  `node-fastify` fixtures
- **Then** two cells register with launch commands invoking `node` on the
  fixture entry scripts, the scenario's marker contract, and the
  scenario's protocol mapping

#### Scenario: digest parity

- **Given** the smoke harness for a scenario with a canonical payload
- **When** the node contenders' smoke runs
- **Then** their input digests equal the digests the existing contenders
  produced for the same scenario (byte-identical canonical input)

#### Scenario: dry-run without Node on the host

- **Given** a host without `node` on PATH and `--dry-run`
- **When** the node cells resolve
- **Then** fixtures without `package.json` resolve from their committed
  scripts (no build) and fixtures with `package.json` report a
  would-build marker without failing the dry-run

### Requirement: Pinned Node runtime

The runner image SHALL install Node from the official `nodejs.org/dist`
tarball with its SHA256 pinned by `pin.sh` alongside the existing runner-image
digest record; the runner refuses mutable tags and unpinned versions.

#### Scenario: pin verification

- **Given** a runner image build with `NODE_VERSION` and `NODE_SHA256`
  recorded by `pin.sh`
- **When** the build downloads the tarball
- **Then** the build verifies the SHA256 before install and fails closed
  on mismatch

#### Scenario: XML engine auditability

- **Given** the `xsd-validation-bridge` and `xslt-bridge` node fixtures
  (which require external libraries because Node stdlib has no XML)
- **When** a reviewer reads the fixture
- **Then** the fixture README names the library and engine in use
  (`saxon-js`, `xmllint-wasm`) and the counterpart engine on the JVM
  side, so the cross-engine comparison is auditable

## MODIFIED Requirements

### Requirement: Payload-size axis

The benchmark harness SHALL support selecting a transport-body size class
(1024, 32768, 262144, or 1048576 bytes — other values rejected) for
load-driven measurements (Protocol A and M3 throughput), and timer-driven
fixtures (Protocol B) SHALL honor a `BENCH_PAYLOAD_BYTES` environment
variable constructing a byte-identical canonical body in every artifact.
Fixture-count references below mean the completeness-declaring fixture
set of the scenario (registered contenders; eight with the node family).

#### Scenario: Transport body builder exactness

- **GIVEN** the loadgen body builder unit test
- **WHEN** bodies are built for 1024, 32768, 262144, 1048576 bytes
- **THEN** each body's length is exactly the requested size and its
  SHA-256 matches the recorded golden digest for that size

#### Scenario: Invalid size rejected

- **GIVEN** `measure-throughput --payload-size 2048`
- **WHEN** the CLI parses arguments
- **THEN** it exits with a usage error naming the four valid sizes

#### Scenario: Protocol-B fixture canonical body and size assert

- **GIVEN** a t2-json fixture started with `BENCH_PAYLOAD_BYTES=32768`
  and tick 0
- **WHEN** the route builds its body
- **THEN** the fixture logs `BENCH_INPUT_SHA256=<golden hex for
  (32768, tick 0)>` and asserts the body length equals 32768 before
  processing and the exact output length equals 32768 + 13 bytes after
  marshaling, emitting NO marker (failing the cell) on any mismatch

#### Scenario: Artifacts byte-equivalent

- **GIVEN** the same `BENCH_PAYLOAD_BYTES` and tick passed to the
  completeness-declaring fixture set of a scenario
- **WHEN** each fixture builds its body
- **THEN** every fixture produces byte-identical canonical inputs (golden
  digest equality), so rankings measure the framework, not payload skew

### Requirement: t2-json scenario

The suite SHALL provide a `t2-json` scenario exercising
unmarshal("json") → jsonpath validate → transform (append field) →
marshal("json") with the scenario's registered contender set (six
artifact fixtures, eight cells with the node family), registered in the
harness marker and protocol maps, marker-emitting exactly once per suite
contract.

#### Scenario: rust-lib fixture passes marker contract

- **GIVEN** the harness runs the `t2-json` rust-camel-lib cell
- **WHEN** the route completes one cycle
- **THEN** stdout contains exactly one `BENCH_ROUTE_READY` marker, the
  marshaled output has the exact asserted length (input_size + 13
  bytes), and parsed semantic equality holds (id="bench", original seq,
  fill, appended `"bench": true` present)

#### Scenario: Cross-runtime input equivalence

- **GIVEN** the completeness-declaring fixture set (six artifact
  fixtures, eight cells with the node family) runs with the same payload
  class and tick
- **WHEN** each logs its input digest
- **THEN** all report the same `BENCH_INPUT_SHA256` value for that
  (size, tick); output bytes may differ in field order (documented
  caveat), inputs never do

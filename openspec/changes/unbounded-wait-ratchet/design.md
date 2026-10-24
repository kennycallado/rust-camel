# Design: unbounded-wait-ratchet

## Approach

Full inventory first (done, committed in `classify/`): 299 sites across 98 files, classified
before any edit. (Earlier draft said 44/255; blessing and plan-bless audits reclassified 10 sites whose inner HTTP/TCP reads carry no enforced deadline — 6 in partner_verification_test.rs via `raw_post`, 3 in deadline-free tonic/TCP call paths: cxf `health()` + `invoke()`, partner_validate `raw_request`, camel-bridge process.rs child wait (pipe backpressure).) **Bucket A (fix) = 54, Bucket B (marker) = 245, Bucket C (production) = 0.**
Every flagged site is test-scoped; the six spec-named
global serialization-lock families
(`REGISTRY_TEST_LOCK`, `SERVER_MUTEX`, `TEST_MUTEX`, `SENTINEL_TOPOLOGY_LOCK`, `TOPOLOGY_LOCK`,
`ACK_TEST_LOCK`) appear nowhere in the inventory, so no `acquire_deadline` conversions and no
marker-ban conflicts exist.

**Helper.** One new shared helper beside `acquire_deadline` in
`camel_component_api::test_support` (feature `test-support`):

`#[track_caller] pub fn timeout_wait<T>(what: &str, deadline: Duration, fut: impl Future<Output = T>) -> impl Future<Output = T>`

Internally `tokio::time::timeout(deadline, fut)`; on `Elapsed` it panics naming `what`, the
deadline, and `Location::caller()` — preserving the existing spec guarantee that a converted
wait's failure names the wait and the deadline; the inner output passes through unchanged so
channel-closed stays distinct at the call site (spec test-determinism: fail loudly, never
silently skip). Placement evidence:
camel-core, camel-test and the components already dep `camel-component-api` with
`test-support`; ADR-0055/publish-topology forbids publishable crates from dev-depping
`camel-test`, which rules that crate out as the shared home.

**Why no lint change.** The lint flags wait-class awaited expressions (`recv`, `lock`,
`connect`, …). A site rewritten as `timeout_wait(what, d, rx.recv()).await` awaits a
non-wait-class
helper future; the receive is a bare future argument. Findings die mechanically — the lint's
recognized-timeout set, scanning rules, and `scripts/xtask/src/lint_unbounded_wait.rs` stay
untouched (spec scenario "Lint sources and ratchet stay untouched" is respected verbatim).
Phase 1 verifies this on a sample before mass migration.

**Markers.** Bucket B sites get `// allow-test-wait: <reason>` on a line spanned by the
finding, reason citing the construction (test-owned uncontended mutex, test-fed channel,
join of finite harness task). Existing same-file precedents (jsonpath/xpath/otel suites) match
this pattern. Marker legitimacy documented in one header comment line in the ratchet file
(comments are ignored by the parser per lint source).

**Ratchet.** After the burn the unadjudicated count is 0; ceiling drops 296 → 0 (monotone,
never raised). Spec scenarios carrying historical arithmetic (431, 296-minus-N) are superseded
by the delta spec's zero end state. The spec's "named ceiling entry" escape hatch is not used.

## Affected crates

- `camel-component-api`: `timeout_wait` helper + unit tests (panic path, output passthrough;
  no `tokio::time::sleep` — lint-test-sleep clean).
- Bucket A edits: camel-component-grpc, camel-cxf, camel-jms, camel-validator, camel-xslt,
  camel-rabbitmq, camel-sql, camel-http, camel-stream, camel-test, camel-integration-test,
  camel-cli, camel-core (redb journal join), camel-component-llm/mcp/wasm as per TSVs.
- Bucket B markers: same files plus harness/mock-mutex sites across camel-test suites and
  component tests.

## Architecture boundaries

Runtime/Components boundary untouched — no production code paths change (bucket C is empty).
The helper lives in the existing test-support surface next to `acquire_deadline`; no new
crate, no dependency edges added. camel-test suites keep their existing `wait_until` poll
helper for condition polling; `timeout_wait` bounds one-shot waits — the two compose, neither
duplicates the other. Component publish topology unchanged (lint-publish-cycles gate guards).

## Phases

### Phase 1: Shared helper + marker documentation
- **Goal:** `timeout_wait` shipped with tests; ratchet header line added; helper-migration
  mechanics verified against the real lint.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** `camel_component_api::test_support::timeout_wait`.
- **Deliverable:** helper + unit tests + doc line + lint mechanic verification note in commit.
- **Exit-criteria:** `cargo test -p camel-component-api --lib` passes; lint run on a
  helper-wrapped sample site shows the finding removed; marker sample shows suppression.

### Phase 2: Bucket A — bound the 54 unbounded sites
- **Goal:** every genuinely-unbounded wait bounded via `timeout_wait` (or direct
  `tokio::time::timeout` where `Elapsed` distinction matters).
- **Dependencies:** Phase 1 helper.
- **Externally-visible types/interfaces:** none new.
- **Deliverable:** per-cluster commits matching `classify/*.tsv` A rows.
- **Exit-criteria:** lint findings drop by the A count; touched crates' tests pass; failures
  stay loud (no swallowed `Elapsed`).

### Phase 3: Bucket B — adjudicate the 245 bounded-by-construction sites
- **Goal:** marker + construction reason on every B row, placed on a spanned line.
- **Dependencies:** Phases 1–2 complete (every A-site burned BEFORE any marker lands, so markers never hide an unbounded site).
- **Externally-visible types/interfaces:** none.
- **Deliverable:** per-cluster commits matching `classify/*.tsv` B rows.
- **Exit-criteria:** lint findings 0 on the full tree with ceiling still at 296.

### Phase 4: Ratchet lock-in + verification
- **Goal:** ceiling 0, gate green, evidence table final.
- **Dependencies:** Phases 2–3 complete (findings 0).
- **Externally-visible types/interfaces:** none.
- **Deliverable:** ceiling file at final count + header doc line; classification table.
- **Exit-criteria:** `cargo xtask lint-unbounded-wait` exit 0 at ceiling 0; touched-crate
  suites + `cargo test --workspace --lib` green.

## Alternatives considered

- **Teach the lint the helper name**: rejected — lint source is frozen by spec scenario
  ("Lint sources and ratchet stay untouched"), and it is unnecessary given the non-wait-class
  await shape.
- **Helper in camel-test**: rejected — ADR-0055 publish topology forbids publishable crates
  from depending on camel-test in any dependency kind.
- **Direct `tokio::time::timeout` everywhere, no helper**: rejected — 54 sites of per-site
  boilerplate; the mission mandates one shared bounded helper, matching the `acquire_deadline`
  precedent.
- **Leave ceiling at 296**: rejected — gate stays red; the spec's own trajectory is monotone
  decrease to full adjudication ("no silent sites").

## ADDED Requirements

### Requirement: Shared bounded test-wait helper timeout_wait

`camel_component_api::test_support` SHALL provide a single shared bounded-wait helper
`timeout_wait(what, deadline, fut)` that drives `fut` under `tokio::time::timeout`, and every
crate that needs to bound a one-shot test wait SHALL prefer it over per-site
`tokio::time::timeout` boilerplate; no parallel bounded-wait helper is introduced in any
crate. On deadline elapse the helper SHALL panic naming `what` (the wait under test), the
deadline, and the call site (`#[track_caller]`) — preserving the established guarantee that
a converted wait's failure names the wait and the deadline — and the inner future's output
SHALL pass through unchanged so a channel-closed outcome remains distinct from a hang at the
call site. Adoption of the helper SHALL NOT require changes to
`scripts/xtask/src/lint_unbounded_wait.rs`: a wait rewritten as
`timeout_wait(what, d, expr).await` leaves the lint's wait-class scan because the awaited
expression is the helper call, not a wait-class call.

#### Scenario: Hung wait fails the test loudly

- **GIVEN** a test body containing
  `timeout_wait("orders receive", Duration::from_secs(30), rx.recv()).await`
  where no value is ever sent
- **WHEN** the deadline elapses
- **THEN** the test panics with a message naming `what`, the deadline, and the
  `timeout_wait` call site, instead of parking indefinitely

#### Scenario: Channel closure stays distinct from a hang

- **GIVEN** a test whose bounded receive returns `None` because the sender was dropped
- **WHEN** the test matches on the received value
- **THEN** the `None` is observed as the inner output and asserted like any other value,
  not conflated with the deadline path

#### Scenario: One helper, no per-crate reimplementation

- **WHEN** any crate needs to bound a one-shot test wait
- **THEN** it uses `camel_component_api::test_support::timeout_wait`; a second shared
  bounded-wait helper anywhere in the workspace is a spec violation

#### Scenario: Helper adoption needs no lint modification

- **GIVEN** a flagged unbounded receive rewritten to
  `timeout_wait("orders receive", d, rx.recv()).await` with no other edit
- **WHEN** `cargo xtask lint-unbounded-wait` scans the file
- **THEN** the finding is gone and `scripts/xtask/src/lint_unbounded_wait.rs` is unmodified

### Requirement: Ratchet zero end-state after full adjudication

After every site in the mission-394 inventory (299 findings on main 3dfada1a7) is either
bounded in-tree or marker-adjudicated with a construction-citing reason, the ratchet ceiling
in `scripts/xtask/ratchet-unbounded-wait.max` SHALL be lowered to 0 and
`cargo xtask lint-unbounded-wait` SHALL report 0 findings and exit 0 at that ceiling. Zero
means: no unadjudicated DETECTED sites — waits outside the lint's documented scan scope are
unchanged by this requirement and remain governed by the existing scan-scope rules. The
ceiling SHALL move monotonically down from 296; it SHALL NOT be raised. The ceiling file
SHALL carry a header comment documenting marker legitimacy: a marker is legitimate only
where the wait is bounded by construction (test-owned uncontended lock, test-fed channel,
join of a task whose every progress dependency is bounded or purely in-memory), never to
mask a wait that could hang.

#### Scenario: Ceiling records the burned count and the gate is green

- **GIVEN** all inventoried sites are bounded or marker-adjudicated
- **WHEN** the ceiling file is set to 0 and `cargo xtask lint-unbounded-wait` runs
- **THEN** the lint reports 0 findings and exits 0 at that ceiling

#### Scenario: A new unbounded test wait fails the gate

- **GIVEN** the ceiling is at the burned count
- **WHEN** a change adds a test wait that is neither bounded nor marker-adjudicated
- **THEN** the lint reports the site and fails, and it passes only after the site is
  bounded or adjudicated

#### Scenario: Marker documentation lives in the ceiling header

- **GIVEN** the ceiling file's header comment
- **WHEN** a contributor considers adjudicating a site
- **THEN** the header states the construction-based legitimacy rule for
  `// allow-test-wait:` markers

## MODIFIED Requirements

### Requirement: Deadline-bounded channel receives in test bodies

Every `.recv().await` on an asynchronous channel inside a `#[test]` / `#[tokio::test]`
function body (including receives inside `async move {}` blocks passed to `tokio::spawn`,
which are in test-body AST scope) SHALL be deadline-bounded at the call site — via
`tokio::time::timeout` / `timeout_at` future-argument containment, a per-iteration deadline
inside a loop, or the shared `camel_component_api::test_support::timeout_wait` helper — or
carry an `// allow-test-wait:` marker with a site-specific justification. Once the
mission-394 inventory is fully adjudicated, the ratchet ceiling SHALL equal the
unadjudicated finding count per the ratchet zero end-state requirement; the historical
arithmetic of prior partial burns (548 − 117 = 431) is superseded.

#### Scenario: Single expected receive is timeout-wrapped

- **GIVEN** a test that awaits `rx.recv().await` once and asserts on the value
- **WHEN** the site is converted
- **THEN** the receive is bounded (as the future argument of
  `tokio::time::timeout(deadline, rx.recv())` or via `timeout_wait`), the `Elapsed`
  case fails the test with a message naming the wait and the deadline, and the
  channel-closed case is preserved as a distinct failure

#### Scenario: Drain loop in the test body uses a per-iteration deadline

- **GIVEN** a `while let Some(v) = rx.recv().await` drain loop directly
  in a test function body
- **WHEN** the site is converted
- **THEN** the loop exits on channel-close, and a receive that stalls
  past the deadline fails the test (a silent early exit on stall is not
  accepted)

#### Scenario: Receive inside a spawned background task is per-iteration bounded

- **GIVEN** a `tokio::spawn(async move { .. rx.recv().await .. })`
  background drainer or pipeline simulator in a test body
- **WHEN** the site is converted
- **THEN** each receive iteration is individually deadline-bounded (an
  overall timeout on the whole task is not used, because the task
  legitimately spans the whole test), a stall ends the task's loop, and
  any pre-existing `expect` chain inside the task that carries
  test-alive semantics is preserved with the timeout wrapper so a stall
  still fails observably

#### Scenario: Marker use requires justification

- **GIVEN** a receive site where a deadline is semantically wrong
- **WHEN** the site is adjudicated instead of bounded
- **THEN** the source line carries `// allow-test-wait:` followed by a
  non-empty reason naming why a deadline is wrong for that wait, and
  the conversion log lists the site with that justification

#### Scenario: Ratchet counts only unadjudicated receives at the zero end state

- **GIVEN** `lint-unbounded-wait` reports the workspace unadjudicated
  finding count and every mission-394 inventory site is bounded or
  marker-adjudicated
- **WHEN** the ceiling is lowered to that count
- **THEN** `cargo xtask lint-unbounded-wait` exits 0 at the lowered
  ceiling, and the ceiling is never raised afterward

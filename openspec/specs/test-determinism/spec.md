# test-determinism Specification

## Purpose
TBD - created by archiving change controlbus-flake-fix. Update Purpose after archive.
## Requirements
### Requirement: Wall-clock-independent controlbus stop test

The `controlbus_stops_route` integration test in camel-test SHALL drive its timer
routes with tokio paused time (`CamelTestContextBuilder::with_time_control()` +
`TimeController::advance`) instead of relying on real wall-clock deadlines, so its
pass/fail outcome is independent of CPU contention on the host.

#### Scenario: Stop action completes under virtual time

- **GIVEN** an auto-start timer route and a one-shot trigger route whose pipeline
  calls the ControlBus stop action on the auto route
- **WHEN** the test pumps virtual time in bounded increments (advance + yield
  bursts) and the trigger tick fires
- **THEN** `mock:stop-done` receives its exchange and the auto route's status
  reaches Stopped, with no real-time sleep or wall-clock deadline on the pass path

#### Scenario: Bounded failure instead of hang

- **GIVEN** the stop trigger pipeline never delivers its exchange (regressed
  pipeline)
- **WHEN** the bounded virtual-time pump exhausts its iteration budget
- **THEN** the test fails immediately with an explicit assertion message naming
  the virtual-time budget, rather than hanging on an unbounded endpoint poll; and
  teardown runs on real time because the test resumes the clock before stopping
  the harness

#### Scenario: Flake class eliminated under CPU saturation

- **GIVEN** the host CPU is deliberately saturated while the test runs
- **WHEN** the test executes repeatedly (20 consecutive runs)
- **THEN** all runs pass, because the test clock no longer advances in real time

### Requirement: Event-driven settling for seda integration tests

The seda integration tests in camel-test that assert an exact exchange count on
a bounded-repeat timer pipeline SHALL settle by awaiting exchange arrival
(`await_exchanges`) before stopping the harness, not by sleeping a fixed
wall-clock window, so their pass/fail outcome does not depend on CPU contention
from sibling tests.

#### Scenario: Exact count settles before teardown

- **GIVEN** a timer producer with `repeatCount=K` feeding a seda endpoint whose
  consumer writes to a mock endpoint
- **WHEN** the test awaits K exchanges (Notify-based, generous deadline) and
  then stops the harness
- **THEN** the exact exchange-count assertion passes with exactly K exchanges,
  regardless of how long delivery took within the deadline

#### Scenario: Deadline is a backstop, not a gate

- **GIVEN** sibling tests in the binary run in parallel and delay delivery
  beyond the old fixed sleep window
- **WHEN** delivery still completes within the generous deadline
- **THEN** the test passes — the flake mechanism (window shorter than actual
  delivery under load) is removed

#### Scenario: Full binary stable under repetition

- **GIVEN** the complete seda_test binary (all six tests) which previously
  failed ~4/8 full-binary runs on main
- **WHEN** the binary runs 10 consecutive times
- **THEN** all 10 runs are green

### Requirement: Bounded per-test execution in the Rust library-test job

The Unit Tests (ubuntu-latest) CI job SHALL execute the container-free
Rust library-test scope (`--workspace --lib`) under cargo-nextest 0.9.143
(pinned in CI) with a checked-in `ci`
profile that terminates any single test at an effective ceiling of
approximately 90 seconds (slow-timeout period 30s x terminate-after 3),
retries each failing test exactly once for diagnostics, and treats a
retry-passing test as a FAILED run (`flaky-result = "fail"`).

#### Scenario: Hung test terminates at the ceiling

- **GIVEN** a unit test that never completes (unbounded wait)
- **WHEN** the pilot job runs under the ci profile
- **THEN** each timed-out attempt is terminated at approximately 90
  seconds with an explicit slow-timeout report, remaining selected tests
  continue because `fail-fast = false`, and the final job conclusion is
  failure

#### Scenario: Retry-pass fails the gating job

- **GIVEN** a test that fails on first attempt and passes on retry
- **WHEN** the pilot job runs with retries = 1 and flaky-result = "fail"
- **THEN** the run is reported FLAKY and the job conclusion is failure,
  so a flaky test cannot merge silently

#### Scenario: Container suites stay on cargo test

- **GIVEN** camel-test integration binaries that share containers through
  process-local fixtures
- **WHEN** the pilot lands
- **THEN** the testcontainers, K3s, and bridge suites in the Full Tests
  job keep their existing `cargo test` invocations unchanged

#### Scenario: Selection parity with the cargo baseline

- **GIVEN** the pilot's first run on main
- **WHEN** the nextest-reported selected test count is compared with the
  cargo `--workspace --lib` baseline
- **THEN** the counts match exactly, and any divergence blocks the
  rollout decision recorded in bd rc-mhsn

### Requirement: Nested-cargo tests are color-env insensitive

Every camel-cli test that spawns a nested `cargo` process and parses
its output SHALL be immune to `CARGO_TERM_COLOR=always` in the
environment (CI exports it workflow-wide). Immunity SHALL be achieved
by pinning `CARGO_TERM_COLOR=never` on the spawned cargo child and/or
stripping ANSI escapes at parse sites (the feature_profiles pattern,
bd rc-k6dln), or, where a test parses no cargo-transit output, by a
documented immunity verdict in the audit. The audit SHALL cover:
compile_command_test, compiled_artifact_test, job_one_shot_test,
job_signal_test, lint_test_doc_skip, new_test,
run_empty_discovery_test, run_exec_guard_test, run_signal_test.

#### Scenario: Color-always environment does not break output parsing

- **GIVEN** a camel-cli test from the audit set that parses child
  process output, run with `CARGO_TERM_COLOR=always` exported
- **WHEN** the test executes its assertions against captured stdout
- **THEN** the assertions pass without ANSI contamination (pin applied
  and/or escapes stripped), verified by running the full camel-cli
  suite with `CARGO_TERM_COLOR=always` exported

#### Scenario: Immunity verdicts are recorded

- **GIVEN** a test in the audit set whose parsed output never
  transits a cargo process
- **WHEN** the audit records its verdict
- **THEN** the bd rc-0omir closure note states the test as immune with
  the concrete reason (no cargo child in the parsed path, or camel's
  own piped output carries no ANSI under test capture)

### Requirement: Deadline-bounded await loops in test bodies

Every `loop` in a test function body that carries an `.await` SHALL be
bounded by a deadline: either an enclosing `tokio::time::timeout` over the
whole wait, or an attempt-bounded retry wrapped in an overall timeout —
both failing loudly when the awaited progress never arrives. A
per-iteration timeout whose expiry silently exits the loop SHALL be used
only for true quiescence drains, where expiry is the normal end of the
wait; a loop that waits for a completion marker or other externally
driven progress SHALL NOT use the silent per-iteration form — its expiry
means the progress never arrived, which is a failure. A loop that cannot
carry a deadline without killing the behavior under test (a spawned
long-lived service loop per ADR-0069 §13.2 R1) SHALL be marked
`// allow-test-wait: <reason>` with a non-empty justification. The
`lint-unbounded-wait` ratchet ceiling SHALL decrease when loop sites are
converted and SHALL never increase in this change.

#### Scenario: readiness loop fails loudly instead of hanging

- **GIVEN** a readiness-polling loop that awaits externally driven
  progress (e.g. Redis failover recovery) and the progress never arrives
- **WHEN** the test runs
- **THEN** it fails within the enclosing timeout budget with a message
  naming what was awaited, rather than parking forever on the inner await

#### Scenario: converted sites leave the unadjudicated count

- **GIVEN** the seed inventory at 9ad4290f with 37 loop-class sites and
  a ratchet ceiling of 548
- **WHEN** all conversions and justified markers are applied and
  `xtask lint-unbounded-wait` runs
- **THEN** each of the 37 sites is no longer counted (converted or
  suppressed with its recorded reason) and the ratchet ceiling is
  lowered to 511

#### Scenario: escape marker requires justification

- **GIVEN** a spawned long-lived service loop where an internal deadline
  would terminate the service under test
- **WHEN** the site is marked `// allow-test-wait: <reason>`
- **THEN** the reason is non-empty, references the ADR-0069 R1
  service-loop exception, and the owning test bounds its readiness
  assertions and teardown

#### Scenario: no behavior drift in passing tests

- **GIVEN** a test containing a converted loop that previously passed
- **WHEN** the bounded version runs and readiness arrives normally
- **THEN** the test still passes, with the loop exiting through its
  original success path

### Requirement: Unbounded-wait lint resolves aliased and glob-imported paths

The `lint-unbounded-wait` detector SHALL classify bounding timeout
calls and unbounded wait/spawn calls regardless of import form:
fully-qualified paths, single-segment imported names, function aliases,
module aliases used as qualified-call prefixes, transitive
alias-of-alias imports, and glob-imported names (including glob-derived
qualified prefixes such as `use tokio::*; task::spawn(…)` and
glob-derived prefixes inside alias targets such as `use tokio::*; use
task::spawn as s;`). Leading-colon absolute paths (`::tokio::…`) SHALL
resolve literally, bypassing alias expansion, per Rust absolute-path
semantics. Name resolution SHALL collect the set of plausible readings
scope-first: only a namespace-KNOWN binding terminates resolution —
a local item in the syntactic value category (`fn`, `const`,
`static`, tuple-struct/unit-struct constructors; named-field structs
and enums have no value-namespace callable and are excluded) terminates
single-segment value lookups, and a type/module-namespace item
(`mod`, `struct`, `enum`, `union`, `trait`, `type`) terminates
qualified first-segment lookups. An explicit IMPORT's namespace is
not determinable syntactically, so an import hit SHALL add its
reading(s) — same-scope same-name imports union, never overwrite —
and the walk SHALL continue through same-scope glob roots and outer
scopes (a type-only import must not mask a value reading from a glob
or an outer import). A glob root contributes a candidate and the walk
continues outward because a glob may not export the name. Top-level
fn-body `use` statements and item statements follow the same rules
within the fn-body scope; local bindings
(let / closure / for / if-let / match-arm patterns, and nested-block
`use` statements and item statements) contribute NON-TERMINAL
candidates, with sibling conflicts unioned (never overwritten) — a
late or nested local binding SHALL NOT suppress detection of an
awaited import call elsewhere in the fn. Same-scope imports binding
one name in different namespaces SHALL union their readings as
candidates (never overwrite). Absolute import targets (`use ::tokio::…
as t;`) and `extern crate … as …;` aliases SHALL be collected with
their absolute semantics preserved and SHALL be immune to alias
rewriting. The literal
qualified path SHALL be a fallback reading only when the first segment
resolves through no binding at all (an aliased first segment — e.g.
`use crate::fake as tokio;` — shadows the extern-crate name). Matching
SHALL be class-aware: a wait/spawn target SHALL match when any
candidate hits. Boundedness SHALL follow precedence rules over
provenance-tagged candidates (Named / Glob / Literal / Local;
bare-name artifacts are detection aids, never resolutions): (rule 1)
a Named candidate that is a bounding target bounds when it originates
from the fn-body top scope or a module scope (nested-block Named
readings only block, never bound), no other Named candidate, no Local
reading, and no glob candidate in a scope
INNER to the named reading exists; (rule 2, explicit scope
relaxation) a Literal candidate that is a bounding target bounds when
no Named candidate and no Local reading exists — glob candidates do
not block (a glob displaces an extern reading only by exporting a
same-named module; identity re-exports are benign, non-identity
masquerade is adversarial and corpus-zero); (rule 3) when every
candidate is glob-derived, a singleton target bounds. A named
non-target candidate, two conflicting named candidates, a Local
reading, an inner-scope glob against rule 1, or no target candidate
SHALL NOT bound and SHALL NOT suppress a wait finding. Only a
namespace-known value item binding SHALL terminate a single-segment
value lookup; no import and no other local binding SHALL terminate it
(a local can never be a path prefix, so value-namespace readings never
affect qualified first segments). Transitive alias expansion SHALL terminate
on repeated leading alias names (growing suffix cycles such as
`a → b::x` with `b → a::y`). The ratchet ceiling SHALL land at exactly
395 for this change — the 394 baseline plus ONE new true finding from
the fixed false-negative class (an awaited `TcpStream::connect` through
a named import, previously matched literally and missed), inventoried
in the ratchet file with a dated justification — and SHALL never rise
for any other reason.

#### Scenario: module-aliased timeout bounds inner waits

- **GIVEN** a test fn with `use tokio::time as clock;` and an awaited
  wait inside the future argument of
  `clock::timeout(d, async { rx.recv().await })`
- **WHEN** the detector scans the file
- **THEN** no unbounded-wait finding is reported for the inner wait

#### Scenario: module-aliased spawn await is detected

- **GIVEN** a test fn with `use tokio::task as task;` and an awaited
  `task::spawn(work())` call
- **WHEN** the detector scans the file
- **THEN** an unbounded-wait finding is reported for the awaited spawn

#### Scenario: transitive alias chain resolves to the tokio deadline

- **GIVEN** a test fn with `use tokio::time as clock;` plus
  `use clock::timeout as t;` and an awaited wait inside `t(d, fut)`
- **WHEN** the detector scans the file
- **THEN** no finding is reported (the chain expands to
  `tokio::time::timeout`)

#### Scenario: growing alias cycle terminates resolution

- **GIVEN** imports forming a suffix-growing alias cycle
  (`use m1::a as b;` / `use m2::b as a;`-style leading-name cycle) and
  a call through one of the aliases
- **WHEN** the detector expands the path
- **THEN** expansion stops at the repeated leading name, no candidate
  matches a bounding target, and inner waits are reported
  (conservative direction)

#### Scenario: glob-derived qualified prefix is detected

- **GIVEN** a test fn with `use tokio::*;` and an awaited
  `task::spawn(work())` call
- **WHEN** the detector scans the file
- **THEN** an unbounded-wait finding is reported (the glob root expands
  the `task` prefix to `tokio::task`)

#### Scenario: glob-imported timeout bounds loop subtrees

- **GIVEN** a test fn with `use tokio::time::*;` (no competing provider
  of the name anywhere in scope) and an await-carrying `loop` whose
  body calls bare `timeout(d, …)` each iteration
- **WHEN** the detector scans the file
- **THEN** the loop is bounded and no loop finding is reported (the
  singleton glob candidate is unambiguous)

#### Scenario: local shadow beats glob import

- **GIVEN** a test fn with `use tokio::time::*;` and a local
  `fn timeout(…)` binding, calling bare
  `timeout(d, async { rx.recv().await }).await`
- **WHEN** the detector scans the file
- **THEN** the inner `.await` is still reported (the shadowed name is
  not treated as a tokio deadline)

#### Scenario: same-scope explicit import and glob stay ambiguous

- **GIVEN** a test fn importing `timeout` explicitly from a non-tokio
  module in the same scope as a `use tokio::time::*;` glob, awaiting a
  wait inside `timeout(d, fut)`
- **WHEN** the detector scans the file
- **THEN** the inner wait is reported (both readings are kept as
  candidates — the explicit import's namespace is unknown, so the
  glob reading cannot be discarded; ambiguity never bounds)

#### Scenario: type-only import does not mask same-scope glob value

- **GIVEN** a module with `use types::spawn;` (a type-only import of
  that spelling) and `use tokio::task::*;` in the same scope, and a
  test fn awaiting bare `spawn(work())`
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (the named-import hit does
  not terminate; the same-scope glob contributes the
  `tokio::task::spawn` value reading)

#### Scenario: scope-local item shadows glob import

- **GIVEN** a module declaring `fn timeout(…)` alongside
  `use tokio::time::*;` and a test fn in that module calling bare
  `timeout(d, async { rx.recv().await }).await`
- **WHEN** the detector scans the file
- **THEN** the inner `.await` is reported (the explicit local item is
  terminal and wins over the glob in the same scope)

#### Scenario: inner non-matching glob does not suppress outer wait import

- **GIVEN** an outer module scope with `use tokio::task::spawn;`, a
  test fn in an inner module (sees the outer import via `use super::*;`)
  that also glob-imports a non-tokio module (`use
  crate::helpers::*;`), and an awaited bare `spawn(work())` call
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (the reading is ambiguous —
  glob candidate plus outer import — and ambiguity never suppresses a
  wait finding)

#### Scenario: nested block import cannot wrongly bound the fn

- **GIVEN** a module scope importing `timeout` from a non-tokio module,
  a test fn whose body declares `use tokio::time::timeout;` inside a
  nested block only, and an awaited wait inside a bare
  `timeout(d, fut)` call outside that block
- **WHEN** the detector scans the file
- **THEN** the inner wait is reported (the flattened body import is a
  non-terminal candidate; the competing outer binding makes the
  reading ambiguous, and ambiguity never bounds)

#### Scenario: aliased extern name cannot bypass resolution

- **GIVEN** a test fn module with `use crate::fake as tokio;` and an
  awaited wait inside `tokio::time::timeout(d, fut)` (the local alias
  shadows the extern crate name)
- **WHEN** the detector scans the file
- **THEN** the inner wait is reported (the literal path is only a
  fallback reading; the aliased first segment produces the non-target
  candidate `crate::fake::time::timeout`)

#### Scenario: tuple-struct constructor shadows glob import

- **GIVEN** a module declaring `struct timeout<D, F>(D, F);` alongside
  `use tokio::time::*;` and a test fn in that module calling bare
  `timeout(d, async { rx.recv().await }).await`
- **WHEN** the detector scans the file
- **THEN** the inner `.await` is reported (the value-namespace constructor
  is a tracked local item and terminates resolution before the glob)

#### Scenario: top-level fn-body import and outer import stay ambiguous

- **GIVEN** a module with `use tokio::task::spawn;`, a test fn whose
  body top level declares `use crate::helpers::spawn;` and awaits bare
  `spawn(work())`
- **WHEN** the detector scans the file
- **THEN** an unbounded-wait finding is reported (imports are never
  terminal — the body reading and the outer tokio reading both remain
  candidates, and ambiguity never suppresses a wait finding)

#### Scenario: conflicting sibling block imports stay ambiguous

- **GIVEN** a test fn where one nested block declares
  `use tokio::time::timeout;` and a sibling block declares
  `use crate::helpers::timeout;`, with an awaited wait inside a bare
  `timeout(d, fut)` call in the fn body
- **WHEN** the detector scans the file
- **THEN** the inner wait is reported (both readings are kept as
  candidates; ambiguity never bounds)

#### Scenario: type alias does not suppress value-namespace call

- **GIVEN** a module with `type spawn = ();` alongside
  `use tokio::task::spawn;` and a test fn awaiting bare `spawn(work())`
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (the type-namespace alias does
  not terminate the value-namespace lookup)

#### Scenario: nested block glob import is detected

- **GIVEN** a test fn with a nested block containing
  `use tokio::task::*;` and an awaited bare `spawn(work())` inside that
  block
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (the nested glob root
  contributes the `tokio::task::spawn` candidate)

#### Scenario: glob-derived prefix inside an alias target resolves

- **GIVEN** a test fn module with `use tokio::*;` and
  `use task::spawn as s;` (the `task` prefix is glob-derived) and an
  awaited `s(work())` call
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (expansion branches through
  the glob root: `s` → `task::spawn` → `tokio::task::spawn`)

#### Scenario: leading-colon absolute path bypasses aliases

- **GIVEN** a test fn module with `use crate::fake as tokio;` and an
  awaited wait inside `::tokio::time::timeout(d, fut)`
- **WHEN** the detector scans the file
- **THEN** no finding is reported (the absolute path resolves literally
  to the tokio deadline; the alias does not intercept it)

#### Scenario: enum name does not suppress value-namespace call

- **GIVEN** a module declaring `enum spawn { A }` alongside
  `use tokio::task::spawn;` and a test fn awaiting bare `spawn(work())`
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (the enum name is type
  namespace only; the value-namespace call is the imported spawn)

#### Scenario: named-field struct does not suppress value-namespace call

- **GIVEN** a module declaring `struct spawn { x: u32 }` alongside
  `use tokio::task::spawn;` and a test fn awaiting bare `spawn(work())`
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (a named-field struct has no
  value-namespace constructor; the call is the imported spawn)

#### Scenario: late local binding does not suppress earlier detection

- **GIVEN** a test fn importing `tokio::task::spawn`, awaiting bare
  `spawn(work())` early in the body, with a later
  `let spawn = helper_fn;` binding
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (the local binding is a
  non-terminal candidate; it cannot suppress the import reading for
  the earlier call)

#### Scenario: same-name dual-namespace imports keep both readings

- **GIVEN** a module with `use tokio::task::spawn;` and
  `use types::spawn;` (a type-only import of the same spelling) and a
  test fn awaiting bare `spawn(work())`
- **WHEN** the detector scans the file
- **THEN** the awaited spawn is reported (both same-scope readings are
  kept; the type-only import does not overwrite the callable one)

#### Scenario: absolute import target is immune to alias rewriting

- **GIVEN** a module with `use crate::fake as tokio;` and
  `use ::tokio::time::timeout as t;` and a test fn with an awaited
  wait inside `t(d, fut)`
- **WHEN** the detector scans the file
- **THEN** no finding is reported (the absolute target resolves to the
  real `tokio::time::timeout`; the local `tokio` alias cannot rewrite
  an absolute leading segment)

#### Scenario: extern crate alias resolves qualified calls

- **GIVEN** a module with `extern crate tokio as runtime;` and a test
  fn awaiting `runtime::task::spawn(work())`
- **WHEN** the detector scans the file
- **THEN** an unbounded-wait finding is reported (the extern-crate
  alias resolves the prefix to the absolute `::tokio` root)

#### Scenario: named tokio import bounds despite same-scope glob

- **GIVEN** a test module with BOTH `use tokio::time::timeout;` and
  `use super::*;` in the same scope and a test fn whose await-carrying
  `loop` calls bare `timeout(d, …)` each iteration
- **WHEN** the detector scans the file
- **THEN** the loop is bounded and no finding is reported (the named
  target reading wins by same-scope explicit-beats-glob precedence;
  the glob-derived candidate does not block bounding)

#### Scenario: qualified tokio deadline bounds despite glob in scope

- **GIVEN** a test fn in a module with any glob import (`use
  super::*;` or an extern glob like `use streaming::*;`) and an
  awaited wait inside the future argument of a fully-qualified
  `tokio::time::timeout(d, async { … })`
- **WHEN** the detector scans the file
- **THEN** no finding is reported (the extern reading of the qualified
  path is a target and no named binding displaces it; a glob cannot
  shadow an extern-crate name without exporting it)

#### Scenario: inner-scope glob blocks the named rule

- **GIVEN** an outer scope with `use tokio::time::timeout;`, a test fn
  in an inner module that glob-imports its parent (`use super::*;`)
  where the parent exports a same-named helper `timeout`, and an
  awaited wait inside a bare `timeout(d, fut)` call
- **WHEN** the detector scans the file
- **THEN** the inner wait is reported (a glob candidate inner to the
  named reading displaces it in Rust; rule 1 refuses to bound)

#### Scenario: local binding blocks bounding beside a named import

- **GIVEN** a test fn with `use tokio::time::timeout;` and a local
  `let timeout = helper;` binding, awaiting a wait inside a bare
  `timeout(d, async { rx.recv().await })` call
- **WHEN** the detector scans the file
- **THEN** the inner `.await` is reported (the Local reading is a
  plausible resolution and blocks every bounding rule)

#### Scenario: deeper nested glob blocks the named rule

- **GIVEN** a test fn where one nested block declares
  `use tokio::time::timeout;`, a DEEPER nested block declares
  `use crate::helpers::*;` (a glob whose parent exports a same-named
  helper), and an awaited wait inside a bare `timeout(d, fut)` call in
  the deeper block
- **WHEN** the detector scans the file
- **THEN** the inner wait is reported (the glob candidate's recorded
  block depth is greater than the named reading's; rule 1 refuses to
  bound)

#### Scenario: nested-block named import alone does not bound outside its block

- **GIVEN** a test fn where one nested block declares
  `use tokio::time::timeout;` and an awaited wait sits inside a bare
  `timeout(d, fut)` call OUTSIDE that block, with no other provider of
  the name
- **WHEN** the detector scans the file
- **THEN** the inner wait is reported (a nested-block Named reading
  never establishes boundedness; its block-local visibility cannot be
  confirmed for the call site)

#### Scenario: qualified alias immune to value shadow

- **GIVEN** a test fn with `use tokio::time as clock;`, a local
  `let clock = …` binding, and an awaited wait inside
  `clock::timeout(d, fut)`
- **WHEN** the detector scans the file
- **THEN** no finding is reported (a local cannot be a path prefix; the
  qualified call still denotes the tokio deadline)

#### Scenario: ratchet lands at exactly 395 with one inventoried new finding

- **GIVEN** the pre-change detector reports 394 unadjudicated waits
  across the scanned trees
- **WHEN** the alias/glob resolution fix lands and
  `cargo xtask lint-unbounded-wait` runs
- **THEN** the count is exactly 395 at that landing — the precedence
  rules relieve the 108 glob-ambiguity over-reports measured under
  the pure singleton rule while suppressing nothing the base detector
  reported (later tightenings — per-site loop bounding, inline-closure
  analysis — lower the live ceiling below this point-in-time figure;
  see the ratchet file for the current count)
  (verified by a base-vs-new finding-list diff on the same tree: +1
  site, 0 removals), and the single new site is the inventoried true
  finding (awaited `TcpStream::connect` through
  `use tokio::net::TcpStream;`, previously missed by literal matching);
  the ceiling raise 394 to 395 carries the dated justification in the
  ratchet file; any further deviation is adjudicated per ratchet
  convention before landing


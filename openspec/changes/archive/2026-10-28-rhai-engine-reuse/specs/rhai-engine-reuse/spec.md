## ADDED Requirements

### Requirement: Rhai fixed-registration construction is hoisted out of the evaluation path

The rhai language SHALL construct the fixed, stateless engine registrations —
the `StandardPackage` merged module and the stream-guard plus pure-`trim`
shadow registrations — once per process into shared modules, and per
evaluation SHALL install those prebuilt shared modules (`register_global_module`)
in the current registration order (standard package, JSON host module,
resource limits, disabled `eval`/`import` symbols, stream guards plus
`trim` shadow). Construction hoisting SHALL NOT use thread-local state, SHALL
NOT mutate limits at evaluation time, and SHALL NOT introduce an engine mutex
or a process-wide mutable engine registry. Per-evaluation installation of
prebuilt modules and of the captured-map readers remains in the evaluation
path; only construction is hoisted.

#### Scenario: Trivial evaluation meets the overhead gate

- **GIVEN** the overhead bench workload evaluating an empty-script expression
  on the baseline machine
- **WHEN** the bench measures the complete public evaluation path
- **THEN** the recorded full-path median SHALL be at most 100 us (stretch
  goal 25 us)

#### Scenario: Existing workloads do not regress

- **GIVEN** the baseline medians and p95 recorded before the engine change on
  a machine whose provenance (host identity and CPU model) is committed with
  the baseline
- **WHEN** the overhead bench runs after the change on a machine matching
  that recorded host identity and CPU model
- **THEN** every workload in the matrix (trivial, predicate, 20 headers + 20
  properties, 50 headers + 50 properties, mutating, 10-clause choice) SHALL
  have p95 at most 110% of its pre-change baseline
- **AND** the p95 acceptance criterion SHALL be considered satisfied only by
  this matched same-machine comparison; a foreign-machine CI run that skips
  the p95 check SHALL NOT satisfy it
- **AND** the existing `property_scaling` wide/deep read/mutating shapes at
  every measured entry size (100/1000/5000) plus its read-control cases
  SHALL be covered by matched pre-change/post-change `property_scaling` runs
  extended to print p95 per shape, size, and mode, each within 110% of its
  pre-change value, recorded as acceptance evidence (scope exception: they
  are not part of the eval_overhead matrix)

#### Scenario: Registration order preserved

- **GIVEN** a freshly built evaluation engine
- **WHEN** a script calls the JSON helpers, `trim`, or a stream-refusal guard
- **THEN** the host JSON helpers shadow the stock package helpers, `trim`
  returns a new string without mutating the subject, and stream reads are
  refused exactly as before the change

### Requirement: Captured-map readers stay per-evaluation

The `header()` and `property()` snapshot readers SHALL remain closures created
per evaluation over that evaluation's pre-run maps; the prebuilt package
module SHALL NOT carry them, and a later in-script map assignment SHALL NOT
change what a reader returns.

#### Scenario: Reader observes pre-run value after in-script mutation

- **GIVEN** a mutating script that assigns `headers["k"] = "new"` after
  reading `header("k")`
- **WHEN** the script calls `header("k")` again
- **THEN** the reader returns the pre-run value while the mutated map is what
  propagates back to the exchange

### Requirement: Engine reuse leaks no per-evaluation state

Engine construction reuse SHALL NOT carry state from one evaluation into
another. A script that panics, returns early via `?`, fails with a host
error, or nests an evaluation SHALL leave no residue (scope values, stream
refusal counters, snapshot maps) observable by a subsequent evaluation.

#### Scenario: Panic leaves no sentinel residue

- **GIVEN** a first evaluation sets a property sentinel and then panics
- **WHEN** a second evaluation on a fresh exchange lists all properties
- **THEN** the sentinel is absent

#### Scenario: Early return leaves no sentinel residue

- **GIVEN** a first evaluation sets a property sentinel and exits early via a
  function-level `return` (rhai 1.26 reserves the `?` operator without
  implementing it, so the supported early-return path is a normal early
  exit; the error-abort sibling is covered by the throw/host-error
  scenarios)
- **WHEN** a second evaluation on a fresh exchange lists all properties
- **THEN** the sentinel is absent

#### Scenario: Host error leaves no sentinel residue

- **GIVEN** a first evaluation sets a property sentinel and then fails inside
  a host function (for example `parse_json` on a non-string)
- **WHEN** a second evaluation on a fresh exchange lists all properties
- **THEN** the sentinel is absent

#### Scenario: Nested evaluation leaves no snapshot residue

- **GIVEN** evaluations from distinct expression objects interleave on
  concurrent threads (step 1 installs no snapshot frames — the captured-map
  readers are per-evaluation closures over their own maps — so true nesting
  residue is structurally impossible; this scenario pins the interleaving
  substitute)
- **WHEN** both evaluations complete (including the panic path)
- **THEN** each evaluation's readers still resolve its own maps and no
  state from one evaluation is visible to the other

#### Scenario: Concurrent mixed-limit evaluations are independent

- **GIVEN** several evaluations with different `RhaiLimitsConfig` values run
  concurrently on the shared prebuilt package
- **WHEN** each evaluation completes
- **THEN** each observes only its own limits and none observes another
  evaluation's scope, counters, or snapshots

#### Scenario: Timeout does not corrupt the next evaluation

- **GIVEN** a first evaluation times out at the route level while the
  blocking worker keeps running
- **WHEN** a second evaluation starts afterwards
- **THEN** the second evaluation succeeds with a fresh scope and correct
  results

#### Scenario: Stream refusal counters are fresh per evaluation

- **GIVEN** a first evaluation on a stream body accumulates refusal counts
- **WHEN** a second evaluation on a fresh stream body runs the same script
- **THEN** its counters start at zero and the refusal verdict depends only on
  the second evaluation's accesses

#### Scenario: Transactional mutating write-back is unchanged

- **GIVEN** a mutating script that modifies headers and fails at the last
  statement
- **WHEN** the evaluation returns an error
- **THEN** the exchange is untouched, and a successful script commits only
  changed entries, exactly as before the change

### Requirement: Overhead bench guards the evaluation cost in CI

The rhai crate SHALL provide a non-ignored bench that measures, per workload
(trivial, predicate, 20 headers + 20 properties, 50 headers + 50 properties,
mutating, 10-clause choice), one COMPLETE public evaluation call per sample —
engine build, exchange conversion, reader setup, AST execution, result
conversion, and mutating write-back where applicable — and additionally
reports separate construction, conversion, and execution phase timings; it
SHALL print median and p95 for every full-path and phase measurement. The
bench SHALL hard-fail when the trivial full-path median exceeds 100 us or the
choice full-path median exceeds 250 us (both overridable by explicit
environment thresholds), and SHALL check every workload p95 against 110% of a
committed baseline only when the baseline's recorded CPU provenance matches
the current machine, printing SKIP with the reason otherwise (never a silent
pass). The `bench-smoke` CI job SHALL run this bench with explicit thresholds
and fail when they are exceeded.

#### Scenario: Bench separates phases

- **GIVEN** the overhead bench runs
- **WHEN** it reports results for each workload
- **THEN** construction, conversion, and execution timings are reported
  separately with median and p95, and the gate numbers are complete
  evaluation calls rather than sums of phase measurements

#### Scenario: CI fails on threshold breach

- **GIVEN** the `bench-smoke` job runs the overhead guard with its explicit
  threshold environment including the committed baseline reference
- **WHEN** the trivial full-path median exceeds the configured absolute
  threshold, or the choice full-path median exceeds its configured absolute
  threshold, or a provenance-matched p95 exceeds 110% of baseline
- **THEN** the job step exits non-zero with the offending numbers printed

#### Scenario: Foreign-machine p95 check skips with reason

- **GIVEN** the bench runs on a machine whose CPU model differs from the
  committed baseline provenance
- **WHEN** the relative p95 check is evaluated
- **THEN** the bench prints an explicit SKIP line naming the mismatch and
  still enforces the absolute thresholds

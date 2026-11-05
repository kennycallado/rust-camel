## ADDED Requirements

### Requirement: Per-evaluation binding cost does not accumulate

Each rhai evaluation SHALL convert each bound header, property, and body
value at most once. Binding traversals SHALL be bounded by the size of the
bound values and SHALL NOT grow across repeated evaluations of the same
exchange. Write-back change detection SHALL compare values natively and
SHALL NOT compile or evaluate script expressions. Each evaluation SHALL
hold at most one working copy and one pre-run snapshot of each container.
The inbound JSON converter SHALL carry a test-only thread-local node-visit
counter (`CONVERT_NODE_VISITS`), the write-back diff SHALL carry a
test-only engine-evaluation counter (`DIFF_ENGINE_EVALS`), and the working
copy producer SHALL carry a test-only copy counter (`SNAPSHOT_WORKING_COPIES`);
no counter exists in non-test builds. The node counter measures the JSON
inbound converter only; fixtures pin `Body::Json` bodies and JSON-map
containers, and expected counts are computed from fixture node counts.

#### Scenario: write-back diff never evaluates script code

- **GIVEN** a mutating script that writes one key of a property holding
  10,000 scalar leaves
- **WHEN** the script is evaluated and the write-back diff runs
- **THEN** the `DIFF_ENGINE_EVALS` counter SHALL be `0`; change detection
  SHALL NOT compile or evaluate any script expression

#### Scenario: each evaluation converts each bound value exactly once

- **GIVEN** an exchange with a `Body::Json` body and JSON-map headers and
  properties of known node counts
- **WHEN** a read-only evaluation and a mutating evaluation each run with
  the `CONVERT_NODE_VISITS` counter active
- **THEN** each evaluation's counter delta SHALL equal the total node count
  of the bound values exactly once per node, not a multiple of it

#### Scenario: repeated evaluations do not accumulate copies

- **GIVEN** the same unchanged exchange evaluated N=5 times with the
  `CONVERT_NODE_VISITS` and `SNAPSHOT_WORKING_COPIES` counters active
- **WHEN** all five evaluations complete
- **THEN** the node counter SHALL be exactly 5 times the single-evaluation
  node count, and each evaluation SHALL hold at most one working copy and
  one pre-run snapshot per container (`SNAPSHOT_WORKING_COPIES` at most one
  per container per evaluation, reader registration contributing zero deep
  copies)

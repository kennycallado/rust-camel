## ADDED Requirements

### Requirement: Language group hook is object-safe and optional by default

`camel-language-api` SHALL expose an object-safe `PredicateGroup` trait —
declared with the crate's `#[async_trait]` convention, `async fn
first_match(&Exchange) -> Result<Option<usize>, IndexedPredicateError>` —
and a defaulted `Language::create_predicate_group(&self, scripts: &[&str]) ->
Result<Option<Box<dyn PredicateGroup>>, LanguageError>` whose default
implementation returns `Ok(None)`. A language that does not override the hook
SHALL remain fully functional through the existing per-clause path, and the
addition SHALL NOT change any existing trait method signature.

#### Scenario: Non-implementing language is unaffected

- **GIVEN** a language crate that does not override `create_predicate_group`
- **WHEN** a Choice with multiple `when` clauses in that language is compiled
  and evaluated
- **THEN** compilation uses the existing per-clause sequential path and
  evaluation behavior is identical to the pre-hook behavior

#### Scenario: Group hook stays object safe

- **GIVEN** the `Language` trait used through `Arc<dyn Language>` registries
- **WHEN** `create_predicate_group` is called with the clause scripts
- **THEN** the returned value is `Option<Box<dyn PredicateGroup>>` and the
  call compiles without generics or non-object-safe types leaking through the
  trait boundary

### Requirement: Rhai group evaluation uses one engine and one snapshot per message with per-predicate isolation

The rhai language SHALL implement `create_predicate_group` so that a
`first_match` call uses ONE evaluation engine and ONE exchange snapshot
(body, headers, properties converted at most once per call), evaluates each
pre-compiled clause AST in declaration order against that engine, and gives
each predicate a FRESH `Scope`. Per-clause resource budgets and stream
counters are never shared or accumulated across clauses: the shared engine's
operation counter starts fresh for every clause evaluation (rhai's per-eval
`GlobalState`), and a test SHALL pin this isolation. A stream-marker body
SHALL keep per-clause body conversion (no snapshot sharing for the body), so
its stream counters stay per-predicate.

#### Scenario: One engine and snapshot per evaluation

- **GIVEN** a rhai group of 10 clauses and an exchange that matches none of
  them
- **WHEN** `first_match` runs
- **THEN** the exchange snapshot is converted once and one engine serves all
  clause evaluations (structurally enforced and covered by the group's unit
  tests), while every clause evaluation observes its own budget and scope

#### Scenario: Skipped clauses are never evaluated

- **GIVEN** a rhai group of clauses where clause 1 matches
- **WHEN** `first_match` runs against an exchange
- **THEN** only clauses 0 and 1 are evaluated and no later clause performs
  snapshot reads, engine work, or AST evaluation

#### Scenario: Per-clause budgets do not accumulate on the shared engine

- **GIVEN** a rhai group with operation limit L where clause 0 and clause 1
  are both terminating predicates that each consume more than L/2 but less
  than L, and both return false
- **WHEN** `first_match` runs
- **THEN** both clauses evaluate and the result is `Ok(None)` — a cumulative
  counter would have failed clause 1, so success here proves budget isolation

#### Scenario: Budget breach is reported for the offending clause only

- **GIVEN** a rhai group with a tight operation limit where clause 0 returns
  false and clause 1 contains an unbounded script loop
- **WHEN** `first_match` runs
- **THEN** the error is reported for clause 1, and clause 2 (and any later
  clause) is never evaluated

#### Scenario: Per-clause execution timeouts stay independent

- **GIVEN** a rhai group whose clauses are individually in-budget but whose
  aggregate evaluation duration exceeds one `execution_timeout_ms` period
- **WHEN** `first_match` runs
- **THEN** every clause is evaluated (each with its own full timeout) and the
  result reflects the clause outcomes, not a shared timeout

#### Scenario: Clause timeout yields an indexed error and stops evaluation

- **GIVEN** a rhai group where clause 1's script exceeds its own
  `execution_timeout_ms` and clause 0 returns false
- **WHEN** `first_match` runs
- **THEN** the error is a `Timeout`-class `LanguageError` reported for
  clause 1, and clause 2 (and any later clause) is never evaluated

### Requirement: Same-language Choice uses the group path and mixed languages fall back

Route compilation of a declarative Choice SHALL use the group path when every
`when` clause predicate is written in the same language AND that language's
`create_predicate_group` returns a group; in every other case — mixed
languages, group-not-supported, zero or one clause — compilation SHALL keep
the existing per-clause sequential path unchanged. Filter and Split SHALL NOT
gain grouping or context sharing.

#### Scenario: Mixed-language Choice falls back to sequential

- **GIVEN** a declarative Choice whose clauses alternate between two languages
- **WHEN** the route compiles
- **THEN** the compiled segment is the sequential ChoiceSegment with per-clause
  opaque predicates, identical to the pre-change compilation

#### Scenario: Same-language rhai Choice compiles to the group segment

- **GIVEN** a declarative Choice with multiple rhai `when` clauses
- **WHEN** the route compiles
- **THEN** the compiled segment evaluates predicates through the rhai group
  and runs the first matching clause's body, preserving clause order

### Requirement: Grouped Choice preserves sequential semantics and attribution

The grouped Choice segment SHALL preserve clause order and first-match
selection, SHALL fail the step (never fall through to otherwise) when a
predicate evaluation errors, SHALL map the indexed error through the
offending clause's `EvalMeta` via `to_expression_failed` (same carrier,
redaction, and metadata as the sequential path), and SHALL run otherwise (or
pass through) only when no clause matches. The grouped path SHALL build one
`EvalMeta` per clause that distinguishes the clause position in diagnostics
and logs (the trusted `verb` carries `choice/when[<index>]`), so
`ExpressionFailed` errors and log attribution identify the offending clause
even though all clauses share the outer step id.

#### Scenario: Error in clause N is attributed to clause N

- **GIVEN** a rhai group where clause 2's script raises a runtime error and
  earlier clauses return false
- **WHEN** the Choice evaluates a message
- **THEN** the step fails with `ExpressionFailed` carrying clause 2's
  language/route/step metadata with clause position `choice/when[2]` and the
  redacted cause, and no later clause is evaluated

#### Scenario: No match runs otherwise

- **GIVEN** a rhai group where no clause matches and an otherwise body exists
- **WHEN** the Choice evaluates a message
- **THEN** every clause is evaluated once and the otherwise body runs on the
  original exchange

### Requirement: Choice grouping overhead bench gate

The processor Choice Criterion bench SHALL include, measured against a
`--save-baseline choice-pre` pre-change run:

- sequential same-language rhai shapes: a 1-clause control and a 10-clause
  shape (controls, kept sequential);
- a mixed-language 10-clause sequential control;
- grouped same-language rhai 10-clause shapes at every match position:
  match-first, match-middle, match-last, and no-match (all clauses evaluated —
  the worst case).

The 10-clause same-language grouped Choice median SHALL be at most 250 us at
every covered match position including no-match, and every sequential control
SHALL NOT regress against its `choice-pre` baseline. Evidence (pre medians,
post medians, criterion estimates) SHALL land in `benchmarks/records/`.

#### Scenario: Ten-clause grouped choice meets the gate

- **GIVEN** the recorded `choice-pre` baseline for the processor choice bench
- **WHEN** the grouped 10-clause same-language shapes (match-first,
  match-middle, match-last, no-match) run after the change on the same
  machine class
- **THEN** every shape's median is at most 250 us and the recorded evidence
  (pre medians, post medians, criterion estimates) lands in
  `benchmarks/records/`

#### Scenario: Sequential controls do not regress

- **GIVEN** the recorded `choice-pre` baseline
- **WHEN** the 1-clause, 10-clause sequential, and mixed-language controls
  run after the change
- **THEN** each control's median is within the noise band of its baseline
  value (no regression beyond criterion-reported noise)

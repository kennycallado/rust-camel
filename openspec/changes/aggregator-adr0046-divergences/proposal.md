# Proposal: aggregator-adr0046-divergences

## Why

The Aggregator EIP satisfies ≥1 ADR-0046 §1 divergence-density markers
(stateful correlation/completion + temporization with completion/timeout), so
the full inspiration-not-conformance protocol applies. A spike (bd `rc-mybm`,
discovered-from inventory indexed under `rc-mybm-spike`) read 3 representative
Apache Camel aggregator tests and classified 5 forced divergences (D-A1..D-A5)
plus 1 coverage gap (G-A1). KPI ratio divergences/(divergences+gaps) = 5/6 =
0.83, confirming the protocol did NOT over-invest (Aggregator is genuinely
high-divergence-density, unlike stateless EIPs where an audit suffices).

Per ADR-0046 §Anti-patterns §5, these divergences MUST land in tracked docs
(`CONTEXT.md` / ADRs), not gitignored spike artifacts — otherwise they survive
only in conversation memory and are lost to compaction. The Aggregator
behavior is already mature (1819 lines, 37 tests, full
correlation/completion/timeout/force-on-stop logic); NO production code change
is expected. This change is pure documentation + native test pinning.

## What Changes

**Included:**
- 5 new sections under `crates/camel-processor/CONTEXT.md` documenting D-A1..D-A5
  (one block per divergence: statement, why forced, which ADR, observable
  consequence).
- 1 gap-coverage note (G-A1) in the same CONTEXT.md.
- Native tests (CamelTestContext / existing aggregator test harness) pinning
  OUR semantics for each divergence — these assert what rust-camel DOES, not
  what Camel does (ADR-0046 §Anti-pattern §2: no literal-assert translation).

**Excluded:**
- Any production code change to `aggregator.rs` or `camel-api/src/aggregator.rs`.
  If implementation surfaces a real bug (not a divergence), it is filed as a
  separate bd issue and deferred — out of scope for this change.
- A new ADR. None of the 5 divergences is fundamental enough to warrant one
  (D-A5 is forced by ADR-0033; D-A1/D-A2 by the `AggregationFn` contract
  shape; D-A3 by the nonblocking `force_complete_all() -> ()` plus bounded
  `late_tx`; D-A4 by the existing timeout/task-cap/TTL configuration
  contracts). If the plan-bless
  disagrees, escape to PHASE 1 revision.

**Affected crates:** `camel-processor` (CONTEXT.md + behavioral tests in
`src/aggregator.rs`); `camel-api` (a `compile_fail` doctest on `AggregationFn`
and a typed-variant unit test in `src/aggregator.rs`). No production behavior
or signature change in either crate.

## Acceptance criteria

- `crates/camel-processor/CONTEXT.md` contains a dedicated "Aggregator EIP
  divergences from Apache Camel (ADR-0046 protocol)" section with one
  sub-section per divergence D-A1..D-A5 and the G-A1 gap note.
- Each divergence sub-section names the forcing ADR or existing contract
  shape and the observable consequence for an operator coming from Camel.
- Native tests exist and pass, pinning our semantics for D-A1 (binary fold,
  no null first-call), D-A2 (strategy cannot Err), D-A3 (force-completion
  channel path + drop-under-pressure), D-A5 (validate rejects unbounded).
  D-A4 and G-A1 are mechanism/knob notes with no new behavioral test (covered
  by existing timeout/TTL tests).
- All 6 bd sub-issues (`rc-rqbr`, `rc-0xtn`, `rc-mfah`, `rc-3aw9`, `rc-i7rn`,
  `rc-eiw1`) closed with reason referencing the CONTEXT.md commit.
- Quality gates green (fmt, clippy, build, lib tests, xtask lints, schema,
  audit). No production-code change means the Rust gates are near-N/A except
  for the new test code.

## Risk budget

**Acceptable:** documentation drift if a future refactor changes aggregator
behavior without updating CONTEXT.md (mitigated by referencing exact source
line ranges). Test additions that lock in current behavior as "correct" when
it is merely "current" — mitigated by framing tests as semantic pins
(ADR-0046 §4), not conformance.

**Out of bounds:** any change to `AggregatorService` behavior, the
`AggregationFn` signature, or `AggregatorConfig::validate()`. If the work
surfaces that a divergence is actually a bug, STOP and file a separate bd
issue rather than fixing inline.

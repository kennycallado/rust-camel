## ADDED Requirements

### Requirement: Branch-exchange construction splits the in-flight claim

The system SHALL provide `Exchange::branch_clone()` in `camel-api` as
the sole constructor for fanout branch copies produced by cloning a
source `Exchange`. For a source exchange carrying an in-flight claim,
`branch_clone()` SHALL mint one sibling claim via
`InFlightClaim::split()` onto the copy, so each detached branch copy
counts on the context-global gauge and releases independently on
completion; the source's own claim SHALL remain live and unchanged.
`Exchange::clone` SHALL keep setting `in_flight_claim: None` and
`InFlightClaim` SHALL remain non-`Clone`. Fresh-construction fanout
sites that assemble a new `Exchange` from parsed entry data and carry
no relation to a claimed parent (e.g. tar/archive per-entry exchanges)
are outside the helper's scope. The split-fragment construction seam
is IN scope: `fragment_exchange` (`camel-api/src/splitter.rs`) and any
equivalent segment/streaming fragment builder SHALL split the parent's
carried claim onto every fragment, because each fragment is a fanout
branch exchange of the parent. Every
fanout clone site under `crates/camel-processor/src` and every
exchange-clone site under `crates/camel-core/src/lifecycle/adapters/`
(including `step_compilers/splitting.rs`, `exchange_uow.rs`, and
`route_compiler.rs` boundary/error snapshots) SHALL construct branch
copies through `branch_clone()`. A site MAY keep plain `clone()` only
when the copy provably completes before the source's claim drops and
never escapes the current claim scope, with the justification recorded
in the implementing task report.

#### Scenario: branch copy carries a split claim

- **GIVEN** an exchange holding one live `InFlightClaim` against a
  gauge with total 1
- **WHEN** `branch_clone()` produces a copy
- **THEN** the gauge total is 2, the copy carries its own claim, and
  the original claim is still attached to the source

#### Scenario: sibling releases independently

- **GIVEN** a source exchange and one `branch_clone()` copy with gauge
  total 2
- **WHEN** the copy's claim is dropped
- **THEN** the gauge total is 1 while the source claim remains live,
  and drops to 0 only when the source claim is dropped

#### Scenario: plain clone carries no claim

- **GIVEN** an exchange holding one live claim
- **WHEN** `Exchange::clone` produces a copy
- **THEN** the gauge total is 1 and the copy carries no claim

#### Scenario: detached fanout copy keeps the drain gauge nonzero

- **GIVEN** a route with a wiretap (or divert) branch and a
  deterministic barrier parked inside the branch consumer only
- **WHEN** the main exchange completes while the branch copy is still
  parked behind the barrier
- **THEN** `total_in_flight()` is at least 1 until the barrier
  releases and the branch copy completes, and returns to 0 afterwards

#### Scenario: multicast branch copy outlives the parent claim

- **GIVEN** a route with a multicast of two recipients where one
  recipient fire-and-forget dispatches its copy to a downstream
  consumer route that parks it on a deterministic barrier, and the
  other recipient returns immediately
- **WHEN** the multicast aggregation completes, the parent pipeline
  replies, and the parent's own claim is released
- **THEN** `total_in_flight()` is at least 1 — held only by the
  branch copy's split claim — until the barrier releases and the copy
  completes, and returns to 0 afterwards

#### Scenario: split segment copy outlives the parent claim

- **GIVEN** a splitter route whose per-segment copy is handed to a
  downstream consumer route that parks on a deterministic barrier
- **WHEN** the splitter completes and the parent's own claim is
  released while the segment copy is still parked
- **THEN** `total_in_flight()` is at least 1 — held only by the
  segment copy's split claim — until the barrier releases, and
  returns to 0 after the copy completes

# Design: 485-dot-ry-dsl-decisions

## Approach

Three recorded decisions, no behavior change. The pre-flight expert
(`@experts/e_opus`) ruled B/B/B: document divergence, document the marker rule,
keep the ASTs exhaustive.

## Evidence (verified current behavior)

| Fact | Source |
|---|---|
| `clear_error()` sets `error = None` and removes `CamelExceptionMessage`, `CamelExceptionKind`, `CamelExceptionCaught` | `crates/camel-api/src/exchange.rs:185-191` |
| `handle_error()` inserts `CamelExceptionHandled = true` then calls `clear_error()` | `crates/camel-api/src/exchange.rs:194-200` |
| `DoTrySegment` removes a stale `PROPERTY_EXCEPTION_HANDLED` on block entry | `crates/camel-processor/src/do_try_segment.rs:132-135` |
| `Handled` arm calls `next.handle_error()`; `Continued` arm calls `next.clear_error()` | `crates/camel-processor/src/do_try_segment.rs:197-211` |
| Four `handle_error()` sites: `DoTrySegment` `Handled`; `execute_on_steps` (retry-service) `Handled`; `handle_step` `on_exception` steps `Handled`; `handle_boundary` `on_steps` `Handled`. Dead-letter-channel dispatch uses `clear_error()` and never sets the marker. | `crates/camel-processor/src/error_handler.rs:31-40, 296-345, 388-436, 617-638` |
| No in-tree consumer reads `CamelExceptionHandled` for control flow; control flow matches `PipelineOutcome` | `crates/camel-api/src/pipeline_outcome.rs`; do-try spec scenarios |

These restore Java Camel's symbolic property names but not its retention rule:
Java keeps `EXCEPTION_CAUGHT`, this runtime clears it with the error. That is
the documented divergence below, not an implementation accident.

### Decision 1 — `CamelExceptionCaught` retention (rc-zaqjd)

Record the divergence. `clear_error()` means "this exchange carries no error"
and removes the derived exception properties. Both `doTry` dispositions and the
route-level handler call the same method. Restoring the property in
`DoTrySegment` alone would split doTry from the route-level handler. Restoring
it inside `clear_error()` would leave a stale exception readable after the error
was cleared (redelivery and exchange reuse). Both are worse than a documented
divergence.

The supported pattern is to read or copy the exception inside the catch body
(for example into a user property or header). Steps after a `Handled` or
`Continued` exit must not rely on `CamelExceptionCaught`.

Artifacts: ADR-0025 amendment + `docs/src/concepts/error-handling.md` divergence
note + one-line pointer in `docs/src/eip/do-try.md`. The do-try spec is not
changed; its "separate decision" clause stays true because this ADR records the
decision.

### Decision 2 — `CamelExceptionHandled` canonical rule (rc-ho9j1)

Record the rule in ADR-0025 as normative, with unambiguous wording:

1. Control flow keys off `PipelineOutcome` only.
2. `CamelExceptionHandled` is Camel-compatibility only; no core logic reads it.
3. Only a `Handled` arm can set the marker, and only where that arm calls
   `Exchange::handle_error()`: the doTry `Handled` arm and the route-level
   `Handled` paths that execute `on_exception` steps (step, boundary, and
   retry-service variants). A `Handled` arm that dispatches to the
   dead-letter channel clears with `clear_error()` and does not set it.
   `Continued` never sets it. Any path that clears through `clear_error()`
   drops the caught exception with the error.
4. The two dispositions differ in outcome, not in error clearing: `Handled`
   returns `Completed` and stops the block; `Continued` returns `Completed` and
   advances to the next step after the block.
5. `DoTrySegment` **removes** a stale marker when the block starts. An absent
   marker, a marker removed on entry, and a marker retained from a nested
   `Handled` block are three distinct states; only the third is observable after
   the block.
6. `clear_error()` does not remove the marker, so an outer `Continued` does not
   clear a marker that a nested `Handled` block set.

The code matches all six points (see the evidence table). `docs/src/eip/do-try.md`
repeats points 2, 3, and 6 in user terms. Normative spec scenarios for points 3
and 6 are filed as bd **rc-ud3rl** (P4, discovered-from `rc-ho9j1`); no code or
spec delta here.

### Decision 3 — public DSL AST enums (rc-b48du)

Keep `RouteDslStep` and `DeclarativeStep` exhaustive by design. In a single
workspace `#[non_exhaustive]` has no effect inside `camel-dsl` but forces `_ =>`
arms in camel-core, camel-cli, camel-config, and camel-lint. Those exhaustive
matches are the compile-time guard that every new DSL step is wired through
compile, lint, and CLI. That guard is worth more than the semver cost. The enums
are authoring ASTs, not contract enums; ADR-0049 covers only the three contract
crates (camel-api, camel-component-api, camel-language-api).

`DeclarativeStepKind` (`crates/camel-dsl/src/contract.rs:2-51`) is the intended
external discriminant: it names the authorable step kinds and omits internal-only
variants such as `ResponseMarshal`, which have no kind. It is also an exhaustive
public enum, so adding a kind breaks downstream exhaustive matches and requires
the same breaking-change release discipline; it is not a `#[non_exhaustive]`
contract. Matching `DeclarativeStep::kind()` hides internal-only AST variants but
is not inherently forward-compatible: consumers still need a wildcard fallback
for future kinds.

ADR-0049 gains a scope clarification: the two AST enums are intentionally
exhaustive; a new AST variant (authorable or internal/doc-hidden) is a breaking
change and must appear in release notes. The internal `ResponseMarshal` variants
keep their "internal, do not construct" rustdoc; `#[doc(hidden)]` is deferred to
a later code PR (not needed to close the ticket).

## Affected crates

None. Docs and ADRs only: `docs/adr/0025-*.md`, `docs/adr/0049-*.md`,
`docs/src/concepts/error-handling.md`, `docs/src/eip/do-try.md`,
`CONTEXT-MAP.md`.

## Architecture boundaries

Documentation only. No Runtime, DSL, Component, Service, Language, or Function
change. No public API or signature change.

## Alternatives considered

- **Restore `CamelExceptionCaught`** in `DoTrySegment` or in `clear_error()` —
  rejected: per-site inconsistency or stale exception state.
- **Set `CamelExceptionHandled` in `Continued`** — rejected: contradicts the
  Camel-compatible rule, the mission 479 design, and the existing spec scenario.
- **Add `#[non_exhaustive]` to both AST enums** — rejected: loses the internal
  compile-time wiring guard and is itself a downstream break today.
- **New ADR for the AST freeze** — rejected: ADR-0049 already owns the workspace
  `#[non_exhaustive]` policy; a scope clarification belongs there.

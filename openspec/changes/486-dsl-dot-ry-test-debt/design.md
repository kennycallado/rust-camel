# Design: 486-dsl-dot-ry-test-debt

## Approach

### Shared compiled-step drive rig (rc-v4u92)

`crates/camel-dsl/tests/common/mod.rs` already owns the HTTP client/server
helpers (`SERVER_MUTEX`, `spawn_test_server`, `http_roundtrip`,
`simple_request`, `reply_ok`, `compile_header_step`). Add the step-drive
family beside them:

- `compiled_rest_steps(yaml, op_id, marshal_label)` — parses, finds the
  lowered route, and pins the four positional assertions (6 steps; 0 and 1
  are `Processor`; 2 is `DeclarativeSetHeader`; 3 is `Processor`). The
  marshal position is named by the caller so the existing diagnostics keep
  saying "marshal" (XML battery) or "response marshal" (marshal-error
  battery).
- `compile_step_for_drive(step)` — the superset of both copies:
  `Processor` unwraps, header steps delegate to `compile_header_step`, and
  a literal `DeclarativeSetBody` compiles through `SetBody`, exactly like
  the core declarative set-body compiler.
- `Mutator<'a>` and `drive_steps(steps, ex, inject)` — the optional
  `(index, mutator)` tuple replaces both older shapes: the XML battery
  injects at index 3, the marshal-error battery passes `None`.
- `serve_pipeline(rx, steps, expected, inject)` — same envelope loop, same
  10 s per-iteration deadline, same `reply_ok`/`Err` reply routing.

Both binaries keep their YAML fixtures, `MARSHAL_STEP_INDEX` /
`USER_STEP_INDEX` constants, `authored_steps` (the 7-step layout is unique
to the marshal-error battery), `replace_user_step`, the `spawn_*` wrappers
that carry each binary's diagnostic label, and every `#[tokio::test]`.

Rationale for the tuple rather than a hidden global index: the injection
point is a property of the battery's compiled layout, not of the shared
rig. Passing it explicitly keeps the shared helper layout-agnostic.

### do-try spec scenarios (rc-ud3rl)

The canonical spec file is `openspec/specs/do-try/spec.md`. The delta
reproduces the whole `A Handled or Continued catch exits with the error
cleared` requirement under `## MODIFIED Requirements`, so the sync keeps
all five existing scenarios (Handled, Continued, finally-after-handled,
Propagate, route Unit of Work) and adds:

- a nested `Handled` doTry inside an outer `Continued` catch body survives
  that outer catch's error clearing;
- a marker set in a try body that later fails does not reach the catch
  chain (the pre-try snapshot wins);
- steps after a `Handled` or `Continued` exit cannot rely on
  `CamelExceptionCaught`.

The current sentence "The retention of `CamelExceptionCaught` after a
handled catch is a separate decision and SHALL NOT be settled by this
requirement" becomes normative: `clear_error()` removes the caught
property, so it is absent after a handled catch, and downstream steps SHALL
NOT rely on it. This matches ADR-0025's exception-retention amendment and
the `DoTrySegment` implementation (`handle_error` on `Handled`,
`clear_error` on `Continued`, stale-marker removal on entry).

ADR-0025's amendment sentence that defers the scenarios to rc-ud3rl is
replaced with a pointer to the now-present spec scenarios. Three test-only
regressions in `do_try_segment.rs` (a nested `Handled` doTry in an outer
`Continued` catch body, a `MarkThenFail` try-body stub, and a
`Handled`/`Continued` disposition loop) give the new normative scenarios
direct evidence. No production code changes.

## Affected crates

- `camel-dsl` (tests only): `tests/common/mod.rs`,
  `tests/rest_xml_http_e2e.rs`, `tests/rest_marshal_error_http_e2e.rs`.
- `camel-processor` (tests only): three `#[tokio::test]` cases in
  `src/do_try_segment.rs` pinning the nested-marker, pre-try-snapshot, and
  caught-property scenarios. No production line changes.
- `openspec/specs/do-try/spec.md` via the change delta; `docs/adr/0025-*`
  prose.

## Architecture boundaries

Tests and documentation only. The data/control plane split (ADR-0001) and
the structural-EIP outcome contract (ADR-0025) are untouched. The shared
rig still drives the real compiled `BuilderStep`s, so it does not move
production logic into tests.

## Alternatives considered

- Dedicated `tests/support/` module: rejected. The repo precedent is
  `tests/common/mod.rs` (`camel-integration-test` and the sibling REST e2e
  binaries), so a second convention would split the rig.
- A global `MARSHAL_STEP_INDEX` inside the shared rig: rejected. It would
  bake one battery's layout into a helper shared by others.
- Separate ADDED requirements for the three new scenarios: rejected. The
  scenarios describe the same requirement (error clearing and marker
  state), and a fragmented requirement set is harder to keep coherent.

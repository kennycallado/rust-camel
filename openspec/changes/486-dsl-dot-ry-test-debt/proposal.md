# Proposal: 486-dsl-dot-ry-test-debt

## Why

Two P3 items remain from missions 483 and 485. Neither changes behavior.

- **rc-v4u92** — `crates/camel-dsl/tests/rest_xml_http_e2e.rs` and
  `rest_marshal_error_http_e2e.rs` still carry two verbatim copies of the
  compiled-step drive rig: route lookup + six-step layout pin,
  `compile_step_for_drive`, `drive_steps`, and `serve_pipeline` (~86 lines
  each). The earlier extraction (`tests/common/mod.rs`) covered the HTTP
  client and server helpers but left this step-drive family duplicated. A
  third copy will diverge.
- **rc-ud3rl** — ADR-0025 now records the canonical
  `CamelExceptionHandled` rule (mission 485). The do-try spec covers only
  two of the four rule cases: `Handled` sets the marker and `Continued`
  does not. Marker survival across a nested `Handled` doTry inside an outer
  `Continued` catch body, and the ban on relying on `CamelExceptionCaught`
  after the block, are recorded only in the ADR.

## What Changes

Test support and normative spec text only.

- `tests/common/mod.rs` gains the shared compiled-step drive rig:
  six-step layout pin, `compile_step_for_drive`, `drive_steps`, and
  `serve_pipeline`, with an optional `(index, mutator)` injection point.
- Both e2e binaries drop their copies and call the shared rig. Every test
  name and assertion stays.
- `openspec/changes/486-dsl-dot-ry-test-debt/specs/do-try/spec.md`
  (MODIFIED `A Handled or Continued catch exits with the error cleared`)
  makes the caught-property decision normative and adds the nested-marker
  and caught-property scenarios.
- ADR-0025 drops the "scenarios deferred to rc-ud3rl" sentence and points
  at the spec scenarios that now exist.
- Three test-only regressions pin the new scenarios at the do-try level in
  `crates/camel-processor/src/do_try_segment.rs`: nested `Handled` marker
  survival inside an outer `Continued` catch body, and
  `CamelExceptionCaught` absence after a `Handled` or `Continued` exit.

Explicitly excluded: production code, doTry semantics, new tests outside
the two tickets, a new crate, and any change to the HTTP client/server
helpers in `tests/common/mod.rs`.

## Acceptance criteria

- `cargo test -p camel-dsl --test rest_xml_http_e2e --test
  rest_marshal_error_http_e2e` stays green at 9 tests (2 + 7), with the
  same test names.
- No duplicate copy of `compile_step_for_drive`, `drive_steps`, or
  `serve_pipeline` remains in the two binaries.
- `openspec validate --all --strict` passes.
- `cd docs && mdbook test` passes.
- `cargo fmt --check`, `cargo clippy -p camel-dsl --all-targets -- -D
  warnings`, and the 24 xtask lints pass.

## Risk budget

Acceptable: test-support churn confined to `crates/camel-dsl/tests/`.
Out of bounds: any runtime change, any weakened assertion, any renamed
test, and any production-code edit under `crates/` (the only `src/` change
permitted is the `#[cfg(test)]` block in `camel-processor`).

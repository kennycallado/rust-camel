# Proposal: error-handler-use-original-message

## Why

A demo team (bd `rc-bx0qx`, P1) reports that in a YAML route,
`error_handler.use_original_message: true` has no effect: the dead-letter
channel receives the mutated exchange. The field parses and validates
(`yaml.rs:496` copies it into `DeclarativeErrorHandler`,
`model.rs:236`, `route_ast.rs:397`), but `compile_error_handler`
(`crates/camel-dsl/src/compile.rs:764-892`) builds `ErrorHandlerConfig`
without ever reading `def.use_original_message`, so the config keeps its
`false` default (`camel-api/src/error_handler.rs:229-247`). Downstream the
wiring is correct: `route_compiler_ext.rs:109` and `:257` pass the flag to
`DefaultRouteErrorHandler` and `RouteChannelService`, and
`route_compiler.rs:937` stashes the original message only when the flag is
true — with `false` no snapshot exists, so restoration is impossible. The
docs (`docs/src/yaml-dsl/route-level-config.md`) and both schemas advertise
the field as working. The defect is compiler wiring, not processor logic.

## What Changes

- **Included:** one production line at the END of `compile_error_handler`
  (`config.use_original_message = def.use_original_message;` before
  `Ok(config)`); tests-first coverage for the three config shapes
  (DLC only, retry only, `on_exceptions`), the default-false case, a
  YAML-text → compiled-config test (the missing link between `yaml.rs` and
  the compiler), and an end-to-end camel-cli test-runner test proving the
  DLC receives the ORIGINAL body (plus a no-flag control asserting the DLC
  receives the mutated body); one ADDED-only spec delta under
  `openspec/changes/error-handler-use-original-message/specs/error-handler/`.
- **Excluded:** no new setter or API addition (the config field is already
  public), no refactor of `compile_error_handler`, no changes to
  `camel-processor`, `camel-core`, `route_compiler`, docs, schemas, and no
  lint rule (rationale in `design.md`).

## Acceptance criteria

- `compile_error_handler` copies `def.use_original_message` for all three
  shapes; the default stays `false` when the flag is unset.
- A YAML string → `parse_yaml_to_declarative` → `compile_declarative_route`
  test asserts the flag on `error_handler_config()`.
- The report's exact scenario passes end-to-end: with input
  `ORIGINAL-EVENT` and `set_body: MUTATED`, `mock:dlc` receives
  `ORIGINAL-EVENT`; the no-flag control receives `MUTATED`.
- New tests are red before the fix and green after.
- `cargo test -p camel-dsl`, `cargo test -p camel-cli --test test_runner`,
  `cargo fmt --check --all`, `bash scripts/gates-local.sh` exit 0.

## Risk budget

Minimal: the added line is branch-proof (`ExceptionPolicyBuilder::build()`
preserves the config across every policy branch, so a trailing assignment
covers all shapes). Reading the `Copy`
`bool` field after the partial moves of `def.on_exceptions`/`def.retry` is
valid — proven by compilation. Behavior is opt-in; unset-flag routes are
unchanged. Out of bounds: any behavior change when the flag is absent, any
processor/runtime change, any schema or docs edit.

Affected crates: `camel-dsl` (fix + unit/integration tests), `camel-cli`
(end-to-end test only). bd: `rc-bx0qx`.

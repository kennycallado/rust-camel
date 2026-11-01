# Design: 487-template-test-debt

## Approach

Four independent, ordered edits in `camel-template` plus one docs file. No
production item changes value: the only `src/` edits are `#[cfg(test)]` code
and module declarations.

### 1. Test-module split (`rc-1shz2`)

`template_set.rs` already ends with one `#[cfg(test)] mod tests { ... }`
block owning 28 tests and the private helpers they share
(`snapshot`, `ctx_name`, and the test-only imports). Rust 2018 path rules
map `mod tests;` inside `src/template_set.rs` to
`src/template_set/tests.rs`, so the body moves verbatim, including
`use super::*;`, and the child keeps access to every private parent item.

`template_set` is declared `#[cfg(feature = "default")] pub(crate) mod
template_set;` in `lib.rs`. A child module only compiles when its parent
does, so the child needs `#[cfg(test)]` only; adding a second feature gate
would be redundant. This form is exactly what the ticket requested.

### 2. Deterministic dump fixture (`rc-bsg0z`)

The test stays one test with one assertion pair. Change the object fixture
to `json!({"b": 1, "a": 2})` and the expected render to
`{"a":2,"b":1}|[1,2,3]|"Ada"`. The second render already asserts
byte-identical output; it stays.

Evidence note for the review: the workspace enables minijinja without
`preserve_order`, so its `ValueMap` is a `BTreeMap` and sorting happens on
insertion as well. The fixture pins the *contract* for unsorted input; it
does not prove `dump_filter` itself resorts. Commit and docs wording must
not overclaim.

### 3. Tracing-capture extraction (`rc-qkxbm`)

Move `ensure_global_tracing_default`, `FieldCollector`, `CaptureLayer`, and
`capture_warns` out of `producer::tests` into a new crate-level
`src/test_support.rs`:

```rust
#[cfg(all(test, feature = "default"))]
pub(crate) mod test_support;
```

declared in `lib.rs` next to `template_set`. This follows the documented
natural-home rule in `docs/testing/tracing-capture-guards.md` (camel-dsl
`src/lib.rs` `test_support` is the reference). Only `capture_warns` becomes
`pub(crate)`; the rest stays private to the module. `feature = "default"`
is required, otherwise a `--no-default-features` test build has no caller
and `dead_code` fails `-D warnings`.

`producer::tests` imports `crate::test_support::capture_warns` and drops
every import that only the moved helpers used. Test bodies and names stay
untouched.

### 4. Guard inventory refresh (`rc-zc6gc`)

The doc's 22-site snapshot is stale. A repo-wide
`grep -rn "set_global_default(tracing_subscriber::registry())" --include=*.rs
crates/` returns 30 guard bodies in 28 files (`camel-ws/src/lib.rs` has 3
inline bodies). Bumping 22 -> 23 would be knowingly false and a guaranteed
review finding, so the doc gets an accurate re-inventory in its existing
three-group shape:

- Natural homes gain `camel-template/src/test_support.rs` and
  `camel-processor/src/test_log_capture.rs` (the shared module that
  replaced the removed `error_handler.rs` copy).
- New per-file sites: `camel-jms/src/component.rs`,
  `camel-jms/src/consumer.rs`, `camel-core/.../route_compiler_span_tests.rs`,
  `camel-core/.../context_lifecycle.rs`,
  `camel-cxf/tests/consumer_profile_test.rs`,
  `services/camel-otel/src/service_tests.rs`.
- `camel-ws/src/lib.rs` is annotated "(3 bodies)".
- The stale `camel-processor/src/error_handler.rs` entry is removed.
- The "Not this pattern" paragraph gains
  `camel-rabbitmq/tests/common/mod.rs` (recording subscriber installed with
  `expect`, like the mcp module).
- The dated count note is replaced by the grep command and today's date.

## Affected crates

- `camel-template`: module split, fixture assertion, test-support move.
- None else (`docs/testing/` is repo-level prose).

## Architecture boundaries

No boundary moves. The crate's `feature = "default"` engine gate is
preserved: `test_support` and the split `template_set::tests` only compile
with the engine feature on and `cfg(test)` active. No public API, no
dependency, no Runtime/DSL/Components/Services/Languages/Functions surface
change.

## Alternatives considered

- `src/producer/test_support.rs` (child of producer) for the extraction:
  rejected by the pre-flight oracle — it would be another per-file copy,
  while the crate-level module follows the documented natural-home rule
  and can serve future reload/lifecycle capture tests.
- Adding `#[cfg(feature = "default")]` to the split `tests` module:
  rejected as redundant; the parent gate already covers the child.
- Docs minimal bump 22 -> 23: rejected; the actual count is 30 and the doc
  is the inventory's source of truth.
- Keeping `dump` fixture keys alphabetical: rejected; it cannot fail if a
  future change regresses to insertion order.

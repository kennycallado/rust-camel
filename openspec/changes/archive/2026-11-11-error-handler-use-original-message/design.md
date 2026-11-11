# Design: error-handler-use-original-message

## Approach

One production line at the END of `compile_error_handler`
(`crates/camel-dsl/src/compile.rs:764-892`), immediately before
`Ok(config)`:

```rust
// Copy the declarative flag once after all policy branches.
config.use_original_message = def.use_original_message;
Ok(config)
```

Tests are written FIRST and shown red before the fix (TDD), then the fix
turns them green.

### The chain (verified on `main` 68297e6f2 by code reading)

1. `yaml.rs:496` copies `eh.use_original_message` into
   `DeclarativeErrorHandler` (`model.rs:236`, mirrored in
   `route_ast.rs:397`). The field parses and passes validation.
2. `compile_error_handler` (compile.rs:764-892) builds `ErrorHandlerConfig`
   from the DLC branch (`:765`), rebuilds it per `on_exceptions` clause
   (`config = builder.build()` at `:877`) and in the retry branch (`:889`)
   — but never reads `def.use_original_message`. The config keeps its
   `false` default (`camel-api/src/error_handler.rs:229-247`).
3. `route_compiler_ext.rs:109` passes the flag to
   `DefaultRouteErrorHandler`; `:257` passes it to `RouteChannelService`;
   `route_compiler.rs:937` stashes the original message only when true.
   With `false`, no snapshot exists and restoration is impossible.

### Why the flag was dropped

The field was added to `DeclarativeErrorHandler` and the YAML mapper when
the schema was written, but `compile_error_handler` was never updated to
carry it into `ErrorHandlerConfig`. Everything downstream of the config
(flag → handler → channel service → snapshot stash) is implemented and
already tested at the `camel-processor` level, where tests set the field
by hand. The wiring gap sits exactly at the AST → config boundary, and no
test crosses it: every `camel-dsl` test builds the AST with
`use_original_message: false`.

### Why the assignment goes last

The three branches above the insertion point reassign the binding
`config`: `on_exceptions` ends with `config = builder.build()` (`:877`)
and `retry` with `config = builder.build()` (`:889`).
`ExceptionPolicyBuilder::build()` appends the policy and returns the
existing config unchanged otherwise (`error_handler.rs:334-337`), so the
config — including this flag — is preserved across every branch. Policy
builders preserve the configuration, including this flag. A trailing
assignment maps the flag once at the common successful exit and covers
all three shapes (DLC-only, retry-only, `on_exceptions`) with one line.

**Partial moves:** `def.on_exceptions` and `def.retry` are partially moved
earlier in the function; reading `def.use_original_message` afterwards is
valid because the field is a `bool` (`Copy`), so field access re-borrows
the unmoved field — this is proven by compilation of the fix itself.

### Why no lint rule and why schema cannot catch it

The defect is compiler wiring: the schema already declares
`use_original_message` in both `schemas/dsl/route-schema.json` and
`crates/camel-lint/schema/route-schema.json`, and the document parses and
validates cleanly — a document lint validates the YAML against the schema
and can never observe that a compiled `ErrorHandlerConfig` lost a field
the AST carries. There is nothing for a lint to reject: the route is
schema-valid. The correct guard is the new compiler test that crosses the
YAML → compiled-config boundary; it fails the moment someone drops the
field again.

### Other parsed-and-dropped fields

`DeclarativeErrorHandler` (`model.rs:232-238`) has exactly four fields;
`compile_error_handler` reads `dead_letter_channel` (`:765`),
`on_exceptions` (`:771`) and `retry` (`:879`). `use_original_message` is
the only field parsed and dropped — no other dropped field was found in
this struct (noted for the park report; nothing to fix here).

## Affected crates

- `camel-dsl`: the one-line fix in `compile.rs`; new unit tests in
  `compile.rs` (modeled on existing tests near lines 2424, 2811, 3144);
  new YAML → config test in
  `tests/declarative_compile_test.rs` (`parse_yaml_to_declarative` →
  `compile_declarative_route` → `definition.error_handler_config()`,
  both re-exported from `camel_dsl`).
- `camel-cli`: two behavior tests in `tests/test_runner.rs` using the
  inline document harness (`inputs` + `expects`, modeled on
  `route_files_resolve_relative_to_doc_dir`; `expects` supports
  `bodies`). One with the flag asserting the DLC receives
  `ORIGINAL-EVENT`, one control without the flag asserting the DLC
  receives `MUTATED`. `camel-cli` production code is untouched.

## Architecture boundaries

DSL-layer only. The fix sits inside the DSL compiler
(`compile_error_handler`) mapping an AST field onto the existing public
`ErrorHandlerConfig` field; the data/control plane split, the processor
restoration logic, and `route_compiler` are untouched. `camel-core`'s
`RouteDefinition::error_handler_config()` accessor is read-only test
surface. Respects the runtime/DSL boundary per CONTEXT-MAP.md: the DSL
compiles declarative routes into core definitions; this change completes
that mapping.

## Alternatives considered

- **Assign inside each branch** — rejected: three edits, easy to miss the
  next branch; the trailing single assignment is branch-proof and matches
  the order's constraint of one production line.
- **Carry the flag through `ExceptionPolicyBuilder`** — rejected: new
  builder API surface for a field that `build()` already preserves; the
  order forbids API additions.
- **Fix in `route_compiler_ext`/`DefaultRouteErrorHandler`** — impossible:
  the flag never reaches the config, so downstream layers receive `false`
  and have nothing to wire; the defect is upstream.
- **Lint rule for "field parsed but dropped"** — rejected (see above): a
  document lint sees only the YAML, which is valid; the gap is in Rust
  compiler code no document lint observes. The new boundary test is the
  guard.

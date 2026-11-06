# Shape matrix — minijinja 3.0.5 context/render branches

Task 515.1 output. Persists the branch → owning-test mapping for the context
build (`engine.rs::build_context_bounded`), the `ContextReader` callable
object, the strict-undefined render path, and the three `camel-template`
call sites migrated off the 490-era `Value::from_serialize`.

Status vocabulary:

- `covered` — an existing test asserts the **exact** rendered output or the
  exact error text for this branch.
- `partial` — an existing test asserts only a substring / broad behaviour
  where an exact redacted-diagnostic pin is required. The existing test stays;
  515.2 adds the pin named in column 5.
- `gap` — no test exercises the branch. 515.2 adds the test in column 5.

Column 5 names come from the 515.2 catalog in `tasks.md`; no invented names.

## Engine branches

| Branch / call site (file:line) | Value-construction shape | Owning test (fn + file) | Status | Planned test (515.2 catalog) |
|---|---|---|---|---|
| `crates/languages/camel-language-minijinja/src/engine.rs:386-393` — bytes precheck, early size rejection before the measurement pass | `Err(LanguageError::EvalError("max-context-size {N} bytes exceeded (body bytes alone: {len})"))` when `Body::Bytes` length exceeds `max_context_size` | `bytes_precheck_refusal_text_is_pinned` (`crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs`) | `covered` | none (pinned in 515.2) |
| `crates/languages/camel-language-minijinja/src/engine.rs:408-412` — S9 bounded-context measurement failure (`serde_json::to_writer` into `LimitedWriter`, S9) | `Err(LanguageError::EvalError("max-context-size {N} bytes exceeded"))` | `max_context_size_text_is_pinned` (`crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs`) | `covered` | none (pinned in 515.2) |
| `crates/languages/camel-language-minijinja/src/engine.rs:430-433` — body is `Body::Stream(_)` | `minijinja::Value::from(minijinja::Error::new(InvalidOperation, STREAM_REFUSAL))` (invalid value, refuses only when consumed) | `stream_body_refusal_text_is_pinned` (`crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs`) | `covered` | none (pinned in 515.2) |
| `crates/languages/camel-language-minijinja/src/engine.rs:434` — body non-stream | `minijinja::Value::from(minijinja::value::Serde(&body_view))` (`BodyAsJson` projection) | `producer_replaces_body_on_success` (`crates/components/camel-template/src/producer.rs:251`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:439` — `headers` map | `minijinja::Value::from(minijinja::value::Serde(&exchange.input.headers))` | `header_only_template_passes_stream_body` (`crates/languages/camel-language-minijinja/src/lib.rs:654`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:443` — `exchangeProperty` map | `minijinja::Value::from(minijinja::value::Serde(&exchange.properties))` | `producer_exposes_exchange_property_key` (`crates/components/camel-template/src/producer.rs:715`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:450` — outer context map | `minijinja::Value::from(minijinja::value::Serde(&map))` over `BTreeMap<String, minijinja::Value>` | `compiles_template_once_across_evaluations` (`crates/languages/camel-language-minijinja/src/lib.rs:96`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:497` — `header(key)` hit, `ReaderKind::Header` | `minijinja::Value::from(minijinja::value::Serde(v))` | `header_and_property_readers` (`crates/languages/camel-language-minijinja/src/lib.rs:498`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:497` — `property(key)` hit, `ReaderKind::Property` | `minijinja::Value::from(minijinja::value::Serde(v))` | `header_and_property_readers` (`crates/languages/camel-language-minijinja/src/lib.rs:498`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:498` — `header(key)` miss, `ReaderKind::Header` | `minijinja::Value::from(())` (nil, renders empty) | `header_and_property_readers` (`crates/languages/camel-language-minijinja/src/lib.rs:498`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:498` — `property(key)` miss, `ReaderKind::Property` | `minijinja::Value::from(())` (nil, renders empty) | `header_and_property_readers` (`crates/languages/camel-language-minijinja/src/lib.rs:498`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:493-495` — `has_header(key)`, `ReaderKind::HasHeader` | `minijinja::Value::from(map.contains_key(key))` (bool) | `has_header_absent_vs_null` (`crates/languages/camel-language-minijinja/src/lib.rs:476`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:493-495` — `has_property(key)`, `ReaderKind::HasProperty` | `minijinja::Value::from(map.contains_key(key))` (bool) | `has_property_absent_vs_null` (`crates/languages/camel-language-minijinja/src/lib.rs:487`) | `covered` | none (already covered) |
| `crates/languages/camel-language-minijinja/src/engine.rs:484-486` — `ContextReader::call` argument-count error, `args.len() != 1` | `Err(minijinja::Error::new(InvalidOperation, "reader expects one string argument"))` | `reader_zero_args_text_is_pinned` (`crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs`) | `covered` | none (pinned in 515.2) |
| `crates/languages/camel-language-minijinja/src/engine.rs:487` — `ContextReader::call` non-string argument error (`args[0].as_str()` is `None`) | `Err(minijinja::Error::new(InvalidOperation, "reader expects one string argument"))` | `reader_non_string_arg_text_is_pinned` (`crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs`) | `covered` | none (pinned in 515.2) |
| `crates/languages/camel-language-minijinja/src/engine.rs:166` (`UndefinedBehavior::Strict`) + `engine.rs:269-298` (`map_render_error`) — strict-undefined interpolation | render error → redacted `LanguageError::EvalFailure { class: Runtime, position, detail: Some(kind) }` | `strict_undefined_interpolation_text_is_pinned` (`crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs`) | `covered` | none (pinned in 515.2) |
| `crates/languages/camel-language-minijinja/src/engine.rs:166` (`UndefinedBehavior::Strict`) + `engine.rs:269-298` (`map_render_error`) — strict-undefined conditional | render error → redacted `LanguageError::EvalFailure { class: Runtime, position, detail: Some(kind) }` | `strict_undefined_conditional_text_is_pinned` (`crates/languages/camel-language-minijinja/tests/exact_diagnostics.rs`) | `covered` | none (pinned in 515.2) |

## Migrated call sites (`camel-template`)

Each of these previously routed through `Value::from_serialize`; they now build
the context with `minijinja::value::Serde`, the 3.0-compatible construction.

| Branch / call site (file:line) | Value-construction shape | Owning test (fn + file) | Status | Planned test (515.2 catalog) |
|---|---|---|---|---|
| `crates/components/camel-template/src/lifecycle.rs:184-188` — `ctx_name(k)` test helper | `minijinja::Value::from(minijinja::value::Serde(&BTreeMap<&str, &str>))` | `startup_build_compiles_and_seeds` (`crates/components/camel-template/src/lifecycle.rs:191`) | `covered` | none (already covered) |
| `crates/components/camel-template/src/reload.rs:300-304` — `ctx_name(k)` test helper | `minijinja::Value::from(minijinja::value::Serde(&BTreeMap<&str, &str>))` | `reload_commit_swaps_on_valid_change` (`crates/components/camel-template/src/reload.rs:534`) | `covered` | none (already covered) |
| `crates/components/camel-template/src/template_set/tests.rs:14-18` — `ctx_name(k)` test helper | `minijinja::Value::from(minijinja::value::Serde(&BTreeMap<&str, &str>))` | `template_set_compile_and_render` (`crates/components/camel-template/src/template_set/tests.rs:26`) | `covered` | none (already covered) |
| `crates/components/camel-template/src/template_set/tests.rs:21-23` — `ctx_empty()` test helper | `minijinja::Value::from(minijinja::value::Serde(BTreeMap::<&str, &str>::new()))` | `template_set_compile_strict_undefined` (`crates/components/camel-template/src/template_set/tests.rs:41`) | `covered` | none (already covered) |

## Notes

- The two oversize returns are distinct rows: `engine.rs:386-393` is the
  `Body::Bytes` precheck that fires before any measurement allocation;
  `engine.rs:408-412` is the S9 `LimitedWriter` measurement failure. The
  existing `oversize_context_trips_before_full_allocation` uses a `Body::Text`
  payload (`Body::from("x".repeat(1024))`) with `max_context_size: 8`, so it
  exercises the measurement return, not the precheck.
- Strict-undefined rows are two distinct render paths (interpolation vs
  conditional) even though both surface through `map_render_error`; the 515.2
  catalog pins them separately.
- `producer_replaces_body_on_success` and
  `producer_exposes_exchange_property_key` are component-level tests, but both
  call the shared `build_context_bounded` (`producer.rs:138`), so they cover
  the engine's `body` / `exchangeProperty` Serde constructions and assert the
  rendered string exactly.
- `reader_zero_args_text_is_pinned` must assert the redacted public display
  only: `map_render_error` discards the upstream
  `reader expects one string argument` detail (kind + line survive).

## Migration completeness

Command (run in `/home/shared/rust-camel-worktrees/depsmajors`):

```
$ grep -rn "from_serialize" crates/ --include='*.rs'
(no output)
```

Result: **zero hits** (grep exit status 1 = no match). No `Value::from_serialize`
call remains in `crates/`; every construction site uses
`minijinja::value::Serde` or a plain `Value::from`.

Row count: 21 rows total — 17 engine branches (the two oversize returns
counted separately) + 4 migrated `camel-template` call sites.

Note: row 51's owning test `template_set_compile_strict_undefined` asserts
the render-variant display prefix (`starts_with("template render failed:")`)
— a stable API surface — rather than exact text. The mapped contract is the
`ctx_empty()` Serde construction call site (construction coverage via the
attempted render); the strict-undefined diagnostic itself is separately
pinned `covered` by `tests/exact_diagnostics.rs` (515.2).

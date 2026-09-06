# Design: dsl-parity-json-valid

## Root Cause

`fuzz/src/lib.rs::dsl_parity_harness` uses `serde_json` as ground truth and
panics in `panic_if_yaml_rejects` when `noyalib::compat::serde_yaml` rejects
the same bytes. JSON strings accept any unescaped Unicode scalar >= U+0020,
including DEL (U+007F). YAML forbids non-printable characters anywhere in the
stream. A raw DEL byte therefore produces a spec-correct YAML rejection that
the oracle misreads as a parity divergence.

Production behavior is already correct and stays unchanged:

- `crates/camel-dsl/src/json.rs` (serde_json) accepts the document.
- `crates/camel-dsl/src/yaml.rs` (serde_yml) rejects it;
  `input_format::annotate_format` prefixes the error with
  `YAML DSL error:`.

References: ADR-0026 (JSON canonical authoring format), ADR-0017 (DSL key
naming). Neither ADR promises raw-byte JSON/YAML interchangeability; parity
covers the shared `RouteDslRoutes` AST and step lowering for documents both
formats accept.

## YAML Non-Printable Set

One classifier in `crates/camel-dsl/src/yaml.rs`, `pub` so `camel-fuzz` reuses
it (fuzz already has a path dependency on camel-dsl):

```rust
pub fn yaml_stream_has_non_printable(s: &str) -> bool
```

Prohibited classes (YAML 1.2 c-printable, restricted to classes a JSON string
can still carry):

- U+0000..U+0008, U+000B, U+000C, U+000E..U+001F (C0 minus TAB/LF/CR)
- U+007F (DEL)
- U+0080..U+0084, U+0086..U+009F (C1 minus NEL U+0085, which is
  YAML-printable)
- U+FFFE, U+FFFF

All other characters are printable per YAML and pass through (NEL U+0085 is
allowed — YAML treats it as a line break). Escaped forms (`\u007f` as six
ASCII bytes) do not trip the classifier and stay under strict parity.

## Oracle Change (fuzz/src/lib.rs)

`expect_yaml_overlap` gains one carve-out branch:

1. If `camel_dsl::yaml::yaml_stream_has_non_printable(s)`: assert
   `noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(s)` is `Err`, then
   return (no step-layer comparison is possible without a YAML value).
2. Else: the existing strict path is unchanged — `panic_if_yaml_rejects`
   panics on rejection, `assert_step_layer_parity` runs on success.

## Alternatives Rejected

- Sanitizing the YAML input before parse: silently rewrites user bytes.
- Bumping `noyalib` in hope of acceptance: the rejection is spec-correct.
- Removing the oracle: loses the step-layer parity signal for every printable
  document.

## Tests

- `fuzz/src/lib.rs` unit tests: harness on the minimized document does not
  panic; the carve-out branch asserts `Err` on a DEL document; a printable
  escaped-`\u007f` document still goes through full strict parity.
- `crates/camel-dsl/tests/json_yaml_non_printable_parity.rs` (new): the exact
  minimized document from run 33984285881 as a string literal;
  `parse_json_to_declarative` Ok; `parse_yaml_to_declarative` Err with
  `YAML DSL error:` prefix.
- `crates/camel-dsl/src/yaml.rs` module tests: one case per prohibited class,
  plus allowed TAB/LF/CR, allowed NEL (U+0085), and the escaped form.
- `fuzz/src/lib.rs` keeps `panic_if_yaml_rejects` panic-coverage as a unit
  test (assert `Err` input panics with the divergence message): this is the
  coverage for the spec scenario "printable rejection still panics" — no real
  rejected-printable fixture can be assumed to exist.

## Build Commands

- camel-dsl: standard workspace commands (`cargo test -p camel-dsl` from the
  worktree root).
- camel-fuzz: excluded from the root workspace; run
  `cargo test --manifest-path fuzz/Cargo.toml --lib` from the worktree root.

Single phase, three tasks: classifier, oracle, regression promotion.

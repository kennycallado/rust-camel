# Tasks: dsl-parity-json-valid

Single phase. Tasks run in order 1 → 2 → 3. Worktree root for all commands:
`/home/shared/rust-camel-worktrees/dsl-parity-json-valid`.

Shared test fixture (exact minimized document from assurance run 33984285881,
artifact `minimized-from-acfd0c31…`, 93 bytes, verified valid JSON against
the on-disk artifact in /tmp/cal-art — one raw DEL U+007F inside the `to`
URI; Rust literal, byte-exact):

```rust
const MINIMIZED_DEL_DOC: &str =
    "{\"routes\":[{\"id\":\"r1\",\"from\":\"dtart\",\"steps\":[{\"to\":\"di*rect:ewwwwwwwwwwwwww\x7fwwwwwwwwnd\"}]}]}";
```

Dedup note (checked, per bd rc-crpk triage): fix 83b54efa rejects
seq-shaped (`[]`) route docs; this document is mapping-shaped and the
assurance run 33984285881 executed at 83b54efa — the finding is NOT covered
by that fix. A second minimization exists on disk
(`minimized-from-0c1bd4c1…`, 100 bytes, 14×w + DEL + 15×w) — same class;
the 93-byte one is canonical here.

## Task 1: YAML non-printable classifier in camel-dsl

Files:
- `crates/camel-dsl/src/yaml.rs` (modified)

Steps:
1. In `crates/camel-dsl/src/yaml.rs`, add module tests (in the existing
   `#[cfg(test)]` module, or a new one at file end) for a not-yet-existing
   `pub fn yaml_stream_has_non_printable(s: &str) -> bool`. Run
   `cargo test -p camel-dsl --lib yaml_stream` — expect compile failure
   (function missing).
2. Implement `pub fn yaml_stream_has_non_printable(s: &str) -> bool` in
   `crates/camel-dsl/src/yaml.rs` returning `true` iff `s` contains any char
   in: U+0000..=U+0008, U+000B, U+000C, U+000E..=U+001F, U+007F,
   U+0080..=U+0084, U+0086..=U+009F, U+FFFE, U+FFFF. Everything else
   (including TAB U+0009, LF U+000A, CR U+000D, NEL U+0085) returns `false`.
   Add a `///` doc comment stating the set is the YAML 1.2 c-printable
   prohibited classes and that escaped forms (`\u007f` as six ASCII bytes)
   do not trip it.
3. Run `cargo test -p camel-dsl --lib yaml_stream` — all green.

Tests:
- name: `yaml_stream_has_non_printable_rejects_each_prohibited_class`
  setup: classifier fn exists
  action: call with one string per class — `"\u{0}"`, `"\u{8}"`, `"\u{b}"`,
  `"\u{c}"`, `"\u{e}"`, `"\u{1f}"`, `"\u{7f}"`, `"\u{80}"`, `"\u{84}"`,
  `"\u{86}"`, `"\u{9f}"`, `"\u{fffe}"`, `"\u{ffff}"`
  assert: every call returns `true`
  command: `cargo test -p camel-dsl --lib yaml_stream`
  expected: fails at step 1 (no fn), passes after step 2
- name: `yaml_stream_has_non_printable_allows_printable_and_line_breaks`
  setup: classifier fn exists
  action: call with `""`, `"a\tb"`, `"a\nb"`, `"a\rb"`, `"a\u{85}b"`,
  `"di*rect:e\\u007fnd"` (six ASCII chars backslash-u-0-0-7-f, no raw DEL)
  assert: every call returns `false`
  command: `cargo test -p camel-dsl --lib yaml_stream`
  expected: fails at step 1, passes after step 2

Acceptance:
- `cargo test -p camel-dsl --lib yaml_stream` exits 0
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-dsl -- -D warnings` exits 0
- `cargo xtask lint-unwrap` exits 0 (no unwrap added)

- [x] 1

## Task 2: dsl_parity oracle carve-out in camel-fuzz

Files:
- `fuzz/src/lib.rs` (modified)

Steps:
1. Add unit tests to the existing `#[cfg(test)] mod tests` in
   `fuzz/src/lib.rs` for the carve-out (see Tests below). Run
   `cargo test --manifest-path fuzz/Cargo.toml --lib` — the
   `harness_del_document_does_not_panic` test fails with panic
   `parity divergence: yaml rejects json-valid document` (red, TDD).
2. Implement the carve-out in `expect_yaml_overlap`: before the existing
   strict path, branch — if
   `camel_dsl::yaml::yaml_stream_has_non_printable(s)` is `true`, call a new
   private `fn expect_expected_rejection(s: &str)` which asserts
   `noyalib::compat::serde_yaml::from_str::<RouteDslRoutes>(s)` is `Err`,
   then return. Strict path stays unchanged: `panic_if_yaml_rejects` +
   `assert_step_layer_parity` keep their exact behavior and signatures.
3. Verify a unit test named `panic_if_yaml_rejects_err_panics` exists
   (asserts `panic_if_yaml_rejects(Err(..))` panics with
   `parity divergence: yaml rejects json-valid document`); add it if
   missing. This is the coverage for spec scenario "printable rejection
   still panics".
4. Run `cargo test --manifest-path fuzz/Cargo.toml --lib` — all green.

Tests:
- name: `harness_del_document_does_not_panic`
  setup: `MINIMIZED_DEL_DOC` fixture (shared literal above) as a `const` in
  the test module
  action: `dsl_parity_harness(MINIMIZED_DEL_DOC.as_bytes())`
  assert: returns without panic (carve-out asserted the expected Err)
  command: `cargo test --manifest-path fuzz/Cargo.toml --lib`
  expected: red at step 1 (panics), green after step 2
- name: `escaped_del_document_keeps_strict_parity`
  setup: escaped-wire doc (98 wire bytes; the 93-byte fixture with the raw
  DEL byte replaced by the six ASCII bytes backslash-u-0-0-7-f; in Rust
  source the backslash is doubled):
  `"{\"routes\":[{\"id\":\"r1\",\"from\":\"dtart\",\"steps\":[{\"to\":\"di*rect:ewwwwwwwwwwwwww\\u007fwwwwwwwwnd\"}]}]}"`
  action: `dsl_parity_harness(doc.as_bytes())`
  assert: returns without panic through the STRICT path (both front-ends
  accepted; step parity compared)
  command: `cargo test --manifest-path fuzz/Cargo.toml --lib`
  expected: green before and after (regression guard)
- name: `panic_if_yaml_rejects_err_panics`
  setup: `panic_if_yaml_rejects` exists with its current signature; helper
  `mk_err()` local to the test
  action: `std::panic::catch_unwind(|| panic_if_yaml_rejects(Err(mk_err())))`
  where `mk_err()` builds a `noyalib::compat::serde_yaml::Error` (e.g. from
  `from_str::<RouteDslRoutes>("{")` — unterminated flow mapping)
  assert: caught, payload string contains
  `parity divergence: yaml rejects json-valid document`
  command: `cargo test --manifest-path fuzz/Cargo.toml --lib`
  expected: green after step 3 (add if missing)

Acceptance:
- `cargo test --manifest-path fuzz/Cargo.toml --lib` exits 0
- `cargo fmt --check --all` exits 0
- `cargo clippy --manifest-path fuzz/Cargo.toml -- -D warnings` exits 0 (if
  the environment cannot build the fuzz crate's dev-deps, report instead of
  skipping)

- [x] 2

## Task 3: production front-end regression in camel-dsl tests

Files:
- `crates/camel-dsl/tests/json_yaml_non_printable_parity.rs` (new)

Steps:
1. Create the test file with the three tests below (TDD: they assert
   current-correct behavior, so they are expected green immediately — their
   purpose is pinning the contract against future regressions; if any is
   red, STOP and report `test-design-gap:` followed by the failing test
   name and the observed failure — do not change production code).
2. Run `cargo test -p camel-dsl --test json_yaml_non_printable_parity` —
   all green.

Tests:
- name: `json_front_end_accepts_raw_del_document`
  setup: `MINIMIZED_DEL_DOC` fixture (shared literal above) as a `const`
  action: `camel_dsl::json::parse_json_to_declarative(MINIMIZED_DEL_DOC)`
  assert: `Ok(..)`; the first route's first step `to` contains the char
  `'\u{7f}'`
  command: `cargo test -p camel-dsl --test json_yaml_non_printable_parity`
  expected: green (pins current behavior). If red because the JSON
  front-end rejects for a non-DEL reason (e.g. URI validation), STOP and
  report — the spec scenario assumption is broken.
- name: `yaml_front_end_rejects_raw_del_with_format_annotation`
  setup: `MINIMIZED_DEL_DOC` fixture as a `const`
  action: `camel_dsl::yaml::parse_yaml_to_declarative(MINIMIZED_DEL_DOC)`
  assert: `Err(CamelError::RouteError(msg))` where `msg` starts with
  `YAML DSL error:`
  command: same as above
  expected: green
- name: `escaped_del_document_parity_both_front_ends`
  setup: escaped-wire doc (98 wire bytes; the 93-byte fixture with the raw
  DEL replaced by six ASCII bytes backslash-u-0-0-7-f; Rust source doubles
  the backslash: `…\\u007f…`)
  action: parse it through both `parse_json_to_declarative` and
  `parse_yaml_to_declarative`
  assert: both `Ok`; the first route's first step `to` is identical in both
  results and contains the char `'\u{7f}'`
  command: same as above
  expected: green

Note on imports: the test file needs `use camel_api::CamelError;` —
`CamelError` is defined in camel-api (camel-dsl does not re-export it);
camel-api is a normal dependency of camel-dsl and thus available to
`tests/` without Cargo.toml changes.

Acceptance:
- `cargo test -p camel-dsl --test json_yaml_non_printable_parity` exits 0
- `cargo test -p camel-dsl --lib` exits 0 (no regression)
- `cargo fmt --check --all` exits 0
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0

- [x] 3

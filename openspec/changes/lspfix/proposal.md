# Proposal: lspfix

bd: rc-6g6g4 (P1) · gh #55 · Mission 312

## Why

`camel lsp` lints every YAML buffer as a route document. The LSP handlers
(`did_open`/`did_change`/`did_save` in `crates/camel-lsp/src/lib.rs`) call
`LintEngine::lint(source)`, which takes raw text with no file context, while
the reserved-suffix rule (ADR-0062: `.test.yaml`/`.test.yml` owned by
`camel test`, `.job.yaml`/`.job.yml` owned by `camel job`) is applied only by
the `camel lint` CLI before invoking the engine. Result: opening or editing a
colocated test/job sidecar in any editor integration surfaces false ERROR
diagnostics (missing `id`/`from`, unexpected `routeFiles`/`inputs`/`expects`)
— exactly the placement ADR-0062 Rule 3 blesses.

## What Changes

- `LintEngine` gains `lint_with_path(source, path: Option<&Path>)`. When the
  path has a reserved suffix, the engine skips all rules and returns exactly
  one Info diagnostic (new code `R-RESERVED`) mirroring the CLI info line.
  The string-only `lint(source)` delegates with `None` — behavior unchanged.
- The suffix predicates (`is_test_document`, `is_job_document`,
  `is_reserved_document`) move to `camel-api` (`reserved_suffix` module);
  `camel_dsl::discovery` re-exports them, keeping the ADR-0062 single-rule
  invariant and every existing consumer path stable.
- `camel-lsp` converts the document URI to a file path and calls
  `lint_with_path` on all three lint sites (didOpen direct, didSave direct,
  didChange through the debounced task). Non-file URIs lint as before.
- The `camel lint` CLI keeps its existing pre-check (it already consumes the
  same predicate); no user-visible CLI change.

## Acceptance criteria

- LSP session test: `didOpen` of a `file://…/hello.test.yaml` URI with
  test-document content publishes zero Error diagnostics.
- Engine unit tests: reserved suffixes `.test.yaml`/`.test.yml`/
  `.job.yaml`/`.job.yml` yield exactly one Info `R-RESERVED` diagnostic and
  zero rule diagnostics; plain `.yaml` paths and `None` lint as today.
- `camel-lint` still depends on neither `camel-core` nor `camel-dsl`
  (hex-arch boundary test unchanged and green).

## Risk budget

Low. Additive engine API; one new `DiagnosticCode` variant (additive to the
stable string contract); predicate move is a re-export — zero behavior drift
for discovery, CLI, and corpus gates. Out of bounds: any change to discovery
skip/error semantics, CLI output, or the debouncer's version-ordering
contract.

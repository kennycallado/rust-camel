# Design: lspfix

## Approach

Root cause: the reserved-suffix rule is applied at the CLI layer, but the LSP
path (`crates/camel-lsp/src/lib.rs` → `LintEngine::lint`) bypasses it because
the engine takes raw text with no file context (gh #55, bd rc-6g6g4).

The skip must live in the engine, not per-caller. But the engine cannot call
`camel_dsl::discovery::is_reserved_document` directly: the hex-arch boundary
test (`camel-core/tests/hexagonal_architecture_boundaries_test.rs:865`) and
route-lint spec both forbid `camel-lint` depending on `camel-dsl`.

Resolution — move the predicate to the one crate both sides already see:

1. **`camel-api`** (dep-clean base crate; `camel-lint` and `camel-dsl` both
   depend on it) gains a `reserved_suffix` module hosting `is_test_document`,
   `is_job_document`, `is_reserved_document` verbatim (pure `&Path` name
   checks, std-only) with unit tests.
2. **`camel-dsl`** `discovery.rs` replaces the definitions with
   `pub use camel_api::reserved_suffix::{…}`. All ten consumer files
   (`camel-cli` run/test/job/lint/corpus, `camel-integration-test`, discovery
   internals) keep the `camel_dsl::discovery::*` path — ADR-0062 Rule 2's
   "single suffix rule, no private copies" invariant is preserved; the
   canonical definition site moves below the hex boundary. The dsl spec
   requirement is modified accordingly (normative home of the rule).
3. **`camel-lint`** `engine.rs`: `lint_with_path(&self, source: &str, path:
   Option<&Path>) -> Vec<Diagnostic>`. Reserved path → early return with one
   `Diagnostic { code: RReserved, severity: Info, span: 0..0, message:
   "skipped: <path> is a reserved document (camel test or camel job)" }`
   (mirrors the CLI info line). Otherwise the rule loop runs exactly as
   today. `lint(source)` becomes `self.lint_with_path(source, None)` —
   byte-for-byte behavior parity for every existing caller. New
   `DiagnosticCode::RReserved` displays `R-RESERVED` (additive to the stable
   string contract; corpus baselines never see it because the CLI pre-check
   never reaches the engine with a reserved path).
4. **`camel-lsp`** `lib.rs`: `did_open` and `did_save` compute
   `uri.to_file_path().ok()` and call `lint_with_path(&raw, path.as_deref())`.
   `debounce.rs`'s spawned task converts `task_uri` the same way before its
   unlocked lint pass. Non-file URIs (`untitled:`, `http:`) yield `None` →
   full lint, unchanged. `Url::to_file_path` handles percent-decoding and
   platform paths. camel-lsp's allowed dep set is untouched (route-lsp spec:
   camel-lint, tower-lsp, tokio only).

CLI `camel lint` keeps its existing predicate pre-check (lint.rs:86) — same
predicate, same output, zero regression surface.

## Affected crates

`camel-api` (new module), `camel-dsl` (re-export), `camel-lint` (engine API +
code), `camel-lsp` (three call sites).

## Architecture boundaries

Respects the hex-arch test: no new dependency edges at all. The rule text
moves DOWN (dsl → api), never up. ADR-0062 semantics (suffix reserved,
wildcard-skip/literal-error in discovery, colocation blessed) unchanged.

## Alternatives considered

- **camel-lint depends on camel-dsl** — rejected: violates the enforced
  runtime-free boundary (hex test, route-lint spec).
- **Re-implement the suffix check inside camel-lint** — rejected: private
  copy, the exact anti-pattern ADR-0062 Rule 2 killed.
- **Apply the check in camel-lsp only** — rejected: rule at a caller again;
  next consumer repeats the bug (bd fix direction says engine).
- **CLI migrates to `lint_with_path`** — deferred: would change CLI info-line
  plumbing (`cli_info` field, exit-code path) with no user-visible gain.

Single phase; three ordered tasks (predicate move → engine API → LSP wiring).

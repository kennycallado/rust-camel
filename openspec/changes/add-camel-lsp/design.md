# Design: add-camel-lsp

## Approach

`camel-lsp` is a **thin protocol adapter**. All diagnostics, completion, and hover
logic lives in `camel-lint` as protocol-agnostic types. The server translates
LSP JSON-RPC messages into `camel-lint` calls and maps the results back to
`lsp_types` (via `tower_lsp::lsp_types`). This keeps the adapter swappable (tower-lsp → any other LSP stack =
rewrite glue, not engine) and testable without an LSP client.

**LSP stack:** tower-lsp (tokio-native, best-documented for framework contributors).
Rejected: `lsp-server` (sync/threaded, idiomatic mismatch with the tokio-first
workspace); `async-lsp` (smaller ecosystem, more moving parts than the payoff
justifies). Protocol types are consumed via `tower_lsp::lsp_types::*` (the version
tower-lsp re-exports), NOT via a separately pinned `lsp-types` crate — a separate
pin risks duplicate incompatible protocol-type versions. Ruling: e_opus
architectural consultation (2026-08-09), recorded in bd rc-o5qz.

**Engine injection:** the CLI's `camel lsp` subcommand calls `production_engine()`
(already in `camel-cli`'s lint module) to build a fully-configured `LintEngine`,
then constructs the LSP service via
`LspService::new(move |client| Backend::new(client, engine))`. tower-lsp calls
this closure with a `Client` on connection — the `Backend` stores it for
`publish_diagnostics`. The `Backend` holds `Arc<LintEngine>` (the engine is NOT
`Clone`; `Arc` makes `Backend: Clone` which tower-lsp requires).

**Document state:** the server holds one `(Document, Option<i32>)` per open file
(the tuple carries the LSP document version) in a
`HashMap<tower_lsp::lsp_types::Url, (Document, Option<i32>)>`, guarded by a
`tokio::sync::RwLock`. On `didOpen` the server calls `Document::parse`. On
`Document::parse`. On `didChange` (range edit) it calls
`Document::apply_edit(start, end, text)`. On every change it runs
`LintEngine::lint(&doc.raw)` and publishes diagnostics. On `didClose` the document
is removed and an empty diagnostics notification is published to clear stale marks.

**Incremental edit:** `Document::apply_edit` is extracted from the existing
`apply_fix` core (which already does "replace_span + re-parse + commit"). The
generalized form takes `(start: usize, end: usize, replacement: &str)` directly,
without requiring a `Fix`. **Unlike `apply_fix`**, `apply_edit` ALWAYS commits
(even when the re-parse produces `parse_failure`) — editors routinely produce
intermediate invalid states, and the server's document must mirror the editor's
text so R-SYN can report the syntax error. `apply_fix` remains transactional
(rolls back on parse_failure) because an automated fix must never break syntax.

**Completion:** `LintEngine::complete_at(&self, doc: &Document, offset: usize) ->
Vec<CompletionItem>`. The engine inspects `doc.route_view` to find which
`Spanned<Endpoint>` (or option key/value) contains the cursor offset. If the cursor
is in the scheme position (before `:`), it offers all catalog scheme names. If in
the query-string option-key position (after `?` or `&`), it offers the scheme's
declared option names/aliases. If in an option-value position, it offers kind-
appropriate defaults (bool → `true`/`false`). Returns an empty list when no
context is determinable (graceful, never panics).

**Hover:** `LintEngine::hover_at(&self, doc: &Document, offset: usize) ->
Option<HoverInfo>`. The engine finds the option key at the cursor and looks up its
metadata (`description`, `deprecated`, `secret`). Returns `None` when no metadata is
available.

## Affected crates

- **`camel-lint`** (modified): add `Document::apply_edit` (always-commit variant),
  `CompletionItem` + `HoverInfo` types, `LintEngine::complete_at`,
  `LintEngine::hover_at`. These are pure additions to the existing public surface
  — `apply_fix` is refactored to delegate to `apply_edit` plus a transactional
  rollback guard.
- **`camel-lsp`** (new): tower-lsp `Backend` struct (holds a `LintEngine`), LSP
  handlers, `tower_lsp::lsp_types` conversion layer, debounced diagnostics
  publisher with version tracking.
- **`camel-cli`** (modified): add `camel lsp` subcommand that calls
  `production_engine()` and boots the server with the resulting `LintEngine`.
- **`hexagonal_architecture_boundaries_test`** (modified): assert `camel-lsp` does
  not depend on `camel-core`, `camel-dsl`, `camel-cli`, or `camel-api`.

## Architecture boundaries

The change respects the Runtime / DSL / Components boundary:

- `camel-lsp` is a **Services/Languages** layer consumer — it provides an editor
  integration service on top of the lint engine, not a runtime component.
- `camel-lsp` depends on `camel-lint` (which itself depends only on `camel-api` +
  `noyalib` + `jsonschema` + `ariadne`). It never touches `camel-core` (route
  lifecycle, data plane) or `camel-dsl` (runtime DSL compilation).
- The production `LintEngine` is built by the CLI's `camel lsp` subcommand via
  `production_engine()` and passed to `camel-lsp::Backend::new(engine)`. The server
  receives a fully-built `LintEngine` (which wraps the catalog internally), not a
  raw catalog trait object — so `camel-lsp` never names `camel-api` types.

Authority: ADR-0041 (component-metadata-capabilities-schema — the `uri_options`
metadata that drives completion and hover). The workspace hexagonal-architecture
boundary test (established for `camel-lint` in Change A) is extended to cover
`camel-lsp`.

## Phases

### Phase 1: camel-lsp scaffold + Document::apply_edit

- **Goal:** crate compiles, `camel lsp` boots a stdio server (initialize handshake
  only), `Document::apply_edit` extracted from `apply_fix`.
- **Dependencies:** `camel-lint` (merged), tower-lsp + lsp-types workspace pins.
- **Externally-visible types/interfaces:** `Document::apply_edit`, `camel-lsp::Backend`,
  `camel lsp` clap subcommand.
- **Deliverable:** commit with crate scaffold + apply_edit + smoke test (initialize
  → shutdown handshake over stdin/stdout).
- **Exit-criteria:** `cargo build -p camel-lsp` succeeds; hex-arch boundary test
  extended; `Document::apply_edit` unit tests pass (identical behavior to `apply_fix`
  for the same edit).

### Phase 2: Diagnostics pipeline (didOpen/didChange/didSave → publishDiagnostics)

- **Goal:** editor sees live diagnostics on open, change, and save.
- **Dependencies:** Phase 1 (apply_edit for incremental didChange).
- **Externally-visible types/interfaces:** LSP textDocument/didOpen, didChange,
  didSave, publishDiagnostics handlers.
- **Deliverable:** commit with document-state map + diagnostics pipeline + debounce.
- **Exit-criteria:** integration test simulates an editor session (open → change →
  save) and asserts diagnostics are published at each step; partial-input (truncated
  YAML) publishes syntax diagnostics without panic.

### Phase 3: Completion

- **Goal:** editor offers completion candidates (schemes, option keys, values).
- **Dependencies:** Phase 2 (document state management exists).
- **Externally-visible types/interfaces:** `LintEngine::complete_at`, LSP
  textDocument/completion handler, `CompletionItem` type in camel-lint.
- **Deliverable:** commit with complete_at + completion handler + tests.
- **Exit-criteria:** completion offers scheme names in the scheme position, option
  names in the query-string position; empty list (no panic) for cursor outside any
  URI or for minimal-metadata schemes.

### Phase 4: Hover

- **Goal:** editor shows hover info (description, deprecated, secret).
- **Dependencies:** Phase 2.
- **Externally-visible types/interfaces:** `LintEngine::hover_at`, LSP
  textDocument/hover handler, `HoverInfo` type in camel-lint.
- **Deliverable:** commit with hover_at + hover handler + tests.
- **Exit-criteria:** hover on an option key returns its description/deprecated/secret
  metadata; returns `None` for unknown options or cursor outside any option.

### Phase 5: Integration tests

- **Goal:** end-to-end fixture editor sessions covering the full lifecycle.
- **Dependencies:** all prior phases.
- **Externally-visible types/interfaces:** `tests/lsp_session.rs` integration test.
- **Deliverable:** commit with fixture sessions + partial-input edge cases + debounce
  verification.
- **Exit-criteria:** all session tests pass; partial-input fuzz cases (empty string,
  truncated YAML, malformed Unicode-containing route text) never panic.

## Alternatives considered

- **`lsp-server` (rust-analyzer stack):** rejected — sync/threaded model, idiomatic
  mismatch with the tokio-first workspace. Would require a dedicated thread pool
  and bridge to async catalog/engine calls.
- **`async-lsp`:** rejected — smaller ecosystem, more moving parts (tower-service
  middleware stack) than the payoff justifies for a single-protocol stdio server.
- **Full re-parse on didChange (no apply_edit):** would simplify the engine but
  violates the "no full re-receive per keystroke" acceptance criterion and forces
  the client to resend the entire document text on every change. The incremental
  range-edit path is a tower-lsp standard and cheap to support via apply_edit.
- **Completion/hover logic in camel-lsp (not camel-lint):** rejected — would make the
  logic protocol-coupled and untestable without an LSP client. Keeping it in
  camel-lint as protocol-agnostic types is the reversibility decision (e_opus ruling).

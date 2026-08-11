# Proposal: add-camel-lsp

## Why

`camel lint` runs out-of-process: the developer edits a route file, saves, switches
to a terminal, and re-runs the command. An in-editor LSP server removes that
round-trip — diagnostics appear on every keystroke, completion offers scheme/param
candidates inline, and hover shows metadata without leaving the cursor. This is the
second half of the lint split (Change A = `camel-lint` engine + `camel lint` CLI,
now merged); the engine's trust baseline (zero false positives over the in-tree
corpus) is proven, so exposing it through a protocol adapter is low-risk.

## What Changes

**Included:**

- New crate `camel-lsp` — a stdio LSP server (tower-lsp) that translates editor
  events into `camel-lint` engine calls. Deps: `camel-lint`, `tower-lsp`, `tokio`
  (no `camel-core`, `camel-cli`, or `camel-api`). Protocol types via
  `tower_lsp::lsp_types`.
- `camel-lint` extensions (protocol-agnostic, reusable beyond LSP):
  `Document::apply_edit(start, end, text)` for incremental didChange (always commits,
  including intermediate invalid states); `LintEngine::complete_at(&document, offset)`
  returning completion candidates (URI schemes, option keys, option values by
  context); `LintEngine::hover_at(&document, offset)` returning hover info
  (option description, deprecated reason, secret flag).
- `camel lsp` CLI subcommand — calls `production_engine()` to build the engine,
  then boots the stdio server.
- Handlers: `didOpen`, `didChange` (incremental via `apply_edit`), `didSave` →
  re-lint → `publishDiagnostics`; `completion` → `complete_at`; `hover` →
  `hover_at`.

**Excluded (post trust-baseline, bd follow-ups):**

- Code actions / quick fixes
- Document formatting
- Document symbols / outline
- Deep dataflow rules (`to` → `direct` match analysis)

## Acceptance criteria

- `camel-lsp` depends on `camel-lint` only (no `camel-core`/`camel-cli`/`camel-api`);
  hex-arch boundary test extended to assert this.
- `didChange` applies incremental range edits via `Document::apply_edit` — always
  commits (mirrors editor state, including intermediate invalid syntax).
- Completion offers candidates for schemes with non-empty `uri_options`; graceful
  (empty list, no panic) for minimal-metadata schemes.
- Hover shows option description / deprecated reason / secret flag when metadata
  is available; returns `None` gracefully otherwise.
- Partial/garbage input (truncated YAML, unclosed brackets) never panics the
  server; syntax diagnostics still publish.
- `cargo fmt`, `cargo clippy -D warnings`, and the hex-arch boundary test pass.

## Risk budget

- **Tower-lsp pin** is reversible: `camel-lsp` is a thin adapter. All
  diagnostics/completion/hover logic lives in `camel-lint` as protocol-agnostic
  types. Replacing tower-lsp means rewriting the glue layer (~hundreds of lines),
  not the engine.
- **No new runtime deps on camel-core or camel-api**: the server receives a
  fully-built `LintEngine` from the CLI (which calls `production_engine()`). The
  server names only `camel-lint` types — never `camel-api` (where the catalog
  trait lives).
- **Partial-input panics** are the highest-likelihood defect: the server must
  handle any UTF-8 string the editor sends (LSP document text is always Unicode).
  Edge-case coverage is in scope for Phase 5 integration tests.

Bd: rc-o5qz

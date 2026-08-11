# route-lsp Specification

## Purpose
TBD - created by archiving change add-camel-lsp. Update Purpose after archive.
## Requirements
### Requirement: LSP server runs over stdio and completes the initialize handshake

The `camel-lsp` crate SHALL expose a tower-lsp `Backend` struct that speaks the
Language Server Protocol over stdin/stdout. The `camel lsp` CLI subcommand SHALL
boot the server by calling `production_engine()` (from `camel-cli`'s lint module)
to build a `LintEngine` with the production catalog, then passing that engine to
`camel-lsp::Backend::new(engine)`. The server SHALL respond to the LSP `initialize`
request with server capabilities (textDocument sync = `Incremental`, completion
provider = true, hover provider = true) and SHALL complete the `initialized`
handshake. The server SHALL NOT advertise `diagnostic_provider` (pull diagnostics)
— only push diagnostics (`publishDiagnostics`) are implemented. The server SHALL
shut down cleanly on the `shutdown` → `exit` sequence.

#### Scenario: Initialize handshake completes

- **GIVEN** a `camel lsp` server process started on stdio
- **WHEN** an LSP client sends `initialize` followed by `initialized`
- **THEN** the server responds with a valid `InitializeResult` advertising incremental text-document sync, completion, and hover capabilities; `diagnostic_provider` SHALL be absent (push diagnostics require no capability advertisement)

#### Scenario: Shutdown then exit terminates the server

- **GIVEN** a running `camel lsp` server
- **WHEN** the client sends `shutdown` then `exit`
- **THEN** the server process exits with code 0

### Requirement: didOpen stores the document and publishes diagnostics

The server SHALL handle `textDocument/didOpen` by parsing the document text into a
`camel-lint::Document`, storing it in the per-URI document map, running the
`LintEngine`, and publishing diagnostics via `textDocument/publishDiagnostics`.
Diagnostics SHALL be mapped from `camel-lint::Diagnostic` to
`tower_lsp::lsp_types::Diagnostic` with byte-offset spans converted to LSP `Position`
(line/column) via the UTF-16 line mapping.

#### Scenario: Opening a valid document publishes zero diagnostics

- **GIVEN** a server with the production catalog and a valid route file
- **WHEN** the client sends `didOpen` with that file's text
- **THEN** the server publishes a `publishDiagnostics` notification with an empty diagnostics array

#### Scenario: Opening a document with a syntax error publishes a diagnostic

- **GIVEN** a server and a route file with an unclosed YAML bracket
- **WHEN** the client sends `didOpen` with that text
- **THEN** the server publishes a `publishDiagnostics` notification containing at least one diagnostic with severity `Error` positioned at the syntax error location

### Requirement: didChange applies incremental range edits and republishes diagnostics

The server SHALL handle `textDocument/didChange` with `Incremental` sync by
converting each `tower_lsp::lsp_types::TextDocumentContentChangeEvent` (range + text) into a
`Document::apply_edit(start, end, text)` call, then re-running the engine and
republishing diagnostics. The server SHALL NOT require the full document text on
each change (range edits are sufficient). When a change event has no `range` (full-
document replacement), the server SHALL re-parse the entire text via
`Document::parse`.

#### Scenario: Range edit updates diagnostics incrementally

- **GIVEN** an open document `from: direct:start\n` and the server has published its diagnostics
- **WHEN** the client sends a `didChange` with a range edit replacing `start` with `end`
- **THEN** the server applies the edit via `Document::apply_edit`, re-lints, and republishes diagnostics reflecting the updated document

#### Scenario: Full-document replacement re-parses

- **GIVEN** an open document and the server has published its diagnostics
- **WHEN** the client sends a `didChange` event with no `range` (full text replacement)
- **THEN** the server re-parses the new text via `Document::parse` and republishes diagnostics

#### Scenario: Rapid didChange events do not interleave corrupt state

- **GIVEN** an open document and a rapid sequence of 5 `didChange` range edits
- **WHEN** the edits are processed
- **THEN** the document map's final state reflects all 5 edits applied in order, and the final diagnostics reflect the fully-edited document

### Requirement: didSave republishes diagnostics

The server SHALL handle `textDocument/didSave` by re-running the engine over the
stored document and republishing diagnostics. This supports workflows where the
client only re-reads the file from disk on save.

#### Scenario: Save triggers re-lint

- **GIVEN** an open document with stale diagnostics
- **WHEN** the client sends `didSave`
- **THEN** the server re-lints and republishes current diagnostics

### Requirement: didClose removes the document and clears diagnostics

The server SHALL handle `textDocument/didClose` by removing the document from the
per-URI document map, cancelling any pending or in-flight debounced lint for that
URI, and publishing an empty diagnostics notification for that URI (so the editor
clears stale diagnostics from the closed file). The cancellation prevents a stale
lint result from overwriting the empty diagnostics published by `didClose`.

#### Scenario: Close clears diagnostics

- **GIVEN** an open document with published diagnostics
- **WHEN** the client sends `didClose` for that URI
- **THEN** the server removes the document from its map, cancels any pending lint, and publishes an empty diagnostics array for that URI

#### Scenario: Close during pending lint discards the stale result

- **GIVEN** an open document with a debounced lint in-flight, and the client sends `didClose`
- **WHEN** the in-flight lint completes after `didClose`
- **THEN** no diagnostics are published for the closed URI (the result was cancelled)

### Requirement: Debounced diagnostics respect document version ordering

The server SHALL debounce `didChange`-triggered lint runs to avoid redundant
computation on rapid keystrokes. The debounce SHALL track the LSP document
`version` number: only the diagnostics from the lint run matching the latest
received `version` SHALL be published. If a newer `didChange` arrives while a
debounced lint is pending or in-flight, the stale result SHALL be discarded (not
published), preventing older diagnostics from overwriting newer ones. Diagnostics
from `didOpen` and `didSave` (which are not debounced) SHALL always publish.

#### Scenario: Rapid didChange events publish only the final version's diagnostics

- **GIVEN** an open document and a rapid sequence of 5 `didChange` events (versions 2–6)
- **WHEN** the debounce window elapses
- **THEN** the server publishes diagnostics exactly once, reflecting the document at version 6 (the latest)

#### Scenario: Stale in-flight lint result is discarded

- **GIVEN** a lint run in-flight for version 3, and a new `didChange` arrives for version 4
- **WHEN** the version-3 lint completes
- **THEN** its diagnostics are discarded (not published); only the version-4 lint result is published

### Requirement: textDocument/completion delegates to the engine

The server SHALL handle `textDocument/completion` by converting the LSP `Position`
to a byte offset (via the UTF-16 line mapping), calling
`LintEngine::complete_at(doc, offset)`, and mapping each `camel-lint::CompletionItem`
to an `tower_lsp::lsp_types::CompletionItem`. The response SHALL be a `CompletionResponse::List`
or `None` when the engine returns an empty list.

#### Scenario: Completion in scheme position returns scheme candidates

- **GIVEN** an open document `from: tim` and the cursor positioned inside `tim`
- **WHEN** the client sends `textDocument/completion` at that position
- **THEN** the server responds with a list including `timer` (and other catalog schemes)

#### Scenario: Completion in a non-URI region returns None

- **GIVEN** an open document and the cursor on a YAML key outside any URI
- **WHEN** the client sends `textDocument/completion` at that position
- **THEN** the server responds with `None` (no candidates)

### Requirement: textDocument/hover delegates to the engine

The server SHALL handle `textDocument/hover` by converting the LSP `Position` to a
byte offset, calling `LintEngine::hover_at(doc, offset)`, and mapping the
`camel-lint::HoverInfo` (when `Some`) to an `tower_lsp::lsp_types::Hover` with markdown
content. The response SHALL be `None` when the engine returns `None`.

#### Scenario: Hover on a documented option returns markdown content

- **GIVEN** an open document with `from: timer:tick?period=1s` and a catalog where `period` has a description
- **WHEN** the client sends `textDocument/hover` at the position of `period`
- **THEN** the server responds with a `Hover` containing the option description in its markdown content

#### Scenario: Hover outside any option returns None

- **GIVEN** an open document and the cursor in a non-option region
- **WHEN** the client sends `textDocument/hover` at that position
- **THEN** the server responds with `None`

### Requirement: Partial and malformed input never panics the server

The server SHALL handle any UTF-8 string received via `didOpen` or `didChange`
without panicking. (LSP document text is always a valid Unicode string carried in
a JSON-RPC `string` — raw binary bytes cannot arrive over the protocol.) Edge
cases that SHALL NOT panic include: empty strings, truncated YAML (unclosed
brackets, dangling colons), route text containing non-ASCII Unicode (accented
letters, emoji, CJK characters), and documents much larger than any reasonable
route file. Syntax errors SHALL be reported as diagnostics via the engine's R-SYN
rule, not as server crashes. Byte-offset ↔ LSP Position conversions SHALL be
total: out-of-bounds offsets from race conditions between `didChange` and
`completion`/`hover` SHALL return empty/`None`, not panic.

#### Scenario: Empty document does not crash

- **GIVEN** a running server
- **WHEN** the client sends `didOpen` with an empty string
- **THEN** the server publishes diagnostics (possibly a parse-related note or empty) and does not panic

#### Scenario: Non-ASCII Unicode in route text does not crash

- **GIVEN** a running server
- **WHEN** the client sends `didOpen` with a route containing emoji and CJK characters (e.g. `from: timer:café?note=こんにちは🌟`)
- **THEN** the server publishes diagnostics and does not panic; subsequent completion/hover requests at positions after the non-ASCII characters resolve correctly

#### Scenario: Truncated YAML does not crash

- **GIVEN** a running server
- **WHEN** the client sends `didOpen` with a truncated route (`from: timer:tick?period=` — value left incomplete)
- **THEN** the server publishes syntax diagnostics and does not panic

#### Scenario: Completion request on a partially-typed URI does not crash

- **GIVEN** an open document containing only `from: ` (incomplete URI) and the cursor after the space
- **WHEN** the client sends `textDocument/completion` at that position
- **THEN** the server responds (possibly with scheme candidates or an empty list) and does not panic

### Requirement: camel-lsp does not depend on camel-core, camel-dsl, or camel-cli

The `camel-lsp` crate SHALL depend on `camel-lint` (for the engine, document,
completion, and hover types), `tower-lsp` (for the protocol layer, including its
re-exported `tower_lsp::lsp_types`), and `tokio` (the async runtime tower-lsp
requires) only. It SHALL NOT depend on `camel-core`
(runtime lifecycle), `camel-dsl` (runtime DSL compilation), `camel-cli` (CLI
binary), or `camel-api` (the trait definitions — accessed transitively via
`camel-lint`'s re-exports). The production `LintEngine` is constructed by the
CLI's `camel lsp` subcommand via `production_engine()` and passed into the server;
the server receives it as a fully-built `LintEngine`, not a catalog trait object.
The workspace hexagonal-architecture test SHALL be extended to assert these
dependency edges.

#### Scenario: Hex-arch boundary test includes camel-lsp

- **GIVEN** the workspace hexagonal-architecture test
- **WHEN** the test checks `camel-lsp`'s dependency edges
- **THEN** none of `camel-core`, `camel-dsl`, or `camel-cli` appears as a dependency of `camel-lsp`

#### Scenario: camel lsp subcommand builds the engine and boots the server

- **GIVEN** the `camel-cli` source
- **WHEN** `camel lsp` is invoked
- **THEN** it calls `production_engine()` to build a `LintEngine`, constructs the `camel-lsp` server with that engine, and runs it over stdio


# Tasks: add-camel-lsp

## Phase 1: camel-lsp scaffold + Document::apply_edit

### camel-lint

#### Task 1.1: Add Document::apply_edit and refactor apply_fix to delegate

**Files:**
- `crates/camel-lint/src/document.rs` (modified)

**Steps:**
1. Add a new pub method `apply_edit(&mut self, start: usize, end: usize, replacement: &str) -> Result<(), LintError>` on `impl Document`. The body SHALL:
   - Parse `&self.raw` into a CST document via `cst::parse_document(&self.raw)`. On parse error, return `Err(LintError::Internal(format!("apply_edit source un-parseable: {e}")))`.
   - Call `cst_doc.replace_span(start, end, replacement)`. On error (out-of-bounds, non-character-boundary), return `Err(LintError::Internal(format!("apply_edit edit rejected: {e}")))`.
   - Get `new_raw = cst_doc.source().to_string()`.
   - `let reparsed = Document::parse(&new_raw);` — always, regardless of `parse_failure`.
   - Commit: `self.raw = reparsed.raw; self.route_view = reparsed.route_view; self.parse_failure = reparsed.parse_failure;`
   - Return `Ok(())`.
2. Refactor the existing `apply_fix` to delegate: clone self before the edit, call `self.apply_edit(fix.span.start, fix.span.end, &fix.replacement)`, then if the result is `Ok(())` but `self.parse_failure.is_some()`, restore the pre-edit state from the clone and return `Err(LintError::Internal("apply_fix produced invalid syntax".into()))`. Otherwise return the `apply_edit` result.

**Tests:**
- `apply_edit_replaces_range`: setup `Document::parse("from: direct:start\n")` — byte layout: `from: ` = 0–5, `direct:` = 6–12, `start` = 13–17. Action `apply_edit(13, 18, "end")`, assert `raw == "from: direct:end\n"` and `parse_failure.is_none()`.
- `apply_edit_commits_syntax_breaking_edit`: setup valid doc, action `apply_edit` with a replacement producing unclosed `[`, assert result is `Ok(())`, `parse_failure.is_some()`, `raw` reflects edited text, AND `engine.lint(&doc.raw)` yields at least one R-SYN diagnostic.
- `apply_edit_recovers_invalid_to_valid`: setup a doc with `parse_failure = Some(_)`, action `apply_edit` that fixes the syntax, assert `parse_failure.is_none()` and `route_view` reflects valid structure.
- `apply_edit_rejects_out_of_bounds`: setup 20-byte source, action `apply_edit(0, 25, "x")`, assert `Err` and document byte-identical to pre-edit.
- `apply_fix_rolls_back_on_parse_failure`: setup valid doc + a Fix whose replacement breaks syntax, action `apply_fix(fix)`, assert `Err` and document byte-identical to pre-edit.
- Command: `cargo test -p camel-lint --lib document`
- Expected: all pass after implementation.

**Acceptance:**
- `cargo test -p camel-lint --lib` exits 0.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.
- `cargo test -p camel-cli --test lint_corpus` exits 0 (apply_fix behavior unchanged).

- [x] 1.1

### camel-lsp

#### Task 1.2: Create camel-lsp crate scaffold

**Files:**
- `crates/camel-lsp/Cargo.toml` (new)
- `crates/camel-lsp/src/lib.rs` (new)
- `crates/camel-lsp/CONTEXT.md` (new)
- `Cargo.toml` (modified — add `camel-lsp` to workspace members + `tower-lsp` + `camel-lsp` to `[workspace.dependencies]`)

**Steps:**
1. In root `Cargo.toml`, add `"crates/camel-lsp"` to `[workspace] members`. Add to `[workspace.dependencies]`: `tower-lsp = "0.20"` and `camel-lsp = { path = "crates/camel-lsp" }`. Verify `tokio` is already present (it is); verify `camel-lint` is already in `[workspace.dependencies]` (added in Change A).
2. Create `crates/camel-lsp/Cargo.toml` with: `[package]` name = "camel-lsp", `version.workspace = true`, `edition = "2021"`. `[dependencies]` `camel-lint = { workspace = true }`, `tower-lsp = { workspace = true }`, `tokio = { workspace = true }`.
3. Create `crates/camel-lsp/src/lib.rs` with:
   - `pub mod position;` (empty module stub, fleshed out in Task 2.1)
   - `#[derive(Clone)] pub struct Backend { client: tower_lsp::Client, engine: std::sync::Arc<camel_lint::LintEngine> }`
   - Note: `LintEngine` is NOT `Clone` (holds `Vec<Box<dyn Rule>>`); wrapping in `Arc` makes `Backend` cloneable. tower-lsp clones `Backend` internally for each request via the `LspService::new(|client| Backend::new(client, engine))` closure pattern.
   - `impl Backend { pub fn new(client: tower_lsp::Client, engine: camel_lint::LintEngine) -> Self { Self { client, engine: Arc::new(engine) } } }`
   - `#[tower_lsp::async_trait] impl tower_lsp::LanguageServer for Backend` with stub `initialize` (returns `Ok(InitializeResult::default())`), `shutdown` (returns `Ok(())`), and all other handlers returning `Ok(())`.
4. Create `crates/camel-lsp/src/position.rs` as an empty module (`// Position conversion utilities — fleshed out in Task 2.1`).
5. Create `crates/camel-lsp/CONTEXT.md` with: "LSP server crate (stdio). Thin adapter: translates LSP JSON-RPC to camel-lint engine calls. Deps: camel-lint + tower-lsp + tokio only (no camel-core/camel-cli/camel-api)."

**Tests:**
- `cargo build -p camel-lsp` compiles with no errors. This IS the test (crate scaffold).
- Command: `cargo build -p camel-lsp`
- Expected: pass after implementation.

**Acceptance:**
- `cargo build -p camel-lsp` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 1.2

#### Task 1.3: Initialize/shutdown handlers + camel lsp CLI subcommand

**Files:**
- `crates/camel-lsp/src/lib.rs` (modified — flesh out `initialize` and `shutdown`)
- `crates/camel-cli/Cargo.toml` (modified — add `camel-lsp = { workspace = true }` dependency)
- `crates/camel-cli/src/commands/lsp.rs` (new)
- `crates/camel-cli/src/commands/mod.rs` (modified — add `pub mod lsp;`)
- `crates/camel-cli/src/main.rs` (modified — add `Lsp` subcommand to clap enum)

**Steps:**
1. In `crates/camel-lsp/src/lib.rs`, implement `initialize` in the `LanguageServer` impl to return `InitializeResult` with: `capabilities.text_document_sync = Some(TextDocumentSyncKind::INCREMENTAL)`, `capabilities.completion_provider = Some(CompletionOptions { ..Default::default() })`, `capabilities.hover_provider = Some(OneOf::Left(true))`. Do NOT set `diagnostic_provider` (it must be absent — push diagnostics only). Set `server_info` to `Some(ServerInfo { name: "camel-lsp".into(), version: Some(env!("CARGO_PKG_VERSION").into()) })`.
2. Implement `shutdown` to return `Ok(())`.
3. Add `camel-lsp = { workspace = true }` to `crates/camel-cli/Cargo.toml` `[dependencies]`.
4. Create `crates/camel-cli/src/commands/lsp.rs` with a `pub async fn run() -> i32` that:
   - Calls `crate::commands::lint::production_engine().await` and on `Err` prints to stderr and returns 2.
   - Constructs the LSP service: `let (service, socket) = tower_lsp::LspService::new(move |client| camel_lsp::Backend::new(client, engine));` (the closure captures `engine` by move; tower-lsp calls it with the `Client` on connection).
   - Spawns `tower_lsp::Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service)`.
   - Awaits the server and returns 0 on clean shutdown.
5. In `crates/camel-cli/src/commands/mod.rs`, add `pub mod lsp;`.
6. In `crates/camel-cli/src/main.rs`, add a `Lsp` variant to the clap subcommand enum. In the dispatch, call `commands::lsp::run().await` and exit with its code.

**Tests:**
- `lsp_initialize_handshake`: setup — build `Backend::new(test_client(), test_engine())` via a `LspService`, create `tokio::io::duplex(8192)`, spawn `Server::new(read, write, socket).serve(service)`. Write an `initialize` JSON-RPC request (Content-Length framed) to the write half. Action: read the response. Assert: the `InitializeResult` capabilities include `text_document_sync == Incremental`, `completion_provider.is_some()`, `hover_provider` is set, and `diagnostic_provider` is `None` (absent from the JSON).
- `lsp_shutdown_exits_clean`: setup same server, send `shutdown` then `exit`, assert: server task completes without panic.
- Command: `cargo test -p camel-lsp --lib`
- Expected: pass after implementation.

**Acceptance:**
- `cargo build -p camel-cli -p camel-lsp` exits 0.
- `cargo test -p camel-lsp --lib` exits 0.
- `cargo clippy -p camel-lsp -p camel-cli -- -D warnings` exits 0.

- [x] 1.3

#### Task 1.4: Extend hexagonal-architecture boundary test for camel-lsp

**Files:**
- `crates/camel-core/tests/hexagonal_architecture_boundaries_test.rs` (modified)

**Steps:**
1. Find the existing test asserting `camel-lint` does not depend on `camel-core` or `camel-dsl` (added in Change A).
2. Add parallel assertions for `camel-lsp`: it SHALL NOT depend on `camel-core`, `camel-dsl`, `camel-cli`, or `camel-api`.
3. Follow the existing `cargo metadata` JSON pattern used for `camel-lint`.

**Tests:**
- `camel_lsp_boundary_no_camel_core`: assert `camel-core` is absent from `camel-lsp`'s dependency graph.
- `camel_lsp_boundary_no_camel_cli`: assert `camel-cli` is absent.
- `camel_lsp_boundary_no_camel_api`: assert `camel-api` is absent from `camel-lsp`'s direct dependencies.
- Command: `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` exits 0.

- [x] 1.4

## Phase 2: Diagnostics pipeline

### camel-lsp

#### Task 2.1: Document state map, version tracking, and byte-offset ↔ LSP Position conversion

**Files:**
- `crates/camel-lsp/src/position.rs` (modified — implement the functions)
- `crates/camel-lsp/src/lib.rs` (modified — add documents field with version tracking)

**Steps:**
1. Implement `crates/camel-lsp/src/position.rs` with two pub functions:
   - `pub fn byte_offset_to_lsp(source: &str, byte_offset: usize) -> tower_lsp::lsp_types::Position` — walks the source char-by-char tracking line number (0-based) and UTF-16 code unit count within the line. Each `char` contributes 1 or 2 UTF-16 code units (2 if `c > '\u{FFFF}'`, i.e. outside BMP). Returns `Position { line, character }`. Clamp `byte_offset` to `source.len()`.
   - `pub fn lsp_to_byte_offset(source: &str, position: tower_lsp::lsp_types::Position) -> usize` — walks lines and UTF-16 code units to find the byte offset. Returns `source.len()` for out-of-bounds positions (total, never panics).
2. In `crates/camel-lsp/src/lib.rs`, add a `documents` field to `Backend`:
   - `documents: std::sync::Arc<tokio::sync::RwLock<std::collections::HashMap<tower_lsp::lsp_types::Url, (camel_lint::Document, Option<i32>)>>>`
   - The tuple stores `(Document, Option<version>)` — the version comes from LSP `text_document.version`.
   - `Backend::new` initializes this to an empty map.

**Tests:**
- `byte_offset_to_lsp_basic`: setup `"hello\nworld\n"`, action `byte_offset_to_lsp(source, 7)`, assert `Position { line: 1, character: 1 }`.
- `byte_offset_to_lsp_non_ascii`: setup `"café\ntest\n"` (é = 2 UTF-8 bytes, 1 UTF-16 code unit), action `byte_offset_to_lsp(source, 5)` (byte offset of `\n` after `café`), assert `line: 0, character: 4`.
- `byte_offset_to_lsp_emoji`: setup `"🌟\nx\n"` (🌟 = U+1F31F, 4 UTF-8 bytes, 2 UTF-16 code units), action `byte_offset_to_lsp(source, 4)` (byte offset of first `\n`), assert `line: 0, character: 2`.
- `lsp_to_byte_offset_roundtrip`: setup `"hello\nworld\n"`, action `lsp_to_byte_offset(source, Position{line:1,character:1})`, assert `7`.
- `lsp_to_byte_offset_out_of_bounds`: setup `"hi\n"`, action `lsp_to_byte_offset(source, Position{line:99,character:99})`, assert `source.len()` (no panic).
- Command: `cargo test -p camel-lsp --lib position`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --lib` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 2.1

#### Task 2.2: didOpen handler

**Files:**
- `crates/camel-lsp/src/lib.rs` (modified)

**Steps:**
1. Add a private helper `fn diagnostics_to_lsp(source: &str, diags: Vec<camel_lint::Diagnostic>) -> Vec<tower_lsp::lsp_types::Diagnostic>` that maps:
   - `Severity::Error` → `DiagnosticSeverity::ERROR`, `Warning` → `WARNING`, `Info` → `INFORMATION`.
   - `DiagnosticCode` → string label via `Display`.
   - `Span { start, end }` → `Range { start: byte_offset_to_lsp(source, span.start), end: byte_offset_to_lsp(source, span.end) }`.
2. Implement `did_open` in the `LanguageServer` impl:
   - Extract `uri = params.text_document.uri`, `text = params.text_document.text`, `version = params.text_document.version`.
   - Parse via `Document::parse(&text)`.
   - Insert `(doc, Some(version))` into `self.documents.write().await`.
   - Run `self.engine.lint(&doc.raw)`.
   - Publish via `self.client.publish_diagnostics(uri, diagnostics_to_lsp(&doc.raw, diags), Some(version)).await`.

**Tests:**
- `did_open_valid_publishes_empty`: setup a server with a test engine that returns no diagnostics, send `didOpen` with valid route text, assert: `publishDiagnostics` received with empty array.
- `did_open_syntax_error_publishes_diagnostic`: send `didOpen` with text containing unclosed `[`, assert: `publishDiagnostics` received with at least one `Error`-severity diagnostic.
- Command: `cargo test -p camel-lsp --lib`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --lib` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 2.2

#### Task 2.3: didChange handler, DebouncedLinter, and version-ordered publication

**Files:**
- `crates/camel-lsp/src/debounce.rs` (new)
- `crates/camel-lsp/src/lib.rs` (modified — add debouncer field + didChange handler)

**Steps:**
1. Create `crates/camel-lsp/src/debounce.rs` with:
   - `#[derive(Clone)] pub struct DebouncedLinter { pending: std::sync::Arc<tokio::sync::Mutex<std::collections::HashMap<tower_lsp::lsp_types::Url, (i32, tokio::task::JoinHandle<()>)>>> }`
   - `pub fn new() -> Self` — empty map.
   - `pub async fn schedule(&self, version: i32, uri: Url, documents: Arc<RwLock<HashMap<Url, (Document, Option<i32>)>>>, client: Client, engine: Arc<LintEngine>, delay: Duration)`: lock the map, if an entry for `uri` exists abort its task, spawn a new task that: sleeps `delay`, then reads `(doc, current_ver)` from `documents` via read lock, runs `engine.lint(&doc.raw)`, then BEFORE publishing re-reads `current_ver` from `documents` — if `current_ver != Some(version)`, discard (stale), otherwise publishes via `client.publish_diagnostics`. Insert `(version, handle)` into the map.
   - `pub async fn cancel(&self, uri: &Url)`: lock the map, remove and abort the entry for `uri`.
2. Add `debouncer: Arc<DebouncedLinter>` field to `Backend`. Update `Backend::new` to initialize it.
3. Implement `did_change` in the `LanguageServer` impl:
   - Acquire a write lock on `self.documents` ONCE for the entire notification.
   - For each `change` in `params.content_changes`: get the current `(doc, _)` from the map (or clone the doc), apply the change:
     - If `change.range` is `Some(range)`: convert to byte offsets via `position::lsp_to_byte_offset`, call `doc.apply_edit(start, end, &change.text)`.
     - If `change.range` is `None` (full replacement): `doc = Document::parse(&change.text)`.
   - After ALL changes applied, store `(doc, Some(version))` back into the map. Release the write lock.
   - Schedule: `self.debouncer.schedule(version, uri, self.documents.clone(), self.client.clone(), self.engine.clone(), DEBOUNCE_DELAY).await`.
4. Export `debounce` module from `lib.rs`.

**Tests:**
- `did_change_range_edit_updates_document`: setup open doc `"from: direct:start\n"`, send `didChange` with range replacing `start` (bytes 13–17 in the source) with `end`, assert: stored doc's `raw` equals `"from: direct:end\n"`.
- `did_change_full_replacement_replaces`: setup open doc, send `didChange` with no range and full text, assert: stored doc reflects new text.
- `did_change_rapid_sequence_final_state`: send 5 rapid `didChange` events, assert: final stored doc reflects all 5 edits applied in sequence.
- `debounce_publishes_only_final_version`: send 5 `didChange` events (versions 2–6), wait past debounce (200ms), assert: exactly one `publishDiagnostics` received, reflecting version 6.
- `debounce_stale_result_discarded`: trigger lint for version 3, send `didChange` for version 4 before debounce elapses, wait, assert: only version-4 diagnostics published.
- Command: `cargo test -p camel-lsp --lib`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --lib` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 2.3

#### Task 2.4: didSave and didClose handlers

**Files:**
- `crates/camel-lsp/src/lib.rs` (modified)

**Steps:**
1. Implement `did_save`:
   - Read doc from `self.documents.read().await`.
   - Run `self.engine.lint(&doc.raw)` and publish diagnostics immediately (no debounce — save is explicit).
2. Implement `did_close`:
   - Call `self.debouncer.cancel(&uri).await` to kill any pending lint.
   - Remove the document from `self.documents.write().await`.
   - Publish empty diagnostics: `self.client.publish_diagnostics(uri, vec![], None).await`.

**Tests:**
- `did_save_republishes_diagnostics`: setup open doc, send `didSave`, assert: fresh `publishDiagnostics` received.
- `did_close_clears_diagnostics`: setup open doc with diagnostics, send `didClose`, assert: `publishDiagnostics` with empty array, doc removed from map.
- `did_close_cancels_pending_lint`: trigger rapid `didChange` (schedules debounced lint), immediately send `didClose`, wait past debounce, assert: no diagnostics published after the close.
- Command: `cargo test -p camel-lsp --lib`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --lib` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 2.4

## Phase 3: Completion

### camel-lint

#### Task 3.1: CompletionItem type and LintEngine::complete_at

**Files:**
- `crates/camel-lint/src/completion.rs` (new — type definitions)
- `crates/camel-lint/src/engine.rs` (modified — add `complete_at` method to `impl LintEngine`)
- `crates/camel-lint/src/lib.rs` (modified — add `pub mod completion;` and `pub use completion::*;`)

**Steps:**
1. Create `crates/camel-lint/src/completion.rs` with:
   - `pub struct CompletionItem { pub label: String, pub detail: Option<String> }`
2. In `crates/camel-lint/src/engine.rs`, add `pub fn complete_at(&self, doc: &Document, offset: usize) -> Vec<CompletionItem>` to `impl LintEngine`. The method has direct access to `self.catalog` (the private field). The logic:
   - If `offset > doc.raw.len()`, return `vec![]`.
   - Walk `doc.route_view.endpoints()` to find an `Endpoint` whose URI span contains `offset`.
   - If none found, return `vec![]`.
   - Within the endpoint URI string, determine cursor context:
     - **Scheme position**: offset is at or before the first `:` in the URI. Return all catalog scheme names: `self.catalog.schemes().into_iter().map(|s| CompletionItem { label: s, detail: None }).collect()`.
     - **Option-key position**: offset is after `?` or `&`, before `=`. Extract the scheme (substring before `:`). Call `self.catalog.get_metadata(scheme)`. If `Some(meta)` and `meta.uri_options` is non-empty, return each option name + each alias as `CompletionItem { label: name, detail: Some(opt.description.clone()) if non-empty else None }`. If scheme is minimal or absent, return `vec![]`.
     - **Else**: return `vec![]`.
   - Note: `UrlOption` has field `aliases: Vec<String>` (plural, `Vec`), `description: String` (not `Option`). Convert description to `Option`: `if opt.description.is_empty() { None } else { Some(opt.description.clone()) }`.
3. In `lib.rs`, add `pub mod completion;` and `pub use completion::*;`.

**Tests:**
- `complete_at_scheme_position`: setup engine + doc `"from: tim"` (byte layout: `from: ` = 0–5, `tim` = 6–8). Action `complete_at(&doc, 7)` (inside `i` of `tim`). Catalog has `timer`/`log`/`direct`. Assert: result contains `CompletionItem { label: "timer", .. }`.
- `complete_at_option_key_position`: setup engine + doc `"from: timer:tick?per"` (byte layout: `from: ` = 0–5, `timer:tick?` = 6–16, `per` = 17–19). Action `complete_at(&doc, 18)` (inside `e` of `per`). Catalog `timer` has option `period`. Assert: result contains `CompletionItem { label: "period", .. }`.
- `complete_at_minimal_scheme_returns_empty`: setup engine + doc `"from: redis:cache?op"`, catalog `redis` is minimal (no `uri_options`). Assert: result is `vec![]`.
- `complete_at_outside_uri_returns_empty`: cursor on YAML key outside any URI. Assert: result is `vec![]`.
- `complete_at_offset_beyond_source_returns_empty`: setup 20-byte doc, offset 50. Assert: `vec![]` (no panic).
- Command: `cargo test -p camel-lint --lib completion`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lint --lib` exits 0.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.

- [x] 3.1

### camel-lsp

#### Task 3.2: textDocument/completion handler

**Files:**
- `crates/camel-lsp/src/lib.rs` (modified)

**Steps:**
1. Implement `completion` in the `LanguageServer` impl:
   - Read the document for `params.text_document_position.text_document.uri` from `self.documents.read().await`.
   - If not found, return `Ok(None)`.
   - Convert `params.text_document_position.position` to byte offset via `position::lsp_to_byte_offset(&doc.raw, position)`.
   - Call `self.engine.complete_at(&doc, byte_offset)`.
   - Map each `camel_lint::CompletionItem` to `tower_lsp::lsp_types::CompletionItem { label: ci.label, detail: ci.detail, ..Default::default() }`.
   - Return `Ok(Some(CompletionResponse::Array(items)))` or `Ok(None)` if empty.

**Tests:**
- `completion_in_scheme_position_returns_candidates`: setup open doc `"from: tim"`, send `completion` at Position matching byte 7 (inside `tim`). Assert: response includes label `"timer"`.
- `completion_outside_uri_returns_none`: cursor on YAML key. Assert: response is `None`.
- `completion_on_closed_doc_returns_none`: no open doc. Assert: `None` (no panic).
- Command: `cargo test -p camel-lsp --lib`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --lib` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 3.2

## Phase 4: Hover

### camel-lint

#### Task 4.1: HoverInfo type and LintEngine::hover_at

**Files:**
- `crates/camel-lint/src/hover.rs` (new — type definitions)
- `crates/camel-lint/src/engine.rs` (modified — add `hover_at` method to `impl LintEngine`)
- `crates/camel-lint/src/lib.rs` (modified — add `pub mod hover;` and `pub use hover::*;`)

**Steps:**
1. Create `crates/camel-lint/src/hover.rs` with:
   - `pub struct HoverInfo { pub description: Option<String>, pub deprecated: Option<String>, pub secret: bool }`
2. In `crates/camel-lint/src/engine.rs`, add `pub fn hover_at(&self, doc: &Document, offset: usize) -> Option<HoverInfo>` to `impl LintEngine`. Direct access to `self.catalog`. The logic:
   - If `offset > doc.raw.len()`, return `None`.
   - Walk `doc.route_view` to find an option key whose span contains `offset`.
   - Resolve the scheme from the enclosing endpoint URI.
   - Call `self.catalog.get_metadata(scheme)`. If `Some(meta)`, find the option in `meta.uri_options` whose `name` or `aliases` matches the option key text.
   - If found, return `Some(HoverInfo { description: if opt.description.is_empty() { None } else { Some(opt.description.clone()) }, deprecated: opt.deprecated.clone(), secret: opt.secret })`.
   - If scheme absent/minimal or option unknown, return `None`.
3. In `lib.rs`, add `pub mod hover;` and `pub use hover::*;`.

**Tests:**
- `hover_at_documented_option_returns_description`: setup engine + doc `"from: timer:tick?period=1s"`, catalog `timer` has option `period` with `description = "Tick interval"`. Action `hover_at` at the byte offset of `period`. Assert: `Some(HoverInfo { description: Some("Tick interval"), .. })`.
- `hover_at_deprecated_option_returns_reason`: catalog option `oldFreq` with `deprecated = Some("use period instead")`. Assert: result carries the reason.
- `hover_at_secret_option_returns_flag`: catalog option `password` with `secret = true`. Assert: `secret == true`.
- `hover_at_outside_option_returns_none`: cursor in scheme position. Assert: `None`.
- Command: `cargo test -p camel-lint --lib hover`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lint --lib` exits 0.
- `cargo clippy -p camel-lint -- -D warnings` exits 0.

- [x] 4.1

### camel-lsp

#### Task 4.2: textDocument/hover handler

**Files:**
- `crates/camel-lsp/src/lib.rs` (modified)

**Steps:**
1. Implement `hover` in the `LanguageServer` impl:
   - Read doc from map. If not found, return `Ok(None)`.
   - Convert position to byte offset.
   - Call `self.engine.hover_at(&doc, byte_offset)`.
   - If `Some(info)`, build markdown: join `description`, `"⚠ Deprecated: {reason}"` (if deprecated), `"🔒 Secret option"` (if secret). Return `Ok(Some(Hover { contents: HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value: md }), range: None }))`.
   - If `None`, return `Ok(None)`.

**Tests:**
- `hover_on_documented_option_returns_markdown`: setup open doc `"from: timer:tick?period=1s"`, catalog has `period.description`. Send `hover` at `period` offset. Assert: response is `Some(Hover)` with description in markdown.
- `hover_outside_option_returns_none`: cursor in scheme. Assert: `None`.
- Command: `cargo test -p camel-lsp --lib`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --lib` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 4.2

## Phase 5: Integration tests

### camel-lsp

#### Task 5.1: Editor session fixtures and partial-input edge cases

**Files:**
- `crates/camel-lsp/tests/lsp_session.rs` (new)

**Steps:**
1. Create a test helper `async fn spawn_server(engine: LintEngine) -> (tokio::io::WriteHalf<tokio::io::DuplexStream>, tokio::io::ReadHalf<tokio::io::DuplexStream>, tokio::task::JoinHandle<()>)` that:
   - Builds `LspService::new(|client| Backend::new(client, engine))` → `(service, socket)`.
   - Creates TWO duplex pairs: `tokio::io::duplex(8192)` returns `(server_io, client_io)` — `server_io` goes to the Server, `client_io` stays with the test.
   - Splits `server_io` into `(server_read, server_write)` and passes to `Server::new(server_read, server_write, socket).serve(service)`.
   - Splits `client_io` into `(client_read, client_write)` and returns `(client_write, client_read, handle)` — the test writes requests to `client_write` and reads responses from `client_read`.
2. Write helper `async fn send_jsonrpc(writer: &mut impl tokio::io::AsyncWrite + Unpin, method: &str, params: serde_json::Value, id: i64)` that serializes a Content-Length-framed JSON-RPC request.
3. Write helper `async fn read_jsonrpc(reader: &mut impl tokio::io::AsyncRead + Unpin) -> serde_json::Value` that reads a Content-Length-framed response/notification.
4. Add `serde_json` as a dev-dependency of `camel-lsp` if not already present.

**Tests:**
- `session_open_change_save_close`: spawn server, send `initialize` + `initialized`, send `didOpen` with `"from: direct:start\n"`, assert diagnostics published (empty for valid). Send `didChange` introducing syntax error (replace `start` with `[`), assert diagnostics with error. Send `didChange` fixing it (replace `[` with `start`), assert empty. Send `didSave`, assert still empty. Send `didClose`, assert empty diagnostics published.
- `session_partial_input_empty`: send `didOpen` with `""`, assert no panic, diagnostics published (empty or parse note).
- `session_partial_input_truncated_yaml`: send `didOpen` with `"from: timer:tick?period="` (incomplete value), assert no panic, diagnostic published.
- `session_non_ascii_unicode`: send `didOpen` with `"from: timer:café?note=こんにちは🌟"`, assert no panic. Send `completion` at a position in the option key, assert response received (no panic).
- `session_completion_partial_uri`: send `didOpen` with `"from: "` (incomplete), send `completion` after the space, assert no panic.
- Command: `cargo test -p camel-lsp --test lsp_session`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --test lsp_session` exits 0.
- All edge cases pass without panic.

- [x] 5.1

#### Task 5.2: Debounce and version-ordering tests

**Files:**
- `crates/camel-lsp/tests/lsp_session.rs` (modified — add debounce tests)

**Steps:**
1. Add tests using the helpers from Task 5.1, with short debounce delays. Set `DEBOUNCE_DELAY` to a test-friendly value (e.g. via a const override or test-only Backend constructor).

**Tests:**
- `debounce_publishes_only_final_version`: send 5 rapid `didChange` events (versions 2–6), wait 200ms, assert: exactly one `publishDiagnostics` received, reflecting document at version 6.
- `debounce_stale_result_discarded`: trigger lint for version 3, send `didChange` for version 4, wait, assert: only version-4 diagnostics published.
- `did_close_cancels_pending_lint`: trigger rapid `didChange`, immediately `didClose`, wait past debounce, assert: no diagnostics after close.
- Command: `cargo test -p camel-lsp --test lsp_session`
- Expected: pass after implementation.

**Acceptance:**
- `cargo test -p camel-lsp --test lsp_session` exits 0.
- `cargo clippy -p camel-lsp -- -D warnings` exits 0.

- [x] 5.2

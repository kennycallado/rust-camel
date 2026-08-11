## ADDED Requirements

### Requirement: Document supports incremental range edits

The `Document` struct SHALL expose `apply_edit(&mut self, start: usize, end: usize,
replacement: &str) -> Result<(), LintError>` which replaces the byte range
`[start, end)` in the source with `replacement`, re-parses the result, and **always
commits** the new state — including when the re-parse produces a `parse_failure`.
This mirrors an editor's live state: intermediate edits routinely produce invalid
syntax (e.g. typing `from:` before the value), and the server's document MUST
reflect the editor's actual text so R-SYN can report the syntax error.

`apply_edit` differs from `apply_fix` in this commitment semantic:

- **`apply_edit`** (LSP didChange) — always commits. The new `raw`, `route_view`,
  and `parse_failure` reflect the post-edit state regardless of validity. Returns
  `Err` ONLY for structural problems that prevent applying the edit at all
  (out-of-bounds range, non-character-boundary offset).
- **`apply_fix`** (automated lint fix) — transactional. Delegates to `apply_edit`
  for the byte replacement, but rolls back to the pre-edit `Document` if the result
  has a `parse_failure` (an automated fix must never break syntax). Returns `Err`
  on rollback.

`apply_edit` SHALL be total over its inputs: out-of-bounds ranges and
non-character-boundary offsets SHALL return `Err` without mutating the document,
and no input SHALL cause a panic.

#### Scenario: apply_edit replaces a byte range and updates the route view

- **GIVEN** a `Document` parsed from `from: direct:start\n` and an edit replacing byte offsets 12–17 (`start`) with `end`
- **WHEN** `apply_edit(12, 17, "end")` is called
- **THEN** the document's `raw` field equals `from: direct:end\n`, its `route_view.from` reflects the new value, and `parse_failure` is `None`

#### Scenario: apply_edit commits a syntax-breaking edit and records the failure

- **GIVEN** a `Document` parsed from a valid route, and a replacement that produces an unclosed YAML bracket
- **WHEN** `apply_edit` is called with that replacement
- **THEN** the result is `Ok(())`, the document's `raw` reflects the edited text, its `parse_failure` is `Some(_)`, and a subsequent `LintEngine::lint` over `doc.raw` emits an R-SYN diagnostic

#### Scenario: apply_edit recovers from invalid to valid

- **GIVEN** a `Document` whose `parse_failure` is `Some(_)` (currently broken), and a replacement that fixes the syntax
- **WHEN** `apply_edit` is called with that replacement
- **THEN** the result is `Ok(())`, `parse_failure` becomes `None`, and `route_view` reflects the now-valid structure

#### Scenario: apply_edit rejects out-of-bounds range

- **GIVEN** a `Document` parsed from a 20-byte source
- **WHEN** `apply_edit(0, 25, "x")` is called (end exceeds source length)
- **THEN** the result is `Err` and the document is byte-identical to its pre-edit state

#### Scenario: apply_fix delegates to apply_edit and rolls back on parse_failure

- **GIVEN** the refactored `apply_fix` implementation
- **WHEN** `apply_fix(fix)` is called and the resulting edit produces a `parse_failure`
- **THEN** `apply_fix` returns `Err`, and the document is byte-identical to its pre-fix state (the transactional rollback distinguishes it from `apply_edit`)

### Requirement: Engine provides completion candidates at a byte offset

The `LintEngine` SHALL expose `complete_at(&self, doc: &Document, offset: usize) ->
Vec<CompletionItem>`. The engine inspects `doc.route_view` to locate the cursor
context:

1. **Scheme position** — cursor is in or immediately after a scheme token (before
   the `:` separator). Returns one `CompletionItem` per catalog scheme name.
2. **Option-key position** — cursor is in the query-string region after `?` or `&`,
   in a position that is or follows an option key. Returns one `CompletionItem` per
   declared option name and alias for the resolved scheme.
3. **Option-value position** — cursor is in an option value. Returns kind-
   appropriate defaults (bool → `true` / `false`; other kinds → empty list).
4. **No context** — cursor is outside any URI span. Returns an empty list.

The method SHALL NOT panic for any offset, including offsets beyond the source
length. For a scheme whose catalog entry is `minimal` (no `uri_options`), the
option-key position SHALL return an empty list (graceful, not an error).

#### Scenario: Scheme position offers catalog scheme names

- **GIVEN** a document with `from: tim` (cursor at byte 10, inside the scheme token `tim`) and a catalog containing `timer`, `log`, `direct`
- **WHEN** `complete_at` is called with offset 10
- **THEN** the result includes `timer` (and `log`, `direct`) as completion candidates

#### Scenario: Option-key position offers declared options for the resolved scheme

- **GIVEN** a document with `from: timer:tick?per` (cursor at byte 19, inside `per`) and a catalog whose `timer` entry declares option `period`
- **WHEN** `complete_at` is called with offset 19
- **THEN** the result includes `period` as a completion candidate

#### Scenario: Minimal-metadata scheme returns empty option-key completions

- **GIVEN** a document with `from: redis:cache?op` (cursor inside `op`) and a catalog whose `redis` entry is `minimal` (no `uri_options`)
- **WHEN** `complete_at` is called at that offset
- **THEN** the result is an empty list (no panic, no error)

#### Scenario: Cursor outside any URI returns an empty list

- **GIVEN** a document where the cursor is in a non-URI region (e.g. a YAML key like `steps:`)
- **WHEN** `complete_at` is called at that offset
- **THEN** the result is an empty list

#### Scenario: Offset beyond source length returns an empty list

- **GIVEN** a 20-byte document
- **WHEN** `complete_at` is called with offset 50
- **THEN** the result is an empty list (no panic)

### Requirement: Engine provides hover information at a byte offset

The `LintEngine` SHALL expose `hover_at(&self, doc: &Document, offset: usize) ->
Option<HoverInfo>`. The engine locates the option key at the cursor offset within
a URI query string, resolves it against the catalog, and returns a `HoverInfo`
struct carrying: the option's `description` (if present), the `deprecated` reason
(if the option is deprecated), and the `secret` flag (if the option is marked
secret). Returns `None` when the cursor is not on an option key, when the scheme
has no metadata, or when the option is unknown.

#### Scenario: Hover on a documented option returns its description

- **GIVEN** a document with `from: timer:tick?period=1s` and a catalog whose `timer` option `period` has `description = Some("Tick interval")`
- **WHEN** `hover_at` is called with an offset inside the `period` key
- **THEN** the result is `Some(HoverInfo { description: Some("Tick interval"), .. })`

#### Scenario: Hover on a deprecated option returns the deprecation reason

- **GIVEN** a document using option `oldFreq` and a catalog where `oldFreq` has `deprecated = Some("use \`period\` instead")`
- **WHEN** `hover_at` is called with an offset inside `oldFreq`
- **THEN** the result carries the deprecation reason in its `deprecated` field

#### Scenario: Hover on a secret option returns the secret flag

- **GIVEN** a document using option `password` and a catalog where `password` has `secret = true`
- **WHEN** `hover_at` is called with an offset inside `password`
- **THEN** the result has `secret = true`

#### Scenario: Hover outside any option key returns None

- **GIVEN** a document where the cursor is in the scheme part or outside any URI
- **WHEN** `hover_at` is called at that offset
- **THEN** the result is `None`

# Design: audit-fix-docdrift-lint

## Approach

Add a fourth lint subcommand to the existing monolithic xtask
(`scripts/xtask/src/main.rs`), following the established pattern of
`lint-unwrap` / `lint-log-levels` / `lint-non-exhaustive`. No new crate. The
`quote` workspace dep (already in the root `Cargo.toml`) is added to the xtask
crate for `ToTokens` impl-block stringification in Rule B. The lint walks the
57 `CONTEXT.md` files plus the root `CONTEXT-MAP.md`, applies four rules,
and reports `Violation` records through the same
`Result<Vec<Violation>, String>` channel the other lints use.

The symbol-validation rule uses `syn` (already an xtask dependency with the
`full` feature, and already used by the existing `lint-secrets` /
`lint-non-exhaustive` helpers) to parse each crate's source into an AST. This
gives accurate `impl`-block resolution (including trait impls, generics,
comments, and raw strings) without brittle brace-counting heuristics. A
CONTEXT.md validates its cited symbols against its own crate's `src/`
directory (the directory sibling to the CONTEXT.md), so symbol lookups are
scoped and cheap. The root `CONTEXT-MAP.md` validates against all workspace
crates' `src/`.

## Affected files

- `scripts/xtask/src/lint_context_citations.rs` (new) — a sibling module
  (`mod lint_context_citations;` in `main.rs`, following the existing
  `changelog.rs` precedent) holding the lint function, the per-rule
  `check_*_src` extractors, the glossary cross-file pass, and inline
  `#[cfg(test)]` unit tests.
- `scripts/xtask/src/main.rs` (modified) — add `Commands::LintContextCitations`
  variant to the clap enum and a dispatch match arm that calls
  `lint_context_citations::lint_context_citations(&workspace_root)`.
- `AGENTS.md` — add the gate to `## QUALITY GATES`.
- `.github/workflows/ci.yml` — add the step alongside the other lint steps.

No production crate is modified. The xtask is build/dev tooling.

## Architecture boundaries

This change is confined to the xtask tooling crate and CI configuration. It
touches no `Service<Exchange>`, no `Component`/`Endpoint`/`Consumer` trait, no
runtime path, no canonical compilation. The lint is read-only over the source
tree.

## Rule specifications

### Rule A: cited file paths and anchors resolve

Markdown links `[text](path)` inside a CONTEXT.md — outside fenced code blocks —
SHALL resolve. Two checks:

1. **File existence.** The path portion (before any `#anchor`) SHALL exist.
   Resolution is deterministic: try relative to the CONTEXT.md's own directory
   first; if not found, try relative to the workspace root. External links
   (`http://`, `https://`, `mailto:`) and fragment-only links (`#section`,
   same-document) are excluded. Path traversal (`..` escaping the workspace
   root) is a violation.
2. **Anchor existence.** When a link to a `.md` file carries a `#anchor`, that
   anchor SHALL correspond to a heading in the target file. Anchor
   normalization algorithm (deliberately narrower than full GitHub spec, no
   Unicode/inline-markup handling, NO duplicate-heading suffix `-1`/`-2`): lowercase the heading text, trim, collapse
   runs of whitespace to a single hyphen, strip leading/trailing punctuation,
   drop characters that are not `[a-z0-9-]`. The anchor from the link undergoes
   the same normalization, then membership is checked against the set of
   normalized heading anchors parsed from the target file. Duplicate-heading
   disambiguation is a known v1 limitation (files with two identical headings
   may mismatch GitHub's rendered anchors).

An inline bare path (not in a markdown link) that looks like a workspace file
reference — matching `[\w/]+\.\w+` and containing a `/` — is also checked for
existence, again excluding fenced code blocks.

### Rule B: cited Rust symbols resolve to a definition in crate source

A backtick-quoted token in CONTEXT.md prose — outside fenced code blocks —
that matches a Rust definition pattern SHALL resolve to a matching definition.
The crate source is parsed with `syn::parse_file` into `Vec<syn::Item>`.
Recognized patterns and their lookup:

- `fn <ident>` → an `Item::Fn` whose `sig.ident` matches, OR an `ImplItem::Fn`
  inside any `Item::Impl` block, OR an `Item::Trait` method whose `sig.ident`
  matches. (Methods in impl and trait blocks are legitimate fn definitions.)
- `struct <Ident>` → an `Item::Struct` whose `ident` matches.
- `enum <Ident>` → an `Item::Enum` whose `ident` matches.
- `trait <Ident>` → an `Item::Trait` whose `ident` matches.
- `<Type>::<member>` (path fragment) → the lint first confirms `<Type>` is a
  defined struct/enum/trait. If `<Type>` is NOT found anywhere in the
  searchable source set, it is treated as an EXTERNAL type and the citation is
  SKIPPED. If `<Type>` IS found, the member is resolved in order: (a) enum
  variant of `<Type>` if it is an `Item::Enum`, (b) trait-def method if it is
  an `Item::Trait`, (c) method inside an `Item::Impl` whose self-type or
  trait-type is `<Type>`. If none match, a `[symbol]` violation is emitted.

**Source scope.** A crate CONTEXT.md resolves symbols against its own crate
`src/` first, then falls back to all workspace crate `src/` (cross-crate
prose references are legitimate — e.g. camel-processor citing a trait defined
in camel-api). The root `CONTEXT-MAP.md` validates against all workspace
crates' `src/`. Bare `impl <Ident>` citations (without a member) are out of
scope for v1.

Backtick tokens that do not match any of these patterns (config keys, CLI
flags, free prose) are NOT validated.

### Rule C: no line-number sole-locator citation

A line-number reference of the form `<file>.rs:<digits>` (or `:L<digits>`) in
CONTEXT.md prose is flagged **only when it is the sole locator** on that
reference — i.e. no accompanying symbol citation (one of the Rule-B
recognized patterns: `` `fn ...` ``, `` `struct ...` ``, `` `enum ...` ``,
`` `trait ...` ``, or `` `<Type>::<method>` ``) on the same line. A reference
that pairs a stable symbol with a supplemental line number ("see `fn foo` at
config.rs:80") is allowed — the symbol is the durable locator, the line
number is a convenience.

### Rule D: glossary ownership — no cross-file collisions

Glossary terms are collected only from **explicit glossary sections**: a
markdown heading line whose normalized title is EXACTLY one of `glossary`,
`key terms`, or `terminology` (case-insensitive, trimmed — prefix matches like
`## Glossary conventions` do NOT qualify) introduces a section. Every
`**<Term>:**` (bold, colon-terminated) line within that section is a glossary
term. The section terminates at the next heading of the same or higher level
(e.g. a `## Glossary` section ends at the next `## ` or `# ` heading).
`**Term:**` lines inside fenced code blocks are excluded.

Terms are normalized for comparison: lowercase, trim, collapse internal
whitespace, strip the trailing colon. `CONTEXT-MAP.md`'s Key Terms section is
included in the same collection pass (it is the workspace-level glossary).

When a normalized term appears in two or more files' glossary sections, the
lint emits one violation per extra owner. Owners are ordered deterministically
by file path (sorted); the first path is the canonical owner, subsequent
paths are the colliders named in the violation.

## Data flow

```
main → lint_context_citations(workspace_root)
         1. discover context files (recursive walk of crates/, examples/,
            benchmarks/, platforms/, plus the root CONTEXT-MAP.md; exclude
            target/, .worktrees/, archive/ dirs and hidden dirs)
         2. per-file: lint_context_citations_src(content, file_path, crate_src_dir)
            → rules A (path+anchor), B (symbol), C (line-number) — single-file checks
         3. cross-file: collect glossary-section terms → normalize → detect collisions (rule D)
         4. aggregate violations → return Vec<Violation>
```

Discovery is recursive over the workspace roots, excluding `target/`,
`.worktrees/`, `node_modules/`, hidden dirs (`.git`, `.beads`), and any
`archive/` directory. The current tree yields 58 files (57 `CONTEXT.md` +
`CONTEXT-MAP.md`); the count is baseline evidence, not a hard-coded limit.

The `_src` extractor takes the crate `src/` directory so it can perform symbol
lookups without re-discovering the workspace layout each call. This keeps the
extractor unit-testable: tests pass a synthetic `src/` dir or mock content.

## Violation reporting

Reuses the existing `Violation` struct (`{ file, line, snippet }` at ~L1262).
Each rule produces a snippet prefixed with its rule tag:
`[path]`, `[symbol]`, `[line-ref]`, `[glossary-collision]`. The dispatch arm
prints the violation list and exits non-zero on any finding, identical to the
lint-unwrap / lint-log-levels arms.

## Testing strategy

Inline `#[cfg(test)]` unit tests for the `_src` extractor, one pass-case and
one fail-case per rule (mirroring `lint_non_exhaustive_src`'s test layout).
Cross-file glossary collision is tested at the `lint_context_citations` level
with a temp directory containing two synthetic CONTEXT.md files. The existing
test conventions in main.rs (no external test framework, plain `assert!`) are
followed.

## Baseline expectation

D1 (rc-bwbg) closed the T1 doc-drift baseline. The lint's first run over the
58 context files is expected to find a residual (dangling anchors, stale
symbols, or glossary collisions that pre-date this gate). Because the CI gate
cannot be enabled over a dirty tree, any violation discovered is fixed
in-scope as an additional task within this change — either by correcting the
CONTEXT.md citation or, when the CONTEXT.md is accurate and the lint is
over-matching, by tightening the rule. The change lands only when
`cargo xtask lint-context-citations` exits clean.

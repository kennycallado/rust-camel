# lint-context-citations Specification

## Purpose
TBD - created by archiving change audit-fix-docdrift-lint. Update Purpose after archive.
## Requirements
### Requirement: Cited file paths and anchors in CONTEXT.md resolve

A CONTEXT.md that cites a file path — as a markdown link `[text](path)` or as
an inline workspace path — SHALL reference a file that exists. When the link
carries a `#anchor`, that anchor SHALL correspond to a heading in the target
markdown file. External links (`http://`, `https://`, `mailto:`) and
fragment-only links (`#section`) are excluded from validation.

#### Scenario: markdown link target exists

- **GIVEN** a CONTEXT.md contains a markdown link `[config module](./src/config.rs)`
- **WHEN** the lint resolves the link target relative to the CONTEXT.md directory
- **THEN** the file `./src/config.rs` exists and no violation is emitted

#### Scenario: dangling path reference is flagged

- **GIVEN** a CONTEXT.md cites a path `./src/old_module.rs` that no longer exists
- **WHEN** the lint resolves the path
- **THEN** a violation tagged `[path]` is emitted naming the CONTEXT.md file
  and the dangling reference

#### Scenario: anchor resolves to a heading in the target file

- **GIVEN** a CONTEXT.md links to `[errors](./error.md#not-found-variant)` and
  `error.md` contains a heading `## Not Found Variant`
- **WHEN** the lint normalizes the heading to anchor form and checks membership
- **THEN** the anchor resolves and no violation is emitted

#### Scenario: dangling anchor is flagged

- **GIVEN** a CONTEXT.md links to `[errors](./error.md#removed-section)` but
  `error.md` has no heading matching `removed-section`
- **WHEN** the lint checks the anchor
- **THEN** a violation tagged `[anchor]` is emitted naming the missing anchor
  and target file

#### Scenario: external and fragment-only links are excluded

- **GIVEN** a CONTEXT.md contains `[docs](https://example.com/docs)` and
  `[local](#section)`
- **WHEN** the lint evaluates the links
- **THEN** no path or anchor validation runs on either (external scheme;
  same-document fragment)

#### Scenario: path traversal is rejected

- **GIVEN** a CONTEXT.md links to `[secret](../../../etc/passwd)`
- **WHEN** the lint resolves the path
- **THEN** a violation tagged `[path]` is emitted because `..` escapes the
  workspace boundary

### Requirement: Cited Rust symbols resolve to a definition in crate source

A backtick-quoted token in CONTEXT.md prose — outside fenced code blocks —
that matches a Rust definition pattern (`fn <ident>`, `struct <Ident>`,
`enum <Ident>`, `trait <Ident>`, `<Type>::<member>`) SHALL resolve to a
matching definition in the CONTEXT.md's crate `src/` or, if not found there,
in any workspace crate `src/` (cross-crate prose references are legitimate).
The root `CONTEXT-MAP.md` validates symbols across all workspace crate `src/`.
For `<Type>::<member>`, the member SHALL appear as an enum variant of `<Type>`,
a method in a trait definition for `<Type>`, or a method inside an `impl`
block whose self-type or trait-type is `<Type>` — a member on a different type
does not satisfy the citation. When `<Type>` is not found in any searchable
workspace source, it is treated as an external type and the citation is
SKIPPED (a known false negative: a misspelled workspace type is
indistinguishable from an external type and is not flagged).

#### Scenario: cited function exists in crate src

- **GIVEN** a CONTEXT.md cites `` `fn process_exchange` `` and the crate
  `src/` contains `fn process_exchange`
- **WHEN** the lint searches the crate source for the definition
- **THEN** the symbol resolves and no violation is emitted

#### Scenario: cited struct that was renamed is flagged

- **GIVEN** a CONTEXT.md cites `` `struct RouteConfig` `` but the struct was
  renamed to `RouteDslConfig` and no `RouteConfig` definition remains
- **WHEN** the lint searches the crate source
- **THEN** a violation tagged `[symbol]` is emitted naming the unresolved
  symbol

#### Scenario: Type::method resolves when the method is on that type

- **GIVEN** a CONTEXT.md cites `` `RouteErrorHandler::handle` `` and the crate
  `src/` contains `impl RouteErrorHandler` with `fn handle` inside it
- **WHEN** the lint confirms the type exists and the method is inside its impl
- **THEN** the symbol resolves and no violation is emitted

#### Scenario: Type::method resolves when the method is in a trait impl for that type

- **GIVEN** a CONTEXT.md cites `` `LazyJmsProducer::poll_ready` `` and the crate
  `src/` contains `impl Service<Exchange> for LazyJmsProducer { ... fn poll_ready ... }`
- **WHEN** the lint checks the brace-bounded impl block whose header mentions
  `LazyJmsProducer`
- **THEN** the symbol resolves and no violation is emitted

#### Scenario: Type::method is flagged when the method is on a different type

- **GIVEN** a CONTEXT.md cites `` `CamelContext::poll_ready` `` but `poll_ready`
  exists only inside `impl RouteChannelService`, not any impl mentioning
  `CamelContext`
- **WHEN** the lint checks the impl association
- **THEN** a violation tagged `[symbol]` is emitted

#### Scenario: external type citation is skipped

- **GIVEN** a CONTEXT.md cites `` `DynamicMessage::decode` `` and
  `DynamicMessage` is not defined in any workspace crate (it is a prost type)
- **WHEN** the lint searches for the type definition
- **THEN** the type is treated as external and no violation is emitted

#### Scenario: root CONTEXT-MAP.md validates symbols against all workspace crates

- **GIVEN** `CONTEXT-MAP.md` cites `` `fn compile_declarative_route_to_canonical` ``
  which lives in `crates/camel-builder/src/`
- **WHEN** the lint scopes the symbol search
- **THEN** the search covers all workspace crate `src/` directories (not a
  single crate) and the symbol resolves

#### Scenario: non-symbol backtick token is not validated

- **GIVEN** a CONTEXT.md contains `` `config.watch` `` (a config key, not a
  Rust definition pattern)
- **WHEN** the lint evaluates the token
- **THEN** no violation is emitted because the token does not match a Rust
  definition pattern

#### Scenario: backtick tokens inside fenced code blocks are ignored

- **GIVEN** a CONTEXT.md contains a fenced code block with `` `struct Demo` ``
  as example text
- **WHEN** the lint scans
- **THEN** no symbol validation runs on the code-block content

### Requirement: Line numbers are not used as the sole citation locator

A line-number reference of the form `<file>.rs:<digits>` (or `:L<digits>`) in
CONTEXT.md prose SHALL be flagged as a violation when it is the sole locator
on that line — i.e. no accompanying symbol citation matching a Rule-B
recognized pattern (`fn <ident>`, `struct <Ident>`, `enum <Ident>`,
`trait <Ident>`, `<Type>::<method>`). A reference that pairs a stable symbol
with a supplemental line number is allowed. Fenced code blocks and table rows
are excluded.

#### Scenario: symbol citation with no line number passes

- **GIVEN** a CONTEXT.md cites `` `fn run_steps` `` (no line number)
- **WHEN** the lint scans the prose
- **THEN** no line-number violation is emitted

#### Scenario: bare line-number citation is flagged

- **GIVEN** a CONTEXT.md prose line reads "see config.rs:80 for the field"
  with no backtick symbol on that line
- **WHEN** the lint scans the prose (outside code blocks and tables)
- **THEN** a violation tagged `[line-ref]` is emitted

#### Scenario: symbol plus supplemental line number is allowed

- **GIVEN** a CONTEXT.md prose line reads "see `` `fn foo` `` at config.rs:80"
- **WHEN** the lint scans the prose
- **THEN** no violation is emitted because the line carries a stable symbol
  locator alongside the line number

#### Scenario: line numbers inside code blocks are ignored

- **GIVEN** a CONTEXT.md contains a fenced code block with `error.rs:42` as
  example output
- **WHEN** the lint scans
- **THEN** no violation is emitted because the reference is inside a code
  block

### Requirement: Glossary terms are owned by at most one context file

Glossary terms — `**<Term>:**` bold-colon headings inside an explicit
`Glossary` / `Key Terms` / `Terminology` section — SHALL be owned by at most
one context file across the workspace (including the root `CONTEXT-MAP.md`).
Bold labels outside glossary sections are not tracked. Terms are compared in
normalized form (lowercase, trimmed, internal whitespace collapsed, trailing
colon stripped).

#### Scenario: unique glossary term passes

- **GIVEN** only `crates/camel-api/CONTEXT.md` defines `**Exchange:**` in its
  Glossary section
- **WHEN** the lint collects all glossary-section terms across the workspace
- **THEN** no collision violation is emitted

#### Scenario: term owned by two files is flagged

- **GIVEN** both `crates/camel-builder/CONTEXT.md` and `CONTEXT-MAP.md` define
  `**Canonical Route Spec:**` in their glossary sections
- **WHEN** the lint detects the normalized duplicate
- **THEN** a violation tagged `[glossary-collision]` is emitted on the second
  owner (by sorted path) naming the first owner for triage

#### Scenario: bold label outside a glossary section is not tracked

- **GIVEN** a CONTEXT.md Q&A block contains `**Questions generated:**` and
  `**Outcome:**` outside any Glossary/Key Terms heading
- **WHEN** the lint collects glossary terms
- **THEN** those labels are ignored (not treated as glossary terms)

#### Scenario: normalized term collision is detected

- **GIVEN** one file defines `**Exchange:**` and another defines
  `**exchange :**` (differing in case and spacing)
- **WHEN** the lint normalizes both to `exchange` and compares
- **THEN** a collision violation is emitted

#### Scenario: prefix-heading does not open a glossary section

- **GIVEN** a CONTEXT.md contains `## Glossary conventions` (not an exact
  `Glossary` title) followed by `**Term:**`
- **WHEN** the lint evaluates the heading
- **THEN** the section is NOT treated as a glossary section and `**Term:**` is
  not collected

#### Scenario: glossary section terminates at the next same-level heading

- **GIVEN** a CONTEXT.md has `## Glossary` with `**Foo:**`, then later `## Notes`
  with `**Foo:**` outside any glossary section
- **WHEN** the lint collects terms
- **THEN** only the `**Foo:**` under `## Glossary` is collected; the one under
  `## Notes` is ignored

#### Scenario: bold labels inside fenced code blocks are ignored

- **GIVEN** a glossary section contains a fenced code block with `**FakeTerm:**`
  inside it
- **WHEN** the lint collects glossary terms
- **THEN** the fenced `**FakeTerm:**` is not collected

### Requirement: lint-context-citations is a CI quality gate

`cargo xtask lint-context-citations` SHALL be registered as a quality gate in
`AGENTS.md ## QUALITY GATES` and as a step in `.github/workflows/ci.yml`,
alongside the existing lint-unwrap, lint-log-levels, lint-secrets, and
lint-non-exhaustive gates. The gate SHALL run on every CI build.

#### Scenario: gate appears in AGENTS.md

- **GIVEN** the QUALITY GATES block in AGENTS.md lists the lint commands
- **WHEN** a reader scans the gate list
- **THEN** a `lint-context-citations` entry is present with its
  `cargo xtask lint-context-citations` invocation

#### Scenario: gate runs in CI

- **GIVEN** the CI workflow file `.github/workflows/ci.yml` defines the
  quality-gate job
- **WHEN** CI runs
- **THEN** a step executes `cargo xtask lint-context-citations` and fails the
  build on any violation


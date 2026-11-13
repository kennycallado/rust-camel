# Proposal: scriptfile

## Why

Every script is an inline string today: `LanguageExpressionDef { language, source }`
(`crates/camel-api/src/declarative.rs`), accepted by the YAML/JSON DSL as the
counted one-of `{language+source, simple, rhai, jsonpath, xpath}`
(`crates/camel-dsl/src/yaml.rs`). Inline scripts get no IDE support — no
`.rhai`/`.js` extension means no highlighting, lint, format, or
go-to-definition — and `source: |` block scalars make diffs unreadable. The
same script cannot be shared by two routes. Loading scripts from their own
files fixes all four at once.

bd: rc-bqyc6.

## What Changes

- `source_file:` on every DSL expression object (script step, predicate
  blocks, loop-while, value-source expressions), mutually exclusive with
  `source:`; exactly one source form is required. Failure modes are named
  errors that include the resolved path.
- Language inference from the extension when `language:` is omitted:
  `.rhai`, `.js`, `.jsonpath`, `.xpath`, `.jinja`/`.jinja2`.
  Explicit `language:` always wins.
- Resolution: relative paths resolve against the directory of the referencing
  route file (YAML/JSON); the Rust DSL resolves against the process CWD;
  absolute paths are allowed. Files are read once at load, verbatim; empty
  files are rejected.
- `camel run --watch` reloads when a referenced script changes: resolved
  script paths join the watch set, and script contents fold into the route
  document's source hash so a script-only edit swaps the route instead of
  being skipped.
- Rust DSL: `LanguageExpressionDef::file(path)` / `::file_with_language(path,
  language)` constructors next to the struct literal.
- Contract: `schemas/dsl/route-schema.json` + `schemas/ts` regenerated;
  YAML↔JSON parity tests cover the new field in both formats.
- Docs: the script guide pages gain the file form plus a sentence on IDE
  support; the `camel new` basic template gains a root-level
  `scripts/hello.rhai` referenced from the route with
  `source_file: ../scripts/hello.rhai`.

Not included: templating or env interpolation inside script files; include
graphs between scripts; confinement rules on `..`/symlinks (route scripts are
project source, not dynamic assets).

## Acceptance criteria

- A route file referencing `source_file: scripts/hello.rhai` (no `language:`)
  loads, infers `rhai`, and evaluates the file's verbatim contents.
- Missing file, empty file, both `source` and `source_file`, and unknown
  extension without `language:` each fail with named errors that include the
  resolved path.
- Editing only the script file under `camel run --watch` triggers a route
  reload (a load test proves the watch set contains the resolved path).
- `cargo xtask schema --check`, parity tests, `mdbook test`, and the mission
  gate table all pass.

## Risk budget

- `LanguageExpressionDef` stays `{language, source}` — resolution happens at
  load time, so the declarative contract and its ~100 struct-literal sites do
  not change.
- The public `camel-dsl` AST structs (`ScriptData`, predicate/loop-while
  blocks) gain `source_file` and narrow two fields to `Option` — a pre-1.0
  breaking change for exhaustive struct literals (ADR-0090); documented in
  `design.md` and flagged with `!` on the squash commit.
- Watch integration reuses the existing dev-target machinery (registry +
  classification); no new watcher subsystem.

## Affected crates

camel-api, camel-dsl, camel-core (hot-reload), camel-cli, schemas, docs.

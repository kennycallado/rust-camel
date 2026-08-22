# Lint

Runtime-free route diagnostics engine. Parses a route source and runs five lint rules
against a `ComponentMetadataCatalog`. Produces a flat list of `Diagnostic` values.
Strictly outside the runtime and DSL — depends on `camel-api` (contract types + catalog
trait), never on `camel-core`, `camel-dsl`, or `camel-cli`.

Dependencies: `camel-api`, `noyalib`, `jsonschema`, `ariadne`, `serde`, `thiserror`.

## Architecture

**LintEngine** is stateless — a `Vec<Box<dyn Rule>>` plus an `Arc<dyn ComponentMetadataCatalog>`.
`lint(source: &str) -> Vec<Diagnostic>` parses the source, runs every rule, and returns
diagnostics sorted by span position.

**Editor support**: the same engine powers `complete_at` / `hover_at` for camel-lsp and the
lint CLI. Completion covers scheme position, query-string option keys, and `parameters:`
entry keys (catalog options minus query-declared keys, because the DSL lowering rejects
the overlap) and values (enum variants, bool literals, or the default value). Hover
resolves an option key in either the query string or a `parameters:` entry to its catalog
metadata (description, deprecation, secret flag).

**Rules** implement the `Rule` trait: `analyze(doc: &Document, catalog: &dyn ComponentMetadataCatalog) -> Vec<Diagnostic>`.
The engine ships with 5 rules:

| Code | Rule | Severity | Description |
|------|------|----------|-------------|
| R-SYN | Syntax | Error | YAML/JSON parse failure; `RSynRule` reads the `parse_failure` field of `Document` (set by `Document::parse`) and emits one diagnostic |
| R-SCHEMA | JSON Schema | Error | Validates the parsed document against the embedded route-schema.json, with per-keyword error anchoring |
| R-URI-known | URI known | Error / Info | Validates endpoint schemes and options against the catalog; unknown scheme → Info, unknown option / kind mismatch / missing required / duplicate key across query/parameters → Error |
| R-SECRET | Secret | Warning | Detects literal credentials (passwords, tokens, API keys) in route source |
| R-DEPRECATED | Deprecated | Warning | Flags deprecated component options |

**Document** wraps `noyalib` parse output with `parse_failure`, the route view (`LintRoute` /
`Endpoint` / `LintOption`), and an `apply_fix` hook.

**Endpoint options come from three sources** — query string, `parameters:` maps, and (for
object-form `enrich`/`poll_enrich`) the inner config map. Each captured option carries its
source origin (`Query` / `StepParameters` / `ConfigParameters` on `LintOption.origin`),
distinguishable by rules; every option is still attached to the same endpoint and validated
identically by the per-occurrence rules. `parameters:` entries are captured with byte-exact
key/value spans. Step-level `parameters:` inherit into object-form URI keys and are
CONCATENATED with the inner map (never either/or — dropping a side would hide entries from
rules and false-flag `MissingRequiredOption`). A same key declared in more than one source
(query string, step parameters, or config parameters) fails the DSL lowering
(fail-closed, `EndpointUriError::DuplicateKey`); R-URI-known flags it pre-lowering with
`R-URI-known:duplicate-key` on the redundant occurrence. Repeated keys within the raw
query string alone stay legal.

## Catalog injection

`LintEngine` takes `Arc<dyn ComponentMetadataCatalog>` at construction. The production catalog
is built by `register_builtin_components_for_lint` in the `camel lint` CLI subcommand
(camel-cli). This function creates a handle-free catalog: it registers component metadata
without starting any runtime, so the lint tool stays runtime-free.

## Zero-false-positives gate

A corpus integration test at `crates/camel-cli/tests/lint_corpus.rs` runs the engine over
a set of route fixtures and compares the output against a checked-in baseline in RON format.
The assertion is set-equality: every diagnostic in the baseline must be produced, and every
produced diagnostic must be in the baseline. One-to-one positional matching is too fragile
for diagnostics that may shift spans.

## Language

**LintEngine**:
Stateless engine that owns a rule list and a catalog reference. `lint(source)` returns
`Vec<Diagnostic>`. No mutable state, no caching.
_Avoid_: linter, analyser, checker

**Rule**:
Trait with one method: `analyze(doc, catalog) -> Vec<Diagnostic>`. Each rule is a struct
that implements this trait. Rules are composable and order-independent.
_Avoid_: lint rule, check, validator (use Rule for the trait)

**Document**:
Parsed route source with `parse_failure: Option<ParseError>`, route view, and `apply_fix`
hook. R-SYN failures produce a document with `parse_failure` set; other rules skip
documents that failed to parse (R-SYN owns the syntax error).
_Avoid_: source file, input

**Diagnostic**:
A single lint finding: `code` (the `DiagnosticCode`), `severity` (Error / Warning / Info),
`span` (source range), `message`, and an optional `fix`.
_Avoid_: issue, warning, finding

**DiagnosticCode**:
Enum of all possible lint codes: `RSyntax`, `RSchema(RSchemaSubCode)`, `RUriKnown(UriKnownSubCode)`,
`RSecret(RSecretSubCode)`, `RDeprecated(RDeprecatedSubCode)`. Each variant names the rule
that produced it. `R-URI-known:<sub>` stable strings: `unverified-scheme`, `unknown-option`,
`kind-mismatch`, `missing-required-option`, `duplicate-key`.
_Avoid_: error code, lint code (use DiagnosticCode for the enum)

**Severity**:
Error (R-SYN, R-SCHEMA, R-URI-known errors), Warning (R-SECRET, R-DEPRECATED),
Info (`UnverifiedScheme`).

**Fix**:
Optional suggested edit with a replacement string and the span to replace. `Document::apply_fix(&fix)`
applies a single `Fix`: it substitutes the replacement into the span, re-parses, and on a
syntax-breaking edit returns `Err(LintError::Internal(..))` and leaves the document unchanged.

**LintRoute / Endpoint / LintOption**:
The route view extracted from the parsed document. `LintRoute` holds endpoint URIs;
`Endpoint` holds a parsed URI + option list; `LintOption` holds a key-value pair with
source spans. Used by R-URI-known, R-SECRET, and R-DEPRECATED to walk route structure.
_Avoid_: route AST, parsed route (use route_view or LintRoute for the struct)

**ComponentMetadataCatalog**:
Trait from `camel-api` that provides component metadata by scheme. The lint engine
queries it through `Arc<dyn ComponentMetadataCatalog>`. R-URI-known is the primary
consumer; R-DEPRECATED also checks it for deprecation notices.
_Avoid_: metadata store, registry (use ComponentMetadataCatalog for the trait)

**unverified-scheme**:
An informational `UnverifiedScheme` diagnostic emitted when a route uses a scheme not
registered in the catalog. Not an error: the file might be valid but the catalog
is incomplete. Does not suppress other diagnostics for the same endpoint.
_Avoid_: unknown scheme, missing component

**Zero-false-positives gate**:
The corpus integration test + RON baseline assertion. Every diagnostic the engine
produces must match the baseline, and the baseline must not contain diagnostics the
engine does not produce. Set-equality, not positional. The baseline is checked in;
any change to rule behaviour must update it.
_Avoid_: snapshot test, golden file (the baseline is RON, not a snapshot of raw output)

## `resolve_option` semantics (open namespace)

The shared helper `resolve_option` matches a URI query key against a scheme's
`UriOption` list in two phases:

1. **Phase 1 — exact-name or alias match.** Considers only options whose
   `pattern` field is `None`. The first option whose `name` equals the key, or
   whose `aliases` contains the key, wins. Pattern options do not participate.
2. **Phase 2 — pattern match.** Considers only options whose `pattern` field is
   `Some(_)`. Options are ordered by **descending separator length** (longest
   prefix wins). The first option whose `Prefix.separator` the key starts with,
   AND whose remaining suffix is non-empty, wins. A bare `param.` key does NOT
   match a `Prefix { separator: "param." }` option.

If neither phase produces a match, the key is an `UnknownOption` diagnostic.

This two-phase order ensures that a discrete option named `param.foo` wins over
a pattern option with separator `param.` for the key `param.foo` (exact-name
match at Phase 1 before pattern match at Phase 2). Among overlapping patterns,
the longest separator wins (e.g. `param.foo.bar` matches `param.foo.` over
`param.`).

## Example dialogue

> "Does camel-lint need a running CamelContext?"
> "No. The engine takes a catalog trait object but does not start any runtime.
> The production catalog is built handle-free by `register_builtin_components_for_lint`."

> "Why does R-URI-known only check Bool kind?"
> "v1 scope. Other `OptionKind` variants (String, Int, Float, Duration, Enum, List)
> are deferred — validating them needs format-specific parsers. The `#[non_exhaustive]`
> attribute means unknown future kinds are also non-erroring."

> "How do I add a new lint rule?"
> "Implement the `Rule` trait and register it in `LintEngine::with_rule`. Add the
> corresponding `DiagnosticCode` variant and sub-code enum. Update the corpus baseline."

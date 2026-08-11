# Proposal: open-uri-param-namespace

## Why

The `UriConfig` derive macro generates one `UriOption` per `#[uri_param]` field, but it
cannot express OPEN namespaces like `param.<name>=<value>` where any key is valid. Two
components — `camel-xj` and `camel-xslt` — accept stylesheet parameters via
`param.foo=bar&param.baz=qux` pairs into a `Vec<(String, String)>` field. Their
`Component::metadata()` returns `ComponentMetadata::minimal(scheme)` with empty
`uri_options`, so the lint silently NO-OPs for them (graceful-degradation gate: empty
`uri_options` → no validation).

The canonical spec
(`openspec/specs/endpoint-metadata-derivation/spec.md`, requirement "Per-Component
disposition for query-minimal and namespace-blocked") records both components as
`schema-blocked-deferred` until "the macro/catalog support open-ended namespaces". This
change delivers that capability.

Reference: bd `rc-2s7g` (supersedes `rc-nkuf`). Unblocks two of the ~17 connectors
tracked by `rc-qbdt`.

## What Changes

**Included (Change 1 — capability only):**

- New `UriOptionMatch` enum (`#[non_exhaustive]`) in `camel-api`, with one variant:
  `Prefix { separator: String }`.
- New optional field `pattern: Option<UriOptionMatch>` on `UriOption`, serialized with
  `skip_serializing_if = "Option::is_none"` (byte-identical existing JSON).
- New builder `UriOption::pattern_prefix(separator)` (consuming builder, mirrors
  `secret`/`required`/`deprecated` shape).
- New `#[uri_param(pattern = "param.")]` macro key, valid only on
  `Vec<(String, String)>` fields, with compile-time guardrails (incompatible with
  `required`, `default`, `secret`, `name`, `aliases`, and any non-`String` `kind`;
  empty separator rejected).
- `resolve_option` (the shared lint funnel) extended to match `Prefix` with a non-empty
  suffix. Discrete options win over pattern options; among multiple matching patterns,
  the longest separator wins.
- ADR-0041 amendment + CONTEXT-MAP Key Terms + per-crate CONTEXT.md updates
  (camel-api, camel-endpoint, camel-lint).

**Excluded (Change 2 — authoring, follow-up):**

- Migration of `camel-xj` and `camel-xslt` to `#[derive(UriConfig)]` with `skip_impl`.
- Lint corpus baseline update.
- Any change to `camel-xj` or `camel-xslt` source, fixtures, or tests. Change 1 MUST
  leave both components byte-identical.

## Acceptance criteria

- `cargo build --workspace` clean; `cargo test --workspace --lib` green.
- `cargo test -p camel-cli --test lint_corpus` green with **unchanged baseline** — this
  is the gate that proves Change 1 did not drag in Change 2 work.
- `cargo fmt --check` and `cargo clippy --workspace -- -D warnings` green.
- `cargo xtask schema --check` green after regenerating and committing
  `schemas/component-metadata.json` (the schema gains the optional `pattern` field and
  the `UriOptionMatch` definition).
- `cargo xtask lint-non-exhaustive` confirms `UriOptionMatch` carries `#[non_exhaustive]`.
- `camel-lint` `resolve_option` matches `param.foo` against a
  `Prefix { separator: "param." }` option, rejects bare `param.` (empty suffix), and
  resolves a discrete-name collision in favor of the discrete option.
- An `#[uri_param(pattern = "param.")]` on a non-`Vec<(String, String)>` field fails to
  compile.
- `UriOption` JSON serialization stays byte-identical for options without `pattern`.
- No source, test, or fixture changes in `crates/components/camel-xj/` or
  `crates/components/camel-xslt/` (verified by `git diff --stat`).

## Risk budget

**Acceptable:**

- Additive schema-gen surface (one optional field, one new `#[non_exhaustive]` enum) —
  the JSON schema artifact MUST be regenerated and committed; `schema-check` enforces
  this.
- One new `#[uri_param]` key with strict compile-time guardrails.
- Extension of one shared lint helper (`resolve_option`) — fixes `ruriknown`, `rsecret`,
  and `rdeprecated` coherently in one edit.

**Out of bounds:** migration of any component (Change 2); changes to runtime config
parsing; new `OptionKind` variants; breaking changes to `UriOption` JSON; bumping
`schema_version` (it stays at 1 because the catalog is harvested in memory and the new
field is optional-and-skippable).

**Acknowledged trade-off:** the typed `UriOptionMatch` enum stabilizes the matcher shape
and Rust-side evolution (`#[non_exhaustive]`), but each future variant (e.g. `Glob`,
`Regex`) expands a closed JSON-Schema union. Stale validators or exhaustive generated
consumers may reject the new variant despite the Rust-side forward-compat guarantee, so
each future variant requires schema compatibility review and regenerated downstream
consumers. This cost is preferable to the alternatives rejected in `design.md`, but it
is not zero.

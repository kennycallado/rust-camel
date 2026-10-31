# Proposal: 471-missing-value-semantics

## Why

A missing value renders differently in each expression language. `{{ headers.missing }}`
in MiniJinja fails the render (strict undefined, `engine.rs` `UndefinedBehavior::Strict`);
`${header.missing}` in Simple yields an empty string in a mixed template and `null` as a
whole slot; `header("missing")` in Rhai yields UNIT. The same divergence shows up in
conditionals. This is user-visible, undocumented per-language behavior (bd rc-fyxwh,
e_opus finding). Users cannot predict whether a typo in a header name silently renders
empty output or fails the route — the two failure modes have opposite operational risk.

## Decision (per e_opus mission order)

Keep MiniJinja strict (fail-closed). Do NOT silently make MiniJinja lenient by default
and do NOT add a configurable lenient-print mode in this change. Simple's empty-string /
null rendering and Rhai's UNIT are documented as intentional. The contract is made
normative in the `language-value-boundary` spec and pinned with tests in all three
language crates.

## What Changes

- Tests pinning, for each of Simple, Rhai, MiniJinja:
  - interpolation of a missing value (`{{ headers.missing }}` / `${header.missing}` /
    `header("missing")`);
  - conditional use of a missing value (`{% if headers.missing %}` / a bare
    `${header.missing}` predicate / Rhai `if` and predicate on a missing value);
  - the documented opt-out: MiniJinja `default` filter / `is defined` test under strict
    undefined (verified empirically, pinned only if it works).
- One authoritative docs page (`docs/src/languages/value-boundary.md`) with a
  three-language table: missing-value, null-vs-empty, and error behavior; cross-linked
  from the Simple, Rhai, and MiniJinja language pages and the languages index.
- A delta spec on `language-value-boundary`: the missing-value contract becomes
  normative.

## Explicitly excluded

- No production code changes. `UndefinedBehavior::Strict` stays the default and only
  behavior (non-goal: changing minijinja strict undefined by default).
- No new filters/functions, no route-template placeholder changes, no external template
  component filter changes.
- No lenient-print mode (would require security review; parked as a possible follow-up).

## Acceptance criteria

- `cargo test -p camel-language-simple -p camel-language-rhai -p camel-language-minijinja`
  passes with the new pinning tests.
- `cargo fmt --check --all`; clippy `-D warnings` for the touched crates (all targets);
  usual xtask lints; `cd docs && mdbook test`; `openspec validate --all --strict`.
- Docs page renders with the table; each language page links to it.
- The change dir is left un-archived for master landing (fleet-parked mission).

## Risk budget

Low: tests + docs + spec delta only. Risk is documentation drift if a pinning test
asserts behavior the docs do not match — mitigated by deriving each docs table row from
a pinned test named in the spec scenarios. No runtime, DSL, or component surface is
touched.

Bd: rc-fyxwh (P2 bug, claimed). Affected crates: camel-language-simple,
camel-language-rhai, camel-language-minijinja (tests only), docs.

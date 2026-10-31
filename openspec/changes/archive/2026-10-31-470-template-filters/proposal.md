# Proposal: 470-template-filters

## Why

The external template component (ADR-0047 Stage 2) intentionally registers no
filters, functions, or globals of its own in its MiniJinja environment. Templates migrated
from Eleventy/Nunjucks commonly use two filters — `truncate` and `dump` — so a
straight port breaks at render time. Separately, MiniJinja's built-in HTML
escaper escapes `/` as `&#x2f;`, which surprises migratees but is correct and
safe. The component needs a narrow, documented answer for both: a two-filter
allowlist and a documented escaping policy (bd rc-6ckmb).

## What Changes

- `camel-template`'s compiled environment (`template_set.rs`) registers
  exactly two new component-owned filters and nothing else of its own
  (MiniJinja's engine built-in filters, including `safe`, are part of the
  locked engine and stay untouched):
  - `truncate(length, end="…")` — Unicode-scalar-safe truncation; only a
    non-negative integer `length` renders (negative, non-integral, boolean,
    and non-numeric values fail as render errors); `end` is appended only
    when truncation actually happens; no HTML awareness.
  - `dump` — deterministic JSON serialization; values that cannot be
    serialized fail as render errors. No code execution, no host access.
- No component-owned functions or globals. No custom escaper: MiniJinja's
  built-in escaping, including `/` → `&#x2f;`, stays. Docs document the
  behavior and the safe migration options (`|safe` only for trusted content,
  pre-rendered values).
- Docs updated: `crates/components/camel-template/README.md`,
  `docs/src/components/template.md`, `docs/src/languages/minijinja.md` (the
  "no filters by design" statement), and the crate CONTEXT term.
- Inline `language: minijinja` is unchanged: the inline engine
  (`camel-language-minijinja`) is a separate code path with its own
  environment construction; it gains no component filters.

## Acceptance criteria

- `cargo test -p camel-template` passes, including new TDD tests for truncate
  (short/no-op, long multi-byte, custom end, invalid length), dump
  (object/array/scalar, unserializable error), the `/` → `&#x2f;` escaping
  assertion, and an allowlist assertion (unknown filters still fail).
- `cargo fmt --check --all`; clippy `-D warnings` for camel-template
  (all-targets); usual xtask lints; docs build;
  `openspec validate --all --strict`.

## Risk budget

Low. The change is additive inside one compile-time registration site. No
escaping weakening, no arbitrary host functions, no sandbox/confinement
changes. Rejected scope: eval-style filters, render diagnostics, external
template root/reload changes, inline Simple/Rhai behavior.

Bd: rc-6ckmb

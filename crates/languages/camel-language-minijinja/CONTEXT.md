# MiniJinja template rendering language

MiniJinja (Python Jinja2-inspired) template rendering implementation of the Language SPI.
Phase 1 covers inline template evaluation only — templates are compiled at construction time
from string sources and rendered on demand against Exchange data.

> **Phase boundary.** External file loading, `{% include %}`, template inheritance, and hot-reload
> belong to Phase 2 (bd rc-64if, `crates/components/camel-template`).

## Bounded Context

- **Name**: MiniJinja template rendering language
- **Scope**: Phase 1 — inline template rendering only. Source strings provided at
  `MinijinjaExpression` construction time; compiled once, evaluated via `Expression::eval`.
  External file resolution, includes, and hot-reload are deferred to Phase 2.
- **Authority**: ADR-0047 (`docs/adr/0047-template-rendering-engine.md`).

## Glossary

**autoescape wrapper**:
The lexical invariant that every template source MUST be wrapped in exactly one top-level
`{% autoescape "html"|"json"|"none" %}...{% endautoescape %}` block. Enforced by the lexical
validator at compile time. This ensures render output—which may be HTML, JSON, or plain text—has
a declared escape strategy before any data interpolation occurs.
_Avoid_: escape layer, escape guard

**lexical validator**:
Source-level tokeniser that recognises `{% %}`, `{# #}`, `{{ }}`, and
`{% raw %}...{% endraw %}` blocks. Tracks string literals and backslash escapes to avoid false
positives inside quoted content. Implemented in `autoescape_validator.rs`. Runs once at compile
time (inside `MinijinjaExpression::compile`) and rejects malformed templates immediately.
_Avoid_: parser (it is a tokeniser, not a full parser), linter

**LimitedWriter**:
Bounded `std::io::Write` primitive that aborts mid-write when the configured byte limit is
exceeded — the write returns `Err`, the caller stops rendering, and the Exchange carries a
size-limit `CamelError`. Used for both S6 (output) and S9 (context measurement) limits.
Implemented in `limited_writer.rs`.
_Avoid_: capped writer, bounded buffer (LimitedWriter is the precise struct name)

**fuel**:
MiniJinja's instruction budget. Configured via `Environment::set_fuel(Some(u64))`. Each
instruction executed decrements the budget; when fuel reaches zero, rendering returns a
`minijinja::Error::FuelExhausted` which the implementation converts to a `CamelError`.
Protects against runaway templates, infinite loops, and algorithmic-complexity attacks.
_Avoid_: gas, instruction limit (use the MiniJinja-native term "fuel")

**bounded writer**:
Synonym for `LimitedWriter`. Used interchangeably in discussions and ADR-0047.

**spawn_blocking render**:
Synchronous MiniJinja rendering runs on a Tokio blocking thread via
`tokio::task::spawn_blocking`. The async route future wraps the join handle in
`tokio::time::timeout` for the configured render deadline. The residual-worker caveat
applies (see ADR-0047 §4.2) — tasks that exceed their timeout stay in-flight on the blocking
pool and complete silently.
_Avoid_: background render, async render (the render itself is synchronous; only the scheduling is async)

**owned Environment**:
Each `MinijinjaExpression` owns an `Arc<minijinja::Environment<'static>>`. Templates are added
once during construction (`env.add_template(name, source)`) and compiled immediately.
Subsequent evaluations look up templates by name — no re-compilation at eval time.
The `'static` lifetime over template data avoids lifetime-polymorphism complexity in the
Language SPI's `Expression` trait.
_Avoid_: shared environment, global environment (each Expression owns its own)

## Plan deviations

Deviation from the implementation plan discovered during Tasks 1-18:

- **JSON/urlencode features**: workspace `Cargo.toml` enables `minijinja = { version = "2", features = ["fuel", "json", "urlencode"] }`. The plan only listed `fuel`; `json` is needed for `AutoEscape::Json` (spec §4 S7) and `urlencode` for the `|urlencode` filter (Task 8 happy-path). See bd rc-i231.
- **JSON render output**: `{% autoescape "json" %}` produces full JSON serialisation (e.g. a string value renders as `"..."` including quotes). Task 8 test corrected accordingly.
- **urlencode encoding style**: MiniJinja uses RFC 3986 percent-encoding (space → `%20`), not HTML form-style (`+`). Task 8 test corrected accordingly.
- **range cap**: MiniJinja 2.21 `range()` rejects >100k elements. Task 13 timeout test and Task 16 fuel test adapted to use nested loops or smaller ranges.
- **fuel calibration**: Task 16 fuel-exhaustion test required fuel=100B (not 50M) to ensure timeout wins in Task 13 and fuel-exhaustion wins in Task 16.
- **Bare `{` tokenizer bug** (fixed): Task 5 validator had an infinite-loop on bare `{` (not followed by `%`/`{`/`#`). Discovered via Task 16 JSON calibration template `{"v":[...]}`. Fixed in commit `5e80dfd7`; regression test added in `6a508543`.

## Authority

- **Spec**: `docs/superpowers/specs/2026-07-18-rc-ao5-minijinja-language-phase1-design.md`
- **Plan**: `docs/superpowers/plans/2026-07-18-rc-ao5-minijinja-language-phase1.md`
- **ADR**: `docs/adr/0047-template-rendering-engine.md`

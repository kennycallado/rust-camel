# Design: 469-simple-date-now

## Approach

**Simple `date:` sub-language — native parser branch, not a delegate.** The delegate path
(`parse_delegate_slot`) intentionally defers everything to evaluation ("unknown languages fail at
evaluation"). The mission requires compile-time rejection, so `date:` is intercepted before the
delegate check in `parse_slot_content` and parsed into new AST variants:

- `Expr::Date { source: DateSource, output: DateOutput }` for all `date:` slots, where
  `DateSource` is `Now` | `Header(String)` and `DateOutput` is
  `Formatted(DatePattern)` | `EpochMillis`. The date enums derive `Debug, Clone, PartialEq` to
  match the `Expr` derive set (parser tests compare ASTs by value).

`DatePattern` is a compiled, validated representation produced by a pattern compiler at parse time
(`DatePattern::compile(&str) -> Result<DatePattern, LanguageError>`). Compilation rules:

- Accepted letters: `y` (year, width from count, `yy` two-digit, reject count > 4), `M` (month),
  `d` (day), `H` (hour), `m` (minute), `s` (second) — these five require exactly two repeated
  letters — `S` (millis in Java minimum-width integer semantics: count = minimum digits,
  zero-padded, never truncated, reject count > 3), `X` (ISO-8601 offset; matching JDK
  zero-offset behavior, every accepted count `X`/`XX`/`XXX` renders `Z` for the UTC clock — a
  single offset segment regardless of count), `'` quoting (`''` is a literal quote; unterminated
  quote is a parse error).
- `'...'` quotes literal text; `''` is a literal quote; unterminated quote is a parse error.
- Any other ASCII letter (`G E w W F a h K k z Z D L Q u ...`) is rejected with `ParseError`
  naming the offending letter; non-ASCII characters render literally (JDK `SimpleDateFormat`
  parity, pinned by a test). Nothing is silently dropped.
- The special payload `millis` returns epoch milliseconds as a numeric string
  (`Value::Number`); the default pattern when omitted is the ISO form
  `yyyy-MM-dd'T'HH:mm:ss.SSSX`.

Formatting uses **chrono** (already a workspace dependency — `chrono = { version = "0.4", features =
["serde"] }` in root `Cargo.toml`; used by camel-api, camel-cron). `DateTime<Utc>` from epoch
millis, formatted per `DatePattern` via explicit field extraction (year/month/day/hour/minute/
second/millis + `Z` offset literal). No new dependency. A fallible `format_epoch_ms(ms: i64,
&DatePattern) -> Result<String, LanguageError>` keeps tests deterministic: fixed-instant unit
tests (e.g. `1700000123456` → `2023-11-14T22:15:23.456Z`) plus shape assertions for the
live-clock `now` path — no sleeps, no bare wall-clock equality or ordering (per
`test-determinism` posture).

`date:header.<name>` reads the header at evaluation: missing → `Value::Null`; `Value::Number` →
epoch millis; `Value::String` parseable as i64 → epoch millis; anything else → `ConversionError`.
This is a rust-camel adaptation of Java's `date:command:pattern` (Java passes `Date` objects; the
Value model has none) — documented, not silent.

Both evaluator matchers (`eval_sync_expr`, `eval_async_expr`) get arms; the pure leaf fast path
(`eval_leaf`) intentionally does not (it reads the clock at evaluation). Predicate mode accepts the
same slots; a bare `date:` with an unknown command (`${date:week:...}`) is a parse error naming the
accepted commands.

**Rhai `now_ms()`.** Registered in `create_base_engine` next to the JSON host module:
`engine.register_fn("now_ms", || -> i64 ...)` reading `SystemTime::now()` epoch millis. Decision:
**always registered** — engines are per-evaluation raw engines; existing scripts never call
`now_ms`, so no existing test gains nondeterminism, and no `Camel.toml` plumbing is needed.
Opt-in happens at the call site. Returns `i64`, which the existing outbound converter maps to
`Value::Number` — storable (`headers["t"] = now_ms()` works), fixing the rc-2yb8q note that
`timestamp()` (`Instant`) is refused outbound. `timestamp()` semantics untouched. `lib.rs`
sandbox docs and `docs/src/languages/rhai.md` gain a monotonic-vs-wall-clock distinction.

**MiniJinja decision.** No wall clock in templates. Templates render exchange-derived context
only; a route that needs a timestamp computes it with Simple `date:` or Rhai `now_ms()` and passes
it as a header/property. This preserves the ADR-0032 posture (exchange data untrusted) and the
sandbox surface; adding a host `now` function would be a new capability with its own design — out
of scope. Recorded in `docs/src/languages/minijinja.md`, the crate docs, and the spec delta.

## Affected crates

- `crates/languages/camel-language-simple`: parser (date slot + pattern compiler), evaluator (two arms + formatter),
  `Cargo.toml` (add `chrono.workspace = true` — chrono is already a workspace dependency; no new
  external dep), tests.
- `camel-language-rhai`: `create_base_engine` (now_ms registration), docs comments, tests.
- `camel-language-minijinja`: docs comments + one posture-pinning test (no behavior change).
- `docs/adr/0092-simple-scalar-expressions.md`: amendment note — ADR-0092 ruled "Simple has no
  clock; `now()` is rejected". Mission 469 (master-approved, bd rc-2yb8q) adds the `date:`
  sub-language as the sanctioned clock surface; `now()` stays rejected; the rhai ruling ("clock
  access forces rhai") is updated to name `date:`/`now_ms()` as the two sanctioned surfaces.
- `docs/src/languages/{simple,rhai,minijinja}.md`: user-facing docs.
- `openspec/changes/469-simple-date-now/specs/language-value-boundary/spec.md`: delta (ADDED
  requirements + MODIFIED delegation requirement reserving `date:` from the delegate path).

## Architecture boundaries

Languages layer only. No Language SPI trait change (`LanguageError::ParseError` and `EvalError`
already cover the new diagnostics). The Runtime resolves `date:` inside Simple like any other slot
— no new registry entry. Simple output flows through the existing native output budget
(`charge_native_output`), Rhai `i64` through the existing converter. No component, DSL, or service
changes.

## Phases

Single-phase: parser+evaluator land together (TDD), then Rhai, then docs/spec. No milestone
grouping.

# Proposal: 469-simple-date-now

## Why

No expression language exposes wall-clock time. Simple has no date functions; a bare `now()` is a
parse error because Simple has no clock (`language-value-boundary`, "Bare identifier and clock call
are rejected"). Rhai's `timestamp()` is a monotonic `std::time::Instant` and the outbound converter
refuses it, so it cannot even be stored in a header. MiniJinja has none. A downstream team
(camel-112) runs a JS/Boa route solely to call `Date.now()`/`toISOString()`.

This change adds a Camel-Java-compatible Simple `date:` sub-language and an opt-in Rhai `now_ms()`
so that route can be dropped. bd: rc-2yb8q (P2).

## What Changes

- **camel-language-simple**: `${date:now:<pattern>}` and `${date:header.<name>:<pattern>}` slots
  using a documented Java `SimpleDateFormat` subset (`y M d H m s S X`, `'` quoting), UTC by
  default. Unsupported pattern letters/constructs are rejected at expression compile time with a
  clear `LanguageError::ParseError` — format characters are never silently dropped. `${date:now}`
  without a pattern uses a documented ISO-8601 default. New `Expr` variants keep `date:` out of the
  language-delegate path (delegates fail at evaluation; this surface must fail at compile time).
- **camel-language-rhai**: `now_ms()` returns wall-clock epoch milliseconds as `i64` (outbound
  storable, unlike `timestamp()`). Always registered; opt-in at the call site. Monotonic
  `timestamp()` semantics unchanged.
- **camel-language-minijinja**: decision recorded — no wall-clock access in templates; time is
  computed by Simple/Rhai and passed via header/property. No sandbox weakening.
- **docs/src/languages/**: simple.md (date section, updated no-clock wording), rhai.md (now_ms vs
  timestamp), minijinja.md (decision). Test-determinism notes included.
- **openspec**: delta on `language-value-boundary`; change dir left for master archive.

Excluded: timezone support beyond documented UTC/offset letters, a general chrono DSL, minijinja
wall clock without a design, new external dependencies (chrono is already a workspace dep).

## Acceptance criteria

- `${date:now:yyyy-MM-dd'T'HH:mm:ss.SSSX}` compiles and evaluates to ISO-8601 UTC; `${date:now:millis}`
  and `${date:now}` work per docs.
- Unsupported patterns (`yyyy-MM-dd G`, unterminated quote, unknown letter) fail at
  `create_expression`/`create_predicate` time with a clear parse error.
- Rhai `now_ms()` returns an i64 and is storable to a header in a mutating script.
- `cargo test -p camel-language-simple -p camel-language-rhai` green; fmt, clippy `-D warnings`,
  xtask lints, docs build, `openspec validate --all --strict` green.

## Risk budget

- Pattern subset is intentionally narrow; widening it later is additive.
- `date:` moving from eval-time "unknown language" to compile-time parsing is a behavior change for
  previously-invalid routes — that is the point, and the spec delta records it.
- No changes to the Language SPI trait, the delegate path, or Rhai `timestamp()` semantics.

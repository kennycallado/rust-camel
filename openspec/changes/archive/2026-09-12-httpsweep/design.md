# Design: httpsweep

## Context

bd rc-jbs1v: the camel-http producer injects four operator/trace-sourced
headers via `HeaderName`/`HeaderValue` construction with `if let Ok(...)`
guards that silently drop the header on failure (camel-component-http
`src/lib.rs`, base 851083a8): user-agent (~3024), W3C TraceContext otel
(~3038), Basic auth (~3091), Bearer auth (~3099); plus connection-close
(~3106) on a literal. rc-8l23a landed the zone's settled convention for this
class (`select_outbound_headers` + DEBUG drop records), and the 2026-09-11
oracle ruling ordered the remaining sites to reuse that convention. This
mission also drains bd rc-wsx2y (zone sweep) whose only live item is a Tier A
remainder that transfers to a successor bd.

Pre-flight: e_glm ses_f6b6d722effeAD8MLHNi6YwtrI (2026-09-12), verdict GO.
Its rulings 2-6 shape everything below.

## Goals / Non-Goals

Goals:
- Construction failures at the four dynamic sites surface as DEBUG drop
  records (correlation_id + header name + reason; never values — ADR-0051);
  the request proceeds without the header (operator config is trusted,
  ADR-0032; no exchange failure).
- Success-path behaviour and `collected_headers` push order unchanged.
- Tier A probe remainder leaves rc-wsx2y into a successor bd.

Non-Goals: exchange-failure/redelivery semantics, ADR-0070 inline-probe
refactor, otel per-exchange `HashMap` allocation, any crate outside
`camel-component-http`.

## Decisions

1. **Helper, not signature abuse.** New private helper next to
   `select_outbound_headers` (lib.rs ~3534):
   `constructed_header<'a>(name: &'a str, value: &str) ->
   Result<(reqwest HeaderName, reqwest HeaderValue),
   OutboundHeaderDrop<'a>>` — the record borrows the name, matching
   `OutboundHeaderDrop`'s existing lifetime — reusing the existing record
   type and the exact reason
   strings `"invalid header name"` / `"invalid header value"` so log greps
   stay uniform across both paths. Static-name sites pass lowercase literal
   names (`"user-agent"`, `"authorization"`) so the record's name field is
   log-consistent. Call sites log `Err` with the landed `debug!` shape
   (`correlation_id`, `header = %drop.name`, `"outbound header dropped:
   {reason}"`, no `value_kind`).
2. **connection-close: `HeaderValue::from_static("close")`.** DEVIATION from
   the 2026-09-11 ruling letter ("each of the 4 sites logs ... on from_str
   failure"), sanctioned by pre-flight ruling 3: a static ASCII literal is
   type-level infallible; a log branch there would be dead, untestable code.
   `from_static` refuses non-`&'static` inputs, so a future dynamic value
   cannot silently reintroduce the drop class — it will not compile.
3. **Basic keeps the guard.** Base64 STANDARD output is always header-safe,
   but construction runs on a dynamic `format!` string the type system cannot
   prove safe; the log branch stays for uniformity with Bearer (comment in
   code notes why). Never simplify the `Err` arm away.
4. **otel routing.** The otel loop's per-pair `(name, value)` construction
   routes through the same helper; the site is `#[cfg(feature = "otel")]`
   and its drop record reason distinguishes name vs value failure via the
   helper's record alone (no separate otel-specific logging path). The
   routing itself is accepted by SOURCE REVIEW (the cfg-gated loop visibly
   calls `constructed_header`), not by a `cfg`-gated test: W3C injection
   emits fixed legal pairs, so an invalid injected pair cannot be
   constructed at runtime in a default-feature test.
5. **Tests per pre-flight ruling 5 (executable-level).**
   - Helper unit tests (feature-independent), in the existing `mod tests`
     next to `select_outbound_headers`'s tests:
     - `constructed_header_invalid_value_returns_drop_record`: arrange
       name `"user-agent"`, value containing `"\r\n"`; act
       `constructed_header(name, value)`; assert `Err` whose `reason ==`
       `"invalid header value"` and `name == "user-agent"`; assert the
       formatted record/debug output contains no fragment of the value.
     - `constructed_header_invalid_name_returns_drop_record`: name with a
       space, valid value; assert `Err` with `reason ==
       "invalid header name"`; no value bytes in record.
     - `constructed_header_valid_pair_roundtrip`: valid name+value;
       assert `Ok` and the pair round-trips.
     - No `unwrap`/`expect` (lint-unwrap): match/assert on `Result`.
   - Producer wire test `producer_invalid_configured_headers_surfaced`:
     `#[tracing_test::traced_test]`, TWO `start_request_capturing_server`
     instances (the fixture serves exactly one request each):
     - Producer 1 (invalid config): `user_agent = Some("bad\r\nua")`,
       Bearer token containing `"\r\n"`, destination = server 1. Act:
       send exchange, await captured request. Assert: captured request
       has no authorization header and no user-agent equal to `"bad\r\nua"`
       (value-absence, not "any UA" — reqwest may inject a default);
       logs contain exactly 2 drop records for this correlation_id (one
       `header=user-agent` reason invalid-value, one
       `header=authorization` reason invalid-value); the sentinel
       fragments `"bad\r\nua"` and the bearer token string appear in NO
       captured log line (ADR-0051).
     - Producer 2 (valid config, same test fn): valid user-agent + valid
       Bearer against server 2. Assert: captured request carries both
       headers with the exact configured values; zero drop records for
       its correlation_id.
   - connection-close: no new test (compile-time proof; existing valid-path
     coverage).
   - otel: helper-level coverage only; no `cfg`-gated producer test
     (spec-bless ruling 2026-09-12: unconstructible without the feature).
6. **Spec delta.** `http-emission-correctness`: ADD requirement "Producer
   injected header construction surfacing" — deliberately separate from
   "Producer outbound header forwarding" (rc-8l23a's exchange-propagation
   requirement; different mechanism). Wording is conditional ("when
   construction ... fails"), promises omission + DEBUG record + request
   proceeds, never names values (ADR-0051).

## Data/control plane, ADRs

Log-only surfacing in the data plane; no control-plane surface changes.
References: ADR-0051 (never log credential/value bytes), ADR-0032 (operator
config trusted — log, don't fail), rc-8l23a landed pattern
(`select_outbound_headers`, commit fca5f142), ADR-0070 (successor bd scope
only).

## Risks / Mitigations

- 11609-line `lib.rs`: surgical edits only, no surrounding reformat (fmt
  noise would drown review).
- Header order: push order of `collected_headers` blocks unchanged (tests
  may assert sequence).
- reqwest default user-agent: wire test asserts our invalid value is absent
  rather than "no UA header exists".
- lint-unwrap: `Err` path is `debug!`, never `unwrap`/`expect`/panic.

## Phases

Single-phase (four call-site edits + helper + tests + bd bookkeeping);
no `## Phase N` split — scope fits one delivery group.

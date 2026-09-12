# Proposal: httpsweep

## Why

The camel-http producer injects operator-configured and trace-context headers
(user-agent, W3C TraceContext, Basic/Bearer auth, connection-close) through
`HeaderName`/`HeaderValue` construction with `if let Ok(...)` guards. When
construction fails (e.g. an invalid byte in a user-agent or Bearer token), the
header is **silently absent** from the outbound request. The silent-drop class
was fixed everywhere else (rc-8l23a landed `select_outbound_headers` with
DEBUG drop records); these four producer sites are the remaining neighbors
(bd rc-jbs1v, oracle ruling 2026-09-11: piggyback onto the rc-wsx2y zone
sweep, reuse the same logging convention).

Separately, bd rc-wsx2y (`tech-debt-sweep: camel-http zone`) holds only its
Tier A remainder (~20 ADR-0070 inline bind-read-drop probes, sites ~5648-7718
and ~10812) — explicitly NOT sweep-sized; it transfers to a successor bd.

## What Changes

- **Included** (crate `camel-component-http`, `src/lib.rs` only):
  - New private helper `constructed_header(name, value) ->
    Result<(HeaderName, HeaderValue), OutboundHeaderDrop>` reusing the landed
    `OutboundHeaderDrop` record and reason strings.
  - The four construction sites (user-agent ~3024, otel ~3038, Basic ~3091,
    Bearer ~3099) route through the helper; `Err` emits a DEBUG drop record
    (correlation_id, header name, reason; never the value — ADR-0051) and the
    request proceeds without the header. Success path unchanged.
  - connection-close (~3106) uses `HeaderValue::from_static("close")` —
    type-level infallible; sanctioned deviation from the 2026-09-11 ruling
    letter (e_glm pre-flight ruling 3, 2026-09-12).
  - Tests: helper unit tests + one producer wire test (capturing server +
    `traced_test`) covering invalid values surfaced / valid path unchanged.
  - bd bookkeeping: file successor bd for the Tier A probe remainder, then
    close rc-wsx2y; close rc-jbs1v.
- **Excluded**: ADR-0070 inline-probe refactor (Tier A, successor bd), otel
  `HashMap` allocation per exchange, exchange-failure semantics for bad
  headers (request proceeds), any crate outside the camel-http zone lease.

## Acceptance criteria

- Each of the 4 helper-routed sites logs a DEBUG drop record (name + reason,
  no value) on construction failure; no behavioural change on success.
- connection-close path compiles via `from_static`; no dead log branch.
- Wire test: invalid user-agent + invalid Bearer → headers absent from
  captured request, drop records present in logs, offending value strings
  absent from logs; valid config → headers present.
- `cargo test -p camel-component-http`, fmt, clippy `-D warnings`, all
  xtask lints, doc gate on `camel-component-http` — green.
- rc-wsx2y closed only after successor bd exists; rc-jbs1v closed with the
  four sites + `from_static` note.

## Risk budget

- Acceptable: zero success-path behaviour change; log-only surfacing (DEBUG,
  not exchange failure); surgical edit inside the 11609-line `lib.rs` with
  no surrounding reformat; header push order unchanged.
- Out of bounds: exchange failure / redelivery on bad headers, value bytes in
  logs (ADR-0051), refactors beyond the four sites + close literal, any other
  crate, `cargo test --workspace`.

Bd: rc-wsx2y (sweep collector), rc-jbs1v (defect, REQUIRED).

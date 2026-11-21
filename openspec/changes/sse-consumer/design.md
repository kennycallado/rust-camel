# Design: sse-consumer

## Approach

All behavioral decisions are pinned by bd rc-q48en (e_opus review of the SSE
epic); this design maps them onto the existing camel-http seams.

New module `crates/components/camel-http/src/sse.rs` (framing parser +
consumer loop) plus a thin `SseComponent` / `SseEndpoint` pair. The framing
parser is a pure state machine: bytes in, `Vec<SseFrame>` out
(`Event(SseEventFrame)` or `Comment`; partial input stays buffered in the
parser — no `NeedMore` variant). It handles
one BOM at stream start only, CRLF/CR/LF line endings, multi-line `data:`
joined with `\n` (final `\n` dropped), comment-only and empty-data block
suppression, and field parsing (`event:`/`id:`/`retry:`/`data:`). No new
dependency; no streaming framework.

`HttpBundle::register_all` (bundle.rs:25-28) additionally registers
`SseComponent` (`scheme() == "sse"`, per `Component` trait,
camel-component-api/src/component.rs:12-45). `SseEndpoint::create_consumer`
returns the SSE consumer; `create_producer` returns `Err` (consumer-only).
The consumer follows the camel-mqtt pattern (consumer.rs:203-316): spawned
loop task, `ConsumerStartupMode::Explicit`, `background_task_handle()`,
`biased` select on the cancel token, `ctx.send(exchange).await?` per event
(ADR-0044 back-pressure — no internal channel), task errors returned as
`Err` so route supervision (ADR-0007) owns recovery.

Per connect: `ssrf::validate_url_for_ssrf` (plus a new userinfo rejection),
`resolve_initial_url_for_ssrf` for DNS pinning,
`PinnedClientCache::get_or_build` for the pinned client, and
`send_with_ssrf_safe_redirects` for GET with `Accept: text/event-stream` —
which already implements ADR-0081 per-hop validation with Authorization
stripped cross-origin, and restricts redirects to the connect phase (the
stream is consumed from the final response). TLS allowlist applies via the
shared `build_client` path. All of this re-runs on EVERY reconnect.

Resubscription loop: transport errors and 5xx → close, gate the attempt
with `NetworkRetryPolicy::should_retry`, sleep `delay_for(attempt)` (default
3 s, clamped [1 s, 60 s]; a server `retry:` field overrides until the next
stream changes it) via a cancelable sleep, then reconnect — always to the
CONFIGURED URL (never a previous redirect target), full security pass
re-applied — with `Last-Event-ID` (stored id, memory only, ≤ 1 KiB). Attempt
budget resets when the FIRST complete event or comment is parsed after a
reconnect — never on HTTP 200. Budget exhaustion → task error. 204, any
other non-2xx, wrong `Content-Type` → terminal task error, never
reconnected. Malformed oversize
event (`maxEventBytes`, default 1 MiB) and invalid UTF-8 → non-retryable
task errors, each with one metric label
(`increment_errors(route_id, "b-prime:sse:oversize")` /
`"b-prime:sse:utf8"`); the UTF-8 deviation from the spec U+FFFD rule is
recorded in the ADR. Idle timeout (default 90 000 ms) resets on ANY bytes
including comments; connect timeout default 10 000 ms (crate default is
5 s — the sse scheme pins its own default). An id longer than 1 KiB is
delivered but not stored for resend (a truncated id would corrupt
resubscription).

Config: `#[derive(..., UriConfig)] #[uri_scheme = "sse"]` struct with
`maxEventBytes`, `idleTimeoutMs`, `connectTimeoutMs`, `retryInitialMs`,
`retryMaxMs`, `allowInternal`, `allowCleartext`, `authHeaders`
(`Name1=Value1,Name2=Value2` — credentials come only from these configured
headers; `sse:` defines no username/password params; userinfo in the URL is
rejected at endpoint creation).
`cargo xtask schema` regenerates `schemas/component-metadata.json` (the new
scheme's `uri_options` flow from the derive).

ADR: **new** `docs/adr/0102-sse-consumer-scheme.md` — no existing ADR owns
these decisions (ADR-0060 explicitly excluded SSE transports from the
stream: Cargo surface, a different seam). It records: `sse:` as a
consumer-only scheme inside camel-http (reusing ssrf/tls/client_cache
seams), resubscription-not-retry with the budget-reset point, the four
bounds + id cap, and the UTF-8 task-error deviation from U+FFFD. References
ADR-0007 (supervision), 0013 (retry policy), 0044 (back-pressure), 0081
(redirect auth).

Tests: unit tests for the parser boundary cases (BOM, line endings, join,
suppression, headers, 1 MiB oversize, invalid UTF-8, id cap, chunk splits);
route-level tests in `camel-http/tests/sse_consumer_route.rs` using
`HttpTestServer` (`staged`/`chunked` responders — lint-no-free-port and
lint-no-raw-http-test-server satisfied) driving a real `CamelContext` route
(the camel-rabbitmq/tests/route_e2e.rs pattern; camel-core + camel-builder
are already dev-deps): one-exchange-per-event, 204 terminal, 5xx retried,
cancel mid-read and mid-sleep. `Last-Event-ID` resend, non-2xx (403) and
wrong-`Content-Type` stay at component tier (`sse_tests.rs`).

## Affected crates

- `camel-http`: new `sse.rs`, `SseComponent` registration, uri config,
  tests, docs page section.
- `schemas/` + `docs/adr/0102-*`: regenerated / new. NOTE (deviation recorded
  after Task 1.7): `schemas/component-metadata.json` is a schemars type
  schema of the `ComponentMetadata` struct, not a per-scheme registry — a
  URI-config-only addition cannot drift it. Acceptance is
  `cargo xtask schema --check` exit 0 (no drift; verified) plus the runtime
  metadata surface test `sse_metadata_surface_pinned` (pins scheme,
  consumer-only capabilities, and exactly the 8 options).
- No producer-path changes; no camel-api / camel-core source changes
  expected (schema check verifies).

## Architecture boundaries

Components lane only. The consumer submits `ExchangeEnvelope` via
`ConsumerContext::send` (CONTEXT-MAP: Components → Runtime); it does not
touch pipeline internals, runtime buses, or DSL. Data/control plane split:
bounds and metrics are data-plane; route supervision stays in the runtime
(ADR-0007). The HTTP server direction (`HttpRouteRegistry`) is untouched.

## Alternatives considered

- Separate `camel-sse` crate (epic sketch): rejected — bd rc-q48en pins
  in-crate placement to reuse the ssrf / tls_allowlist / client_cache seams
  without cross-crate exposure (all are `pub(crate)`).
- Reusing the producer path with a per-event split: rejected — the producer
  contract is one exchange in/out; its read-whole-body path must stay
  untouched (order boundary).
- A streaming-framework dependency (eventsource crates): rejected by order
  — the parser is small and must not pull a dependency.

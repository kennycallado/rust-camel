# Proposal: llm-timeout-deadline

## Why

`hardened_http_client_with_pinning()` builds the shared reqwest client with a
hidden `.timeout(Duration::from_secs(30))` — a **total** timeout covering
headers and the whole body (reqwest 0.13.5). Chat is always an SSE/NDJSON
stream on the wire (even in materialized mode), so the 30 s wall fires
mid-body and surfaces as `Decode("error decoding response body")` with
`is_timeout() == true`. `map_reqwest_error` then reports it as
`LlmError::Timeout(configured)` where `configured = timeout_secs.or(global).unwrap_or(30)`.
The customer's call died at 08:30:30 with `timeout_secs = 90` configured, and
the error claimed "timeout after 90s". Connect timeouts are misreported
identically.

This contradicts the documented contract (`docs/src/components/llm.md:47,282-286`),
the spec (`openspec/specs/llm-provider-adapter/spec.md:138,191`), and ADR-0021
(the hidden 30 s wall *is* a per-attempt timeout, which ADR-0021 forbids).
The defect shipped in 0.65.0 and 0.66.0. bd: `rc-9pfby` (P1, customer-reported).

## What Changes

- Remove the client-wide `.timeout(30s)` from the hardened client. Keep
  `.redirect(none)`, `.no_proxy()`, `.connect_timeout(10s)`, DNS pinning.
- Introduce `pub(crate) LlmHttpPolicy { connect, read_idle }` with `chat()`
  = (10s, Some(600s)) and `decision()` = (10s, None), plus
  `hardened_http_client_with_policy(policy, pinned)`. Public
  `hardened_http_client*` signatures stay and delegate with `chat()`.
- Add a 600 s transport read-idle guard on the chat/embed client (OpenAI and
  Anthropic SDK default read timeouts; above Ollama's 300 s load-stall limit).
  The decision client gets **no** `read_timeout` (it also caps time-to-headers
  and has no per-request override).
- Drop `unwrap_or(30)` in the three `provider_factory.rs` arms: the adapter
  must not own a deadline it does not report.
- Error honesty (additive, patch-safe): keep `Timeout(Duration)` for the
  producer deadline; add `TransportTimeout { phase, after }` (retryable,
  `#[non_exhaustive]` phase enum) and `LlmError::is_timeout()`. Every timeout
  display contains the word `timeout` and the duration that fired.
- Docs (`docs/src/components/llm.md`, crate `CONTEXT.md`, builder rustdoc),
  glossary entry "transport read-idle guard", and an ADR-0021 addendum
  recording the transport layers and the accepted `timeout_secs: None`
  residual risk.
- Spec deltas: `llm-provider-adapter` (timeouts carry the layer), `llm-decisions`
  (connect row + policy-based client builder).

Excluded: body-size caps for chat/embed (follow-up bd), Keycloak's own hardened
client (`rc-s8itn`), any change to SSRF validation, pinning, the lazy `OnceCell`,
retry policy, backoff, `retry_after` handling,
`MAX_DECISION_RESPONSE_BYTES`/`read_capped`.

## Affected crates

- `camel-component-llm` (the only crate changed)

## Acceptance criteria

- A provider with `timeout_secs = 90` completes a body read whose header-to-body
  gap exceeds 30 s (paused-clock test, staged response server).
- Timeout errors name the layer that fired: connect, read-idle, or producer
  deadline; every display contains `timeout` and the duration that fired.
- Decision payloads, timing, and capped buffering stay unchanged; the only
  intended change is the timeout taxonomy/display.
- `hardened_http_client*` public signatures unchanged; `rg '\.timeout\('` over the crate's
  non-test `src/` hits only `decision/http.rs:92` (two pre-existing test-helper hits stay).
- Docs, CONTEXT.md, rustdoc, ADR-0021 addendum, and spec deltas land in the
  same change.

## Risk budget

- `timeout_secs: None` chat providers lose the hidden 30 s bound; the accepted
  residual risk (a trickling peer is unbounded; no body-size cap yet) is
  recorded in the ADR-0021 addendum and filed as a bd follow-up.
- The additive error shape keeps 0.66.x patch compatibility; `Timeout` stays a
  tuple variant (no struct-variant breaking change).

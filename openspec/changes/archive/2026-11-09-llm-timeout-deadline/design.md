# Design: llm-timeout-deadline

## Approach

**Diagnosis (verified against reqwest 0.13.5 sources).** The shared reqwest
client is built with `.timeout(30s)` — a total timeout from connect to end of
body. Chat is always SSE/NDJSON on the wire (materialized mode included), so
the wall fires mid-body as `Decode("error decoding response body")` with
`is_timeout() == true`. `map_reqwest_error` maps every reqwest timeout to
`LlmError::Timeout(configured)`, `configured = timeout_secs.or(global).unwrap_or(30)`
— hence "timeout after 90s" at 30 s. The 10 s connect timeout is misreported
the same way. `apply_timeout` (producer deadline) is correct and must not change.
The hidden 30 s wall is itself a per-attempt timeout, which ADR-0021 forbids.

**Fix — one client policy per caller.**

```rust
pub(crate) struct LlmHttpPolicy { connect: Duration, read_idle: Option<Duration> }
// chat()     = (10s, Some(600s))
// decision() = (10s, None)
pub(crate) fn hardened_http_client_with_policy(policy, pinned) -> Result<reqwest::Client, CamelError>
```

- Drop `.timeout(30s)`; keep `.redirect(Policy::none())`, `.no_proxy()`,
  `.connect_timeout(10s)`, `resolve_to_addrs` pinning, lazy `OnceCell`.
- The public `hardened_http_client_with_pinning` / `hardened_http_client`
  keep their signatures (semver) and delegate with `chat()`.
- **Why per-caller:** reqwest 0.13.5 `read_timeout` is client-wide, also caps
  time-to-headers (the timer is not reset until headers arrive), and has no
  per-request override. Decision calls (embed-style `text()` reads, cold Ollama
  model loads) rely on the per-request `.timeout(configured)`; giving them a
  client read-idle guard would add a new hidden cap. Decision payloads,
  timing, and capped buffering stay unchanged.
- **Why 600 s read-idle:** the OpenAI and Anthropic SDKs default to 600 s per
  read; Ollama's model-load stall limit is 300 s. Above every documented
  vendor silence, still finite (bounds a hung connection when
  `timeout_secs = None`). Do **not** derive it from `timeout_secs`: equal
  values make the transport timer and the streaming per-chunk timer race and
  flip the reported layer. 600 s > any realistic configured deadline, so the
  producer deadline fires first in the normal case.
- **The configured deadline is the deadline.** `apply_timeout` (materialized
  total) and the per-chunk streaming timeout already implement ADR-0021's
  single-deadline rule. Decisions keep their per-request total
  (`decision/http.rs:92`).
- Drop `unwrap_or(30)` in all three `provider_factory.rs` arms. Keep the
  `build_*` `configured_timeout: Duration` parameter for public-API
  compatibility: the decision path keeps using it, the chat path stops
  consulting it and threads the policy instead.

**Error shape (additive, patch-safe).**

- Keep `Timeout(Duration)` = a deadline lapsed: the producer deadline on the
  chat path, the per-request total on the decision path. New display:
  `timeout: deadline {0:?} exceeded` (contains `timeout` — required by
  `llm-decisions/spec.md:405`, `producer_timeout_tests.rs:26,53`, `error.rs:229`,
  `tests/decision_http_resilience.rs:200`).
- Add `TransportTimeout { phase: TransportTimeoutPhase, after: Duration }` with
  `#[non_exhaustive] pub enum TransportTimeoutPhase { Connect, Idle }`.
  Retryable. Displays: `timeout: connect not established within {after:?}` /
  `timeout: no response activity for {after:?}`. The layer MUST be in the
  display: `From<LlmError> for CamelError` keeps only the string.
- Add `LlmError::is_timeout(&self) -> bool` covering both variants.
- `map_reqwest_error(err, policy, request_total: Option<Duration>)`:
  `is_timeout && is_connect` → `TransportTimeout { Connect, policy.connect }`;
  `is_timeout && !is_connect` → chat/embed (no client total)
  `TransportTimeout { Idle, policy.read_idle }`; decisions (per-request total,
  no idle) → `Timeout(request_total)` unchanged. Do **not** turn `Timeout`
  into a struct variant — breaking public API, cannot ship in a patch.
- Consumers: `http_errors.rs`, `genai_adapter.rs` (thread policy instead of
  configured timeout), `decision/http.rs`, `error.rs::is_retryable`, crate
  `CONTEXT.md:106`.

**Docs and vocabulary.** `docs/src/components/llm.md` (`:47`, "Retry and
timeout" `:282`): what bounds a chat call, in what order, which value wins,
error text meanings. Crate `CONTEXT.md` (`:49-51,106`) and builder rustdoc
(`lib.rs:52-66`). Term: **transport read-idle guard** (glossary marks "idle
timeout" as _Avoid_); add a glossary entry distinguishing it from the activity
timeout.

**ADR-0021 addendum** (same commit): transport layers (connect 10 s,
read-idle 600 s), the single-deadline rule restored, and the accepted
`timeout_secs: None` residual risk — a peer trickling bytes is not bounded and
the chat path has no body-size cap (bd follow-up, discovered-from `rc-9pfby`,
linked to `rc-s8itn`).

## Affected crates

- `camel-component-llm`: `lib.rs`, `error.rs`, `provider/http_errors.rs`,
  `provider/genai_adapter.rs`, `provider_factory.rs`, `decision/http.rs`,
  `producer.rs` (error types only), tests, `CONTEXT.md`.

## Architecture boundaries

Data/control-plane split untouched: the producer timeout and retry execution
is data-plane behavior and keeps its semantics; only the transport client
construction and error mapping change inside the component. No DSL, runtime,
or endpoint surface changes. Public API is additive only.

## Alternatives considered

- **Per-request total timeout on chat** — rejected: genai owns request
  building (`exec_chat_stream`), and a per-attempt total violates ADR-0021.
- **Client `read_timeout` for everyone** — rejected: caps time-to-headers
  client-wide with no per-request override; decisions would gain a hidden cap.
- **Configurable idle value** — rejected: a config knob implies operator
  ownership of a transport detail; 600 s is evidence-based and above every
  realistic deadline.
- **Docs-only fix ("document the 30 s")** — rejected: documents a bug.
- **Struct-variant `Timeout`** — rejected: breaking change, not patch-safe.

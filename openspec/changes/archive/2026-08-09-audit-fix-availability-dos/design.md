# Design: audit-fix-availability-dos

## Approach

### I1 — gRPC accept-loop busy-spin (rc-5qao)

The accept-loop at server.rs:346-356 runs `listener.accept().await` in a
`loop`. On error, it logs and `continue`s immediately. Under EMFILE
(file descriptor exhaustion) or EINVAL (persistent OS error), this is a
CPU-spinning log-flood.

**Fix:** Reuse `camel_api::backoff::BackoffState` (already used by
kafka, redis, ws, jms, xj, xslt — see backoff.rs:43-76). Construct a
`BackoffState::new(BackoffConfig { initial_delay: 10ms, multiplier: 2.0,
max_delay: 5s })` as a local variable in `run_grpc_server`. On accept
error: call `backoff.next_delay()` and `tokio::time::sleep(delay).await`
before `continue`. On successful accept: call `backoff.reset()`.

This avoids reinventing the backoff pattern and leverages the
overflow-safe `next_delay()` implementation (caps the exponent at 63,
prevents shift overflow).

**Accept-flap non-goal:** Under alternating `Err/Ok/Err/Ok` (e.g., FD
exhaustion freeing one FD per accept), `reset()` on each success keeps
backoff at 10ms. This "accept-flap" scenario is declared an explicit
non-goal — it requires a different fix (decay-on-success rather than
reset-on-success) and the practical impact is limited (the server IS
making progress, just slowly). Filed as follow-up if observed in
production.

### I2 — LLM unbounded deserialize (rc-wvty)

`build_chat_request` calls `serde_json::from_value` directly on
untrusted `serde_json::Value`s from exchange headers. A multi-MB JSON
array in `CamelLlmMessages` or `CamelLlmTools` causes unbounded
allocation during typed-object deserialization.

**Fix:** Add `max_header_json_bytes` to `LlmGlobalConfig` with
`#[serde(default = "default_max_header_json_bytes")]` = 65_536 (64 KB),
following the exact `max_prompt_bytes` precedent (config.rs:33-34).
Thread it through `endpoint.rs` → `LlmProducer` via the existing builder
pattern (`.with_max_header_json_bytes()`, NOT a 5th positional arg to
`LlmProducer::new`).

In `build_chat_request`, before each `serde_json::from_value` call on
`CamelLlmMessages`, `CamelLlmTools`, and `CamelLlmToolChoice`, check the
serialized byte size via `value.to_string().len()`. If it exceeds
`max_header_json_bytes`, return `LlmError::InvalidRequest`.

**Measurement rationale:** The `serde_json::Value` is already in memory
(bounded by the Exchange header ingress). The `to_string()` call
produces a temporary `String` that is dropped immediately after the
length check — peak memory is 2× the Value size (Value + String), not
the potentially O(n²) allocation cascade from `from_value` creating
millions of typed objects. This is a secondary defense: the primary
defense is header-size limiting at Exchange ingress. The `to_string()`
check prevents the `from_value` amplification specifically.

## Affected crates

- `camel-component-grpc`: `BackoffState` in `run_grpc_server` accept-loop
- `camel-component-llm`: `max_header_json_bytes` field on
  `LlmGlobalConfig`, threaded through endpoint → producer builder,
  size check in `build_chat_request`

## Architecture boundaries

Both fixes are within their respective component crates. No changes to
`camel-api` (BackoffState already exists). No changes to Runtime, DSL,
or core. The gRPC fix is internal to one async function. The LLM fix
follows the existing `max_prompt_bytes` pattern exactly.

## Alternatives considered

- **gRPC: hand-rolled `2^failures` counter.** Rejected: reinvents
  `BackoffState`, can overflow after ~32 failures, and conflicts with
  the shared backoff glossary.
- **gRPC: abort the server on persistent errors.** Rejected: the server
  should recover when the transient condition clears (e.g., FDs freed).
- **LLM: 5th positional arg to `LlmProducer::new`.** Rejected: breaks
  ~30 call sites. `LlmGlobalConfig` + builder is the established
  pattern.
- **LLM: limit deserialized object count.** Rejected: count-based limits
  miss the OOM vector (each object can be arbitrarily large). Byte-size
  limit on the raw JSON is the correct boundary.
- **LLM: streaming depth-limited parser.** Rejected: the header Value
  is already parsed at ingress; the attack vector is `from_value`
  amplification, not parser depth.

Bd: rc-5qao, rc-wvty

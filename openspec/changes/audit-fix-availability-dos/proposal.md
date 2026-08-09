# Proposal: audit-fix-availability-dos

## Why

Two availability/DoS vulnerabilities found in the v1.0 quality audit:

1. **gRPC accept-loop busy-spin (rc-5qao):** `run_grpc_server`
   (camel-component-grpc/src/server.rs:346-356) uses `continue` with no
   sleep/backoff between `error!` and the next `listener.accept()`. Under
   a persistent `accept()` error (EMFILE, EINVAL), the loop becomes a
   tight busy-spin — log-flooding and CPU pegging. Availability
   degradation that can cascade to other routes sharing the runtime.

2. **LLM unbounded untrusted deserialize (rc-wvty):**
   `build_chat_request` (camel-component-llm/src/producer.rs:170-196)
   performs unbounded `serde_json::from_value` on `CamelLlmMessages` and
   `CamelLlmTools` headers. These are classified untrusted by ADR-0032.
   An attacker can inject a multi-MB JSON payload in these headers,
   causing OOM during deserialization. The `max_prompt_bytes` limit only
   covers the body prompt, not the header payloads.

## What Changes

- **rc-5qao:** Add exponential backoff in the gRPC accept-loop error
  path. On repeated `accept()` failures, sleep with increasing delay
  (capped) before retrying. This prevents CPU spin and log-flood while
  preserving the ability to recover when the transient error clears.
- **rc-wvty:** Add a byte-size limit check on the raw JSON value of
  `CamelLlmMessages`, `CamelLlmTools`, and `CamelLlmToolChoice` headers
  BEFORE `serde_json::from_value`. Reject payloads exceeding a
  configurable threshold (default 64 KB) with
  `LlmError::InvalidRequest`.

## Acceptance criteria

- gRPC accept-loop backs off on repeated errors (not a tight spin)
- LLM producer rejects oversized header payloads before deserialization
- All existing tests pass; new tests prove both fixes
- `cargo clippy` clean on both crates

## Risk budget

Low-medium. The gRPC fix changes timing behavior in an error path
(non-functional under normal operation). The LLM fix adds a rejection
boundary that could reject legitimately large headers — the 64 KB
default is generous for typical chat payloads while preventing OOM.

Bd: rc-5qao, rc-wvty

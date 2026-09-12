# Proposal: jsonreply

## Why

`pipeline_error_to_reply` in `camel-http` repeats the same JSON error body and
HTTP reply construction in four client-error arms. This duplication increases
drift risk while adding no behavior. Bd `rc-q4kkx` records the cleanup found
during REST strict-negotiation review.

## What Changes

- Extract a private `json_error_reply` helper for status, error code, and
  message construction.
- Keep each arm's warning log, status, headers, body bytes, and fallback
  semantics unchanged.
- Add focused behavior-parity tests for the helper and mapped error arms.

No public API, error taxonomy, logging text, or unrelated component changes.

## Acceptance criteria

- The four JSON-error arms call one local helper.
- Replies retain identical status, `Content-Type`, JSON fields, and fallback
  behavior.
- Targeted tests and required Rust quality checks pass.

## Risk budget

Only mechanical code movement in `crates/components/camel-http/src/lib.rs` is
allowed. Preserve the existing `unwrap_or_else` fallback and its lint marker;
do not harden serialization or alter warning placement.

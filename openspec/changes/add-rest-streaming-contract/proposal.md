# Proposal: add-rest-streaming-contract

## Why

L1 (`add-rest-raw-binding`, commit f5f2ace8) landed the explicit `binding:
raw` mode and its canon `rest-dsl` spec deliberately left the streaming
contract open: "single-consumption stream ownership, metadata preservation,
and reply-stream semantics are the streaming contract owned by the follow-up
L3 change and are deliberately not asserted here"
(`openspec/specs/rest-dsl/spec.md`, Raw binding mode pipeline note).

The viability study (`.opencode/fleet/inbox/rest-v2-viability.md` L3
section, blessed by e_gpt) rates L3 as a contract and test increment, not a
new transport feature: core streaming already exists in the HTTP consumer.
What is missing is the pinning surface — tests and spec scenarios that make
the raw stream behavior a contract instead of an accident, plus one decided
policy (response byte limits) that today lives only in a camel-http test
name. bd: rc-q8apn (discovered-from rc-01har).

## What Changes

**In:**

- `openspec/changes/add-rest-streaming-contract/specs/rest-dsl/spec.md` —
  two ADDED requirements on the `rest-dsl` capability:
  "Raw binding stream ownership and metadata" and "Raw binding stream error
  and limit contract".
- `crates/camel-dsl/tests/rest_stream_contract_e2e.rs` (new) — the test
  battery:
  - DSL-level: a driven raw pipeline never polls the request stream,
    preserves stream identity and `StreamMetadata`, and leaves the stream
    consumable exactly once.
  - HTTP-boundary-level: a real `camel-component-http` consumer is driven
    from this crate (dev-dependency; `camel-component-http` does not depend
    on `camel-dsl`, so no cycle) with a hand-rolled TCP client. Pins:
    request `StreamMetadata` carries the request `Content-Type` and
    `Content-Length`; chunked requests over `max_request_body` fail closed
    on consumption; a consumed reply stream yields the existing 500;
    materialized reply bytes over `max_response_body` are replaced with
    500; streamed replies are not byte-capped; a client disconnect during
    a streamed reply does not fail the consumer; the reply may be the
    original request stream or a new stream.
- `crates/camel-dsl/Cargo.toml` — dev-dependencies only:
  `camel-component-api` (feature `test-support`), `camel-component-http`,
  `tokio-util`, plus additive tokio features (`net`, `time`, `io-util`).
- `docs/src/yaml-dsl/step-verbs.md` — "Raw binding streaming contract"
  subsection in the REST DSL section: ownership, metadata, limits policy,
  consumed-stream errors, disconnect semantics.

**Out:**

- No `camel-http` source changes (lease is `camel-dsl`; every pinned
  HTTP-side behavior already exists in code and is only being pinned).
- No OpenAPI code changes — L1's binary schema mapping is already the
  streaming representation; design.md records the pointer to the L1
  OpenAPI requirement that already covers raw streaming operations.
- No `CONTEXT-MAP.md` or glossary edits (shared files outside the lease).
- No JSON-mode changes: raw binding owns stream semantics.

## Compatibility

v1/json routes are untouched — no production code changes at all. The only
build surface is test-only dev-dependencies in `camel-dsl`.

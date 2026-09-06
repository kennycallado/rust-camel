# Proposal: http-query-wire-fidelity

## Why

Pilot + two-expert audit (e_opus, e_glm counter-report; bd rc-enbw) established that the camel-http producer silently rewrites outbound query strings. `HttpEndpointConfig::from_uri` collects query pairs into a `HashMap`, and `resolve_url` rebuilds the query via `url::Url::query_pairs_mut` (form-urlencoded). Effects on the wire: per-process random key order, over-encoding of `,` `:` and space (`%2C`/`%3A`/`+`), and collapse of authored bytes.

The root sits one layer deeper: camel-endpoint `UriComponents` discards the raw query at parse (no `raw_query` field), so no downstream layer can reconstruct the authored bytes (bd rc-oa22z — dedup-loss half refuted by e_glm: `parse_query` already rejects duplicate keys loudly).

On the wire today the serializer also re-encodes authored `RAW(...)` wrapper text, putting literal `RAW%28...%29` in the outbound query (bd rc-g4isv) — the wrapper must instead survive as authored bytes, neither re-encoded nor unwrapped.

Downstream consequence in the scenario tier: partner receives on query-bearing endpoints are structurally unmatchable — arrival lanes key on wire `path_and_query` bytes that nobody can predict (bd rc-kcli). Additionally `resolve_url` panics on malformed operator URLs via `.expect` (bd rc-ph7z2), and receive diagnostics print counts only, never paths.

## What Changes

1. **camel-endpoint** — additive `raw_query: Option<String>` on `UriComponents`, captured byte-for-byte at parse. Structured `params` unchanged (duplicates still rejected loudly). Reconciliation story: *structured view strict, raw view verbatim* — mirroring camel-api `EndpointUri`.
2. **camel-http** — raw-preserving outbound query serialization in `resolve_url`: when raw bytes exist, emit them minus consumed option keys (RAW-aware filtering); RFC-3986-minimal structured serialization only as fallback for programmatic merges. Never unwrap `RAW` wrappers. `.expect` panics replaced by error propagation.
3. **camel-integration-test** — lane key stays **strict wire bytes** (consensus: canonical keys would mask producer bugs). Receive-timeout and partner-count-mismatch diagnostics now list recorded wire paths. `ParsedTarget` silent `/` fallback becomes an apparatus error.

## Acceptance Criteria

- Authored query bytes (order + encoding) survive to the outbound wire, minus explicitly consumed options.
- `RAW(...)` wrapper bytes survive to the wire exactly as authored — never re-encoded (no `RAW%28` text), never unwrapped.
- Programmatic-only query values serialize deterministically (RFC-3986 minimal).
- Malformed operator URLs produce errors, not panics.
- Scenario receive-timeout / count-mismatch messages include arrived wire paths.
- All pinned tests that assert today's rewritten wire format are updated to assert authored bytes.

## Affected crates

camel-endpoint, camel-http, camel-integration-test

## bd

rc-m1k8 (parent), rc-oa22z, rc-g4isv, rc-ph7z2, rc-kcli — all children of epic rc-enbw.

## Risk budget

Wire-format change is externally observable (order becomes stable, fewer escapes, wrapper bytes preserved). Mitigations: raw path only applies when authored raw bytes exist; programmatic fallback keeps prior semantics; pinned expectations updated in-change; security redaction surfaces (ADR-0051) re-verified against raw bytes. Source-compatibility: the new `UriComponents` field breaks in-repo exhaustive struct literals (updated in-change) and any external exhaustive literals (minor-version boundary, documented). No control-plane changes; data-plane producer path only.

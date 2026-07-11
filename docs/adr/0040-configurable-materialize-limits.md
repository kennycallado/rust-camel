# ADR-0040: Configurable Materialize Limits for Producers

**Date:** 2026-07-11
**Status:** Proposed
**Cross-refs:** ADR-0033 (Security defaults), ADR-0038 (Configurable DoS caps), ADR-0032 (Exchange-data trust boundary)

## Context

Three producers (XSLT, XJ, WASM) used `Body::materialize()` or hardcoded `DEFAULT_STREAM_MAX_BYTES` without operator override. XJ had a bug where the operator-set `maxPayloadBytes` was checked after allocation.

## Decision

- XSLT: new `maxPayloadBytes` URI param, threaded through config → endpoint → producer.
- XJ: switch from `materialize()` to `into_bytes(effective)`; remove post-materialization check.
- WASM: new `max_stream_bytes` field on `WasmConfig` + `WasmLimitsConfig`, sourced from URI or Camel.toml.

All default to 10 MiB (`DEFAULT_MATERIALIZE_LIMIT`).

## Out of Scope

- `Body::materialize()` convenience API stays with fixed default.
- Changing `WasmConfig::from_uri()` to fallible return.
- Bounded JSON serialization for non-stream body types in `into_bytes()`.

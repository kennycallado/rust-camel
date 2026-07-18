# ADR-0030: Exchange-aware DataFormat hooks

**Date:** 2026-06-28
**Status:** Accepted (implemented in `d4a423a2`)
**Issue:** bd rc-v5xf
**Oracle:** e_gpt (ses_0f086bbe4ffepmh4MrdpurR0XU) — verdict Trait extension
**Analysis:** docs/superpowers/analysis/dataformat-coupling-2026-06-28.md

## Context

The `DataFormat` trait in `crates/camel-api/src/data_format.rs` exposes only:

```rust
fn marshal(&self, body: Body) -> Result<Body, CamelError>;
fn unmarshal(&self, body: Body) -> Result<Body, CamelError>;
```

CSV full parity needs `captureHeaderRecord=true`, which writes Exchange header
`CamelCsvHeaderRecord`. The trait has no Exchange access, blocking this and
future metadata-sensitive formats (encryption, signing, schema-aware formats,
content-type negotiation).

## Decision

Add default methods to `DataFormat` that receive `&mut Exchange`:

```rust
fn marshal_in_exchange(&self, exchange: &mut Exchange, body: Body) -> Result<Body, CamelError> {
    let _ = exchange;
    self.marshal(body)
}

fn unmarshal_in_exchange(&self, exchange: &mut Exchange, body: Body) -> Result<Body, CamelError> {
    let _ = exchange;
    self.unmarshal(body)
}
```

`MarshalService` and `UnmarshalService` (camel-processor) call the new hooks.
Existing impls (Json, Xml, Zip) inherit defaults unchanged. CSV overrides
`unmarshal_in_exchange` only when `capture_header_record=true`.

## Alternatives considered

- **Crate extraction (`camel-dataformat`)** — YAGNI today; defer until 3+ formats
  with heavy deps or registry/discovery pressure exists.
- **Trait signature change** (breaking) — too high blast radius for one feature.
- **Side-channel via Body metadata** — hacky, pollutes Body type.

## Consequences

- Additive, non-breaking. Existing trait impls compile unchanged.
- Two new trait methods to document and maintain.
- Unlocks Exchange-aware formats beyond CSV (encrypt, sign, audit metadata).
- ADR-0010 (SecurityPolicy pre-pipeline) precedent: project accepts
  Exchange-aware ports when justified.

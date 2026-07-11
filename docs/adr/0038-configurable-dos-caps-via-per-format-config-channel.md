# ADR-0038: Configurable DoS Caps via Per-Format Config Channel

**Date:** 2026-07-11
**Status:** Proposed
**Cross-refs:** ADR-0032 (Exchange-data trust boundary), ADR-0033 (Security defaults), ADR-0017 (DSL snake_case)

> **Note:** Numbered 0038 instead of 0034 because 0034 was already claimed by `0034-controlbus-capability-authz.md`.

## Context

ADR-0033 mandates that each hardened security default has its own per-item escape hatch
— no global "disable hardening" switch. Several data-format DoS caps (`MAX_JSON_BYTES`,
`MAX_XML_DEPTH`) were hardcoded `const` values with no operator override. Additionally,
`ZipConfig` and `CsvConfig` had full config structs that were unreachable from the DSL
because the factory (`builtin_data_format(name: &str)`) could only return
`Default::default()`.

Root cause: the marshal/unmarshal DSL path carried only a format name string at every
layer. There was no configuration lane.

## Decision

Add an optional `config: Option<serde_json::Value>` field to `MarshalStep`,
`UnmarshalStep`, and `DataFormatDef`. A config-aware factory
(`builtin_data_format_with_config`) deserializes this value into the format's own typed
`*Config` struct with `#[serde(deny_unknown_fields)]`, failing closed on unknown keys at
compile time.

Each format gains or already has a `*Config` struct with hardened defaults:
- `JsonConfig { max_bytes }` — default 16 MiB
- `XmlConfig { max_depth }` — default 100
- `ZipConfig` — existing, gains `Deserialize`
- `CsvConfig` — existing, gains `Deserialize`

The `DataFormat` trait is unchanged. Config is applied at construction time.

## Classification Rule

A cap needs a per-item escape hatch **iff** it bounds a resource decision driven by
*exchange data* (untrusted, per ADR-0032). Caps that bound *operator-supplied config
artifacts* are trusted-side and may stay hardcoded.

## ADR-0033 Compliance

Setting a non-default `max_bytes` in YAML **is** the per-item explicit choice mandated
by ADR-0033:
- Scoped to exactly one route step (per-item, not global).
- Has a hardened default that applies when omitted.
- Raising it is an explicit, auditable, per-site operator decision.

There is deliberately no top-level `disable_dos_caps: true` switch.

## Consequences

- **Additive:** existing route files behave identically (config absent → defaults).
- **Fail-closed:** `deny_unknown_fields` catches typos at compile time.
- **Follow-up:** a C-typed schema union for per-key autocomplete is a non-breaking
  refinement, noted for future work.

## Out of Scope

- `MAX_LOOP_ITERATIONS` (EIP cap, separate pattern).
- `convert_body_to: xml→json` path (keeps default depth).
- Custom/user-registered data format registry (deferred; `config: Value` lane generalizes).

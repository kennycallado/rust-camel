# Proposal: 472-validator-refusal-cache

## Why

A schema set that confined Rust discovery accepts but the Java XSD bridge
deterministically refuses is rehashed, re-encoded and re-sent on every
Exchange. Only successful acknowledgements enter the retained schema cache
(`xsd_bridge.rs` `register_with_channel` inserts after the RPC; a refusal
returns `Err` before any insert), and `validate_xml` calls `register`
lazily per Exchange (`compiled.rs`), so a permanently refused set pays one
full register RPC plus a Java compile attempt per message. This amplifies
the Rust/Java acceptance-policy difference tracked in rc-067kq. bd: rc-vavge
(mission 361 round-two review O2).

## What Changes

- Classify bridge errors in `ValidatorError`: a deterministic schema
  refusal (the bridge processed the request and refused the set —
  `COMPILATION_FAILED` response kind) vs transient failures (RPC/transport
  errors, timeouts, channel/generation loss, all other response kinds).
  Only deterministic refusals are cacheable.
- Add a bounded negative cache to `XsdBridgeBackend`, keyed by
  (stable schema id / `SchemaSet::cache_key`, bridge generation). On a
  deterministic refusal, store the typed error and return it for later
  registers of the identical set without hashing, encoding, sending or
  recompiling. Transient errors are never cached and retry on the next
  call. The cache clears on bridge restart/generation change; stale
  (retired-generation) refusal responses never enter it.
- The negative cache shares the existing `schema_cache_max_entries`
  budget with the success cache under a documented combined
  full-clear eviction policy. Success-cache and reseed semantics are
  unchanged.
- Docs: validator README + CONTEXT note the negative-cache behavior,
  key, bound and eviction policy. No new user-visible configuration.

Out of scope (non-goals from the mission order): aligning Rust/Java
acceptance policies (rc-067kq), transport restart handling, schema
discovery, cache bounds unrelated to the negative cache.

## Acceptance criteria

- First deterministic refusal performs exactly one register RPC; N
  subsequent registers of the same set perform zero register RPCs and
  return the same typed error.
- Transient transport errors are not cached: every attempt performs the
  RPC again.
- Bridge restart/generation change clears the negative cache; the next
  register re-attempts the RPC. Stale-generation refusal responses are
  not cached.
- Combined (success + refusal) cache size respects
  `schema_cache_max_entries`; eviction policy is documented.
- Different schema ids do not collide in the negative cache.
- `cargo test -p camel-component-validator`, `cargo fmt --check --all`,
  clippy (validator + all-targets, `-D warnings`), xtask lints, docs
  build, `openspec validate --all --strict` pass.

## Risk budget

- Acceptable: conservative classification (only `COMPILATION_FAILED`
  cached) leaves rare non-deterministic refusal kinds unamortized.
- Acceptable: a deterministic refusal is retried once per bridge
  generation (cache clears on restart) — bounded work, never per-Exchange.
- Out of bounds: changing success-cache insertion, reseed/replay
  semantics, Java bridge behavior, or the `XsdBridge` public trait.

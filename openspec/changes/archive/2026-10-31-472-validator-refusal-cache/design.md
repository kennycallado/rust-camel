# Design: 472-validator-refusal-cache

## Approach

**Error classification.** `ValidatorError` gains a classifier
`is_deterministic_refusal(&self) -> bool`, true exactly for
`CompilationFailed { .. }`. Rationale: `from_bridge_error` maps a
`COMPILATION_FAILED` bridge response kind to `CompilationFailed`, and the
Java bridge emits `COMPILATION_FAILED` (`XsdValidatorService.java`:
`SCHEMA_PARSE_ERROR` and friends) only after it received and processed the
registration — the refusal is a function of the schema bytes, so the same
bytes against the same bridge process recur it deterministically. A bridge
response error of any other kind (`InvalidInput`, `Internal`,
`ResourceNotFound`, `TransformFailed`, `SecurityViolation`, `Unknown`) maps
to a source-less `Transport` and stays unclassified: caching it risks
poisoning the set if the bridge's classification differs across versions;
retrying costs at most one RPC per call. True transient failures
(`Transport` raised by the RPC itself — I/O, timeout, channel loss,
`tonic::Status::Unavailable` — plus generation/restart paths) never reach
the classifier as response errors and are never cached.

**Negative cache.** New field on `XsdBridgeBackend`:
`refusals: Arc<DashMap<(SchemaId, usize), ValidatorError>>` — key is
(schema id, which callers derive from `SchemaSet::cache_key()`, the stable
full-set identity per the xsd-schema-set spec, plus the bridge
`generation` at lookup time). Value is the typed error (`ValidatorError`
is `Clone`).

- Consult: in `XsdBridge::register`, after the success-cache local hit
  check fails and before `register_with_channel`, look up
  `(schema_id, ready.generation)`; on hit return
  `Err(cached.clone())` — no hashing/encoding/RPC/compile.
- Insert: in `register_with_channel`, when the RPC response carries
  `error`: classify; insert only if the error is a deterministic refusal
  AND the published generation still equals `ready.generation` (a refusal
  from a retired process is stale and must not be cached, mirroring the
  existing stale-acknowledgement fence). As with the success insert, the
  `channel.read()` guard that establishes generation currency is held
  across the DashMap insert, closing the check-then-insert gap against
  `launch_and_seed`'s `channel.write().take()`.
- Clear: `launch_and_seed` clears `refusals` when it bumps the
  generation — the single chokepoint for first start and every restart,
  so restarts retry deterministic refusals against the new process (a new
  bridge version may accept what the old one refused).
- `set_schema_cache_max_entries` mirrors its existing clear-on-shrink for
  `refusals`.

**Bound and eviction policy (documented).** The negative cache shares
`schema_cache_max_entries` with the success cache under a combined
full-clear policy in which refusal entries always yield to successes:

- Refusal-path insert of a NEW key: if
  `schemas.len() + refusals.len() >= max`, clear the whole refusal cache
  (refusal entries are re-attempted on demand at one RPC each, mirroring
  the success cache's full-clear rationale); if the success cache alone
  already fills the budget, the new refusal is not cached. Overwrites of
  an existing key are size-neutral and bypass the check.
- Success-path insert of a NEW key: the existing success-cache behavior
  is observably unchanged, but before the insert the policy clears the
  refusal cache whenever the combined size is at/over the bound, so the
  combined size stays ≤ max on that path too.

Known ceiling, to be stated in the README/CONTEXT note: while the success
cache sits at/over capacity, every new-key refusal insert clears the
refusal cache, so refused sets get zero amortization there — exactly
today's behavior (one RPC per Exchange), a documented non-regression
rather than a pathology.

**Unchanged surfaces.** `XsdBridge` trait, `validate` path, reseed
(`reseed_with_channel` replays only retained successes; refused sets were
never retained), restart/transport handling (`is_transport_retryable`),
and public configuration (no new options).

## Affected crates

- `camel-component-validator` (`crates/components/camel-validator`):
  `error.rs` (classifier + unit tests), `xsd_bridge.rs` (field, consult,
  insert, clear, bound, scripted-RPC tests), `README.md`, `CONTEXT.md`.

## Architecture boundaries

Components-layer change only. No Runtime, DSL, Services, Languages or
Functions surface changes: the negative cache lives inside the existing
`XsdBridgeBackend` behind the unchanged `XsdBridge` trait
(`compiled.rs` callers keep calling `register`; they only observe fewer
RPCs and the same typed errors). Domain vocabulary ("XSD schema cache",
refusal language) lives in `crates/components/camel-validator/CONTEXT.md`,
which this change updates; the stable-identity wording aligns with the
existing xsd-schema-set cache-key requirement (spec line 481).

## Alternatives considered

- New `SchemaRefused` error variant instead of classifying
  `CompilationFailed`: rejected — public-API churn and exhaustiveness
  ripples for no functional gain over a predicate.
- Cache all bridge response errors: rejected — non-`COMPILATION_FAILED`
  kinds are not provably deterministic; wrong caching is sticky (until
  restart), wrong retrying is cheap.
- Key by schema id only with clear-on-restart: rejected — the mission
  order requires generation in the key; it is defense-in-depth against a
  missed clear.

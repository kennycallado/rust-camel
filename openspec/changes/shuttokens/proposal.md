# Proposal: shuttokens

## Why

bd rc-4bfnk (P3, discovered-from rc-515m): `ControllerComponentContext`
(camel-core) and `MasterDelegateContext` (camel-master) keep the
`ComponentContext::shutdown_token()` trait default (`None`). Mission 297
left both adopters out of zone. The open question: does a wasm
producer/bean created through these adapters miss Runtime shutdown?

Audit answer (verified by conductor + e_opus pre-flight
ses_f0e3844f6ffeohNNISu6pwM7Wz): **no — `None` is correct.** The token
lineage never flows through endpoint-creation-time contexts:

- The only production per-call callers of the trait method are
  `camel-component-wasm/src/producer.rs:150` and `src/bean.rs:58`
  (`cancel_root`).
- `WasmComponent` captures its `Arc<dyn ComponentContext>` at
  registration time (lib.rs:70-84) and hands `self.registry.clone()` to
  `WasmEndpoint`; the `ctx` passed to `create_endpoint` is used only for
  route health-check registration.
- Production registration binds a slot-mirroring context:
  `camel-bundles/src/lib.rs:421-429` and `camel-cli/src/commands/run.rs:385-393`
  build `RegistryComponentContext::new(..).with_shutdown_slot(ctx.shutdown_token_slot())`,
  so wasm resolves the CURRENT boot's token per call.

Binding the adapters would require either a token snapshot (the
stale-across-stop/start hazard the trait doc warns about) or exposing
the slot through the public trait (API change with zero consumers).
Both are net negatives.

## What Changes

- Boundary-decision comments on both adapters, pointing at the evidence
  above (no behavior change).
- Boundary-lock tests asserting `shutdown_token()` stays `None` on both
  adapters (camel-core `controller_component_context_tests.rs`,
  camel-master `tests/producer_passthrough.rs`).
- Trait-doc extension on `ComponentContext::shutdown_token`
  (camel-component-api): token lineage binds at registration time via
  slot-bound contexts; endpoint-creation adapters must not snapshot.
- One-sentence boundary rule in `camel-component-api/CONTEXT.md` (the
  existing shutdown_token paragraph).
- Fix overstated fail-safety wording in the `start_context` poison
  comment (camel-core `context_lifecycle.rs:101-110`) to describe the
  actual degraded behavior: a poisoned slot makes readers fail CLOSED
  to `None` (`lock().ok()?` in registry.rs:196-199), so wasm's
  `unwrap_or_default()` mints an uncancelled local root — shutdown
  observation is lost for that boot (same observable as unbound
  contexts), never born-cancelled. Poison needs a panic while holding a
  one-line read/write lock — near-zero probability.
- Rewrite the stale `MasterDelegateContext` field-visibility comment
  (`endpoint.rs:63-66`): fields must STAY `pub(crate)` — `leadership.rs`
  builds the struct via field-init.
- Ratchet: `ratchet-cancel-tokens.max` 9 → 7 (current count; the two
  wasm seed-inventory lines went `unwrap_or_default()`), delete the two
  stale inventory lines.
- bd comment on rc-4bfnk documenting the unreachability ruling.

Excluded: any trait API change, any token snapshot, any new binding.
File P4 follow-up bd: lint-cancel-tokens blind spot
(`Option<CancellationToken>::unwrap_or_default` uncounted).

## Acceptance criteria

- Audit outcome documented on bd rc-4bfnk with evidence pointers.
- Boundary-lock tests present and green for both adapters.
- Poison-comment wording matches the fail-closed reality.
- `cargo xtask lint-cancel-tokens` green against max 7.
- No behavior change; no public API change.

## Risk budget

Docs + comments + tests + a monotone ratchet lowering. No runtime risk.
Out of bounds: touching token resolution logic, the slot mechanism, or
httpsweep's camel-http zone.

Affected crates: camel-core, camel-component-api, camel-master (+
scripts/xtask ratchet file). bd: rc-4bfnk.

# Design: shuttokens

## Approach

Document-and-lock, no wiring. The audit (conductor + e_opus pre-flight
ses_f0e3844f6ffeohNNISu6pwM7Wz, worktree 76687d95) established that the
`shutdown_token()` trait method has exactly two production per-call
callers — wasm `producer.rs:150` and `bean.rs:58` — and both resolve
through the `Arc<dyn ComponentContext>` captured by `WasmComponent` at
REGISTRATION time. Production registration sites
(camel-bundles lib.rs:421-429, camel-cli run.rs:385-393) bind
`RegistryComponentContext` with `with_shutdown_slot`, and
`start_context` (context_lifecycle.rs:97-110) re-writes the slot on
every boot, so the resolved token is the current boot's (sole
exception: a poisoned mutex — readers fail closed to `None` and mint an
uncancelled local root; see the poison-wording fix below).
`CamelContext` itself is the third bound implementor (context.rs:1109).

Consequence: the `&dyn ComponentContext` threaded through
`MasterDelegateContext` (endpoint.rs:50, leadership.rs:283) and the nine
`ControllerComponentContext` construction sites never reaches a token
consumer. Keeping the `None` default is the correct boundary decision;
the change makes that decision load-bearing by documenting it at the
trait, the adapters, the crate CONTEXT.md, and locking it with tests.

## Affected crates

- camel-component-api: trait-doc extension on `shutdown_token`
  (component_context.rs) — the binding-time boundary rule; one sentence
  in `crates/components/camel-component-api/CONTEXT.md` shutdown_token
  paragraph.
- camel-core: boundary comment on `ControllerComponentContext`;
  boundary-lock test in `controller_component_context_tests.rs`;
  poison-comment wording fix in `context_lifecycle.rs:101-110`.
- camel-master: boundary comment + stale field-visibility comment
  rewrite on `MasterDelegateContext` (endpoint.rs:63-66 — fields stay
  `pub(crate)` because leadership.rs builds via field-init);
  boundary-lock test in `tests/producer_passthrough.rs`.
- scripts/xtask: `ratchet-cancel-tokens.max` 9 → 7, drop the two stale
  wasm seed-inventory lines.

## Architecture boundaries

Data/control plane untouched. No trait signature changes, no new public
surface, no runtime code path changes — comments and `#[cfg(test)]`
code only. The decision being documented respects the existing
cancellation architecture (ADR-0043 pipeline cancellation tree for
consumer-lifetime tokens; the shutdown-slot mirror design from mission
297 / rc-515m for producer-side lineage). camel-master remains
decoupled from camel-core (no slot type crossing crate boundaries —
that was one of the rejected alternatives).

## Alternatives considered

1. **Snapshot the outer token into MasterEndpoint at `create_endpoint`
   time** — rejected: dead code today (no consumer reads it through
   this path) and introduces the stale-across-stop/start hazard the
   trait doc explicitly warns about.
2. **Expose `Arc<Mutex<CancellationToken>>` slot through the
   `ComponentContext` trait** — rejected: public API change with zero
   consumers; also drags a camel-core-internal representation into the
   component contract.
3. **Do nothing (silent None)** — rejected per bd acceptance criteria:
   the unreachability must be documented or the next auditor re-walks
   the same chain.

Single-phase change (no `## Phase N` headings in tasks.md).

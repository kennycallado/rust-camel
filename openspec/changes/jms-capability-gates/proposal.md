# Proposal: jms-capability-gates

## Why

Mission 509 (durable topics) left three parallel shape-copies in
`crates/components/camel-jms/src/diagnostics.rs`:

- `ensure_tls_capability` (:83), `ensure_selector_capability` (:107),
  `ensure_durable_capability` (:133) — same version-compare + message shape.
- `evaluate_startup_selector_gate` (:157), `evaluate_startup_durable_gate`
  (:182) — same startup ladder; the pool's health-monitor gate is a three-arm
  else-if ladder (`component.rs:1284-1335`).
- `selector_attach_gate` / `durable_attach_gate` on the pool
  (`component.rs:859` / `:913`), plus `get_or_create_slot(broker_name,
  selector_required, durable_required)` (`consumer.rs:283`, `:1020`) carrying
  two positional booleans a reader cannot tell apart (ADR-0091
  named-fields-over-positional).

A fourth capability would copy the shape a fourth time. Fold the copies onto
one descriptor before that happens. (bd rc-06dy1, P3.)

## What Changes

This is a **behaviour-preserving refactor**: byte-identical gate messages,
identical latch ordering (monitor TLS→SELECTOR→DURABLE; slot latch
selector-then-durable; attach preflight selector-then-durable), identical
fail-closed semantics. No gate's message, latch ordering, or positional-boolean
call-site semantics may change — if the implementation cannot hold that line,
the mission stops as `blocked: needs-plan`.

### 1. One descriptor, one evaluator per path (`diagnostics.rs`)

```rust
pub(crate) struct CapabilityGate {
    /// The `Health` capability string the bridge must advertise.
    pub capability: &'static str,
    /// Minimum bridge release advertising the capability; `None` when the
    /// requirement is the pinned build itself.
    pub min_bridge_version: Option<&'static str>,
}
```

- Per-capability constants `CapabilityGate::TLS` (`broker-tls-v1`,
  `min_bridge_version: None`), `::SELECTOR` (`message-selector-v1`,
  `Some(crate::BRIDGE_VERSION)`), `::DURABLE` (`durable-subscribers-v1`,
  `Some(DURABLE_SUBSCRIBERS_MIN_BRIDGE_VERSION)`) replace the three copies.
- `missing_clause()` derives the three authored missing-capability clauses from
  the descriptor data: `None` → "lacks {cap}; use the pinned bridge";
  `Some(min == pinned)` → "lacks {cap}; requires a jms bridge >= {pinned}"
  (selector's min IS the pinned build; no redundant repetition);
  `Some(min != pinned)` → "lacks {cap}; requires a jms bridge >= {min}
  (pinned {pinned})". Two authored per-capability attach fragments
  (`attach_support()`: "message-selector support" / "durable-subscriber
  support"; `attach_timeout_tail()`: "the pinned jms bridge" /
  "a jms bridge >= 0.9.0") map capability → attach-gate noun/tail in ONE
  match — the noun does not derive mechanically from the wire string
  ("durable-subscribers-v1" verifies "durable-subscriber support").
- ONE attach evaluator `ensure_capability(capabilities, gate, required)` and
  ONE startup evaluator `evaluate_startup_capability_gate(capabilities, gate,
  required)` carry the version-compare + message shape once.
- The five existing per-capability functions
  (`ensure_tls_capability`, `ensure_selector_capability`,
  `ensure_durable_capability`, `evaluate_startup_selector_gate`,
  `evaluate_startup_durable_gate`) stay with unchanged names, signatures, and
  behaviour as one-line delegations — message-pinning tests call them directly
  and must pass unchanged.
- Deleted: `MISSING_BROKER_TLS_CAPABILITY`,
  `missing_message_selector_capability()`, `missing_durable_capability()` —
  `missing_clause()` is the single source of all three texts.

### 2. Monitor ladder → table (`component.rs`)

The health-monitor three-arm else-if ladder (:1284) becomes a table over
`[(&TLS, tls_configured), (&SELECTOR, slot.selector_required),
(&DURABLE, slot.durable_required)]` scanned in order (latch/gate ordering
preserved; first armed failure wins, as the ladder short-circuits today). Warn
text generalizes to "Bridge for '{}' lacks {capability}; killed and parking
slot." — byte-identical for all three arms. Park reason stays
`bridge {bridge_version} {clause}`.

### 3. Pool attach gates → one gate (`component.rs`)

`selector_attach_gate` + `durable_attach_gate` fold into one
`capability_attach_gate(slot, channel, gate)`; the Health-RPC-and-park shape is
written once. The consumer attach preflight (`consumer.rs:300-314`) runs the
folded gate in a `[SELECTOR, DURABLE]` loop — order and terminal-error mapping
preserved.

### 4. Positional booleans → named requirements (`component.rs`, `consumer.rs`)

`get_or_create_slot(broker_name, selector_required, durable_required)` becomes
`get_or_create_slot(broker_name, requirements: CapabilityRequirements
{ selector, durable })` (ADR-0091 named fields over positional bools). The
three latch points keep selector-then-durable store order. Production call
sites (`consumer.rs:283`, `:1020`) build the struct from the existing
`selector_required()` / `durable_required()` endpoint predicates; three test
call sites adapt mechanically (`CapabilityRequirements::default()`) with no
assertion change. The slot's `AtomicBool` fields and
`start_bridge_process` arguments are unchanged.

## Messages unchanged — explicit statement

Every surfaced string is byte-identical to today:

| Path | Message (reason bytes) |
| --- | --- |
| TLS gate (attach + monitor) | `bridge lacks broker-tls-v1; use the pinned bridge` |
| TLS startup gate | `bridge {binary_path} lacks broker-tls-v1; use the pinned bridge` |
| Selector gate | `bridge lacks message-selector-v1; requires a jms bridge >= 0.9.1` |
| Durable gate | `bridge lacks durable-subscribers-v1; requires a jms bridge >= 0.9.0 (pinned 0.9.1)` |
| Startup selector/durable | `bridge 0.9.1 {clause}` (pinned prefix, clause as above) |
| Attach timeout (selector) | `could not verify message-selector support: bridge health RPC failed or timed out; the bridge must report capability message-selector-v1 from the pinned jms bridge` |
| Attach timeout (durable) | `could not verify durable-subscriber support: bridge health RPC failed or timed out; the bridge must report capability durable-subscribers-v1 from a jms bridge >= 0.9.0` |
| Attach miss warn | `Bridge for '{broker}' lacks {capability}; killed and parking slot.` (all three arms) |
| Monitor miss reason | `bridge {bridge_version} {clause}` (TLS arm included — byte-identical to today's `MISSING_BROKER_TLS_CAPABILITY` interpolation) |

New byte-exact table-driven tests pin all five evaluator reasons (the three
attach clauses + the two startup forms) so any drift fails CI. The existing
message-pinning tests in `diagnostics.rs` and `consumer.rs` are not edited.

**No spec deltas** (`skip_specs: true`): the JMS capability-gate behaviour is
pinned by existing unit tests and the byte-exact table above; no canonical
capability in `openspec/specs/` changes.

## Non-goals

- No new capability, no version-bump policy change, no message rewording, no
  bridge change, no retry/channel-refresh changes (`rc-qpkok`, mission 541,
  owns those).

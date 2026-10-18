# Design: fix-fanout-claim-split

## Approach

Centralize branch-exchange construction behind one helper in `camel-api`,
then route every fanout clone site through it (bd rc-ulmo6 `design`
field, binding).

The helper lives on `Exchange` itself because both inputs it needs — the
`in_flight_claim` field and `InFlightClaim::split()` — are `camel-api`
types, and `camel-processor`/`camel-core` both need it:

```rust
// crates/camel-api/src/exchange.rs
impl Exchange {
    /// Clone for a fanout branch copy. Unlike [`Exchange::clone`], a
    /// carried in-flight claim is split onto the copy (claimfamily):
    /// the sibling counts the copy independently and releases when the
    /// copy completes; the original claim stays live.
    pub fn branch_clone(&self) -> Self {
        let mut copy = self.clone();
        copy.in_flight_claim =
            self.in_flight_claim.as_ref().map(|claim| claim.split());
        copy
    }
}
```

`InFlightClaim` remains non-`Clone` (forbidden by bd design field).

A second construction seam needs the same semantics: split fragments
are NOT clones — the splitter builds each fragment via
`fragment_exchange(parent: &Exchange, body: Body) -> Exchange`
(`crates/camel-api/src/splitter.rs:418`), a fresh `Exchange::new` with
piecemeal header/property copies. A plain-clone swap cannot cover it.
`fragment_exchange` therefore becomes the fragment branch constructor:
when the parent carries a claim, it splits a sibling onto the fragment
(`ex.in_flight_claim =
parent.in_flight_claim.as_ref().map(|claim| claim.split())`). This is
the same claimfamily semantics as `branch_clone()` at a
fresh-construction seam, and it is what makes the split-segment
regression scenario provable (r_glm plan-review finding, critical:
production fragments carry no claim, so clone-site routing alone has
no GREEN path for the split scenario). Tar/archive per-entry exchanges
(`tar_splitter.rs:519-534`-style `Exchange::new` from parsed entry
data, no claimed parent relation) stay out of scope.

Site classification (the STAGE 3 audit — Task 2.3 step 1 for
camel-processor and Task 3.1 step 1 for camel-core): every
`exchange.clone()` / `Exchange::clone` site under
`crates/camel-processor/src` and every exchange-clone site under
`crates/camel-core/src/lifecycle/adapters/` (including
`step_compilers/splitting.rs`, `exchange_uow.rs`, and
`route_compiler.rs` handler original stash and Security/CircuitBreaker
boundary snapshots at lines ~602/942/960/971) is labeled:

- **branch-escape**: the copy is sent to another task, endpoint, or
  outlives the parent's claim scope (wire_tap tap exchange, multicast
  subscriber copies, splitter/streaming splitter segment copies,
  load_balancer destination calls, recipient_list recipient calls,
  do_try unmatched forwards, error_handler redelivery/dead-letter
  snapshots, splitting step parents) →
  `branch_clone()`.
- **in-scope**: the copy completes before the parent's claim drops and
  never escapes (content_enricher's local enrich copy where the parent
  waits on it) → still routed through `branch_clone()` when it is sent
  to another await point; plain `clone()` only if it never leaves the
  current claim scope, with the reason recorded in the task report.
- **test-harness**: sites inside `#[cfg(test)]` modules (verified
  examples: `resequencer/mod.rs:549` inside the cfg-test boundary at
  `:518`, `split_segment.rs:544/650/736` inside the boundary at `:381`)
  are out of scope — recorded in the audit table as test-harness, not
  patched.

Default rule: when in doubt, a site is branch-escape (over-counting
only delays settle until the copy completes — correct; under-counting
reproduces the flake).

## Affected crates

- `camel-api`: add `Exchange::branch_clone()` + unit test
  (`sibling_split_and_original_unaffected`); extend
  `fragment_exchange` to split the parent's claim onto each fragment.
- `camel-processor`: route listed fanout sites through `branch_clone()`;
  add integration tests (wiretap + multicast/split drain-gauge
  assertions).
- `camel-core`: route adapter clone sites under
  `lifecycle/adapters/` (`step_compilers/splitting.rs`,
  `exchange_uow.rs`, `route_compiler.rs` and any other clone site the
  audit surfaces) through `branch_clone()` after confirming they fan
  out to branches.

## Docs deliverable

- `CONTEXT-MAP.md` (root): extend the claimfamily entry's sibling
  minting-site enumeration (~line 128, "the three pipeline drain sites
  and the aggregate loop's oneshot dispatch paths") with the new
  fanout `branch_clone()` minting class, as part of this change.

## Architecture boundaries

- Data/control plane split untouched: the helper changes no routing
  semantics, only claim residency accounting (observability/ draining).
- Runtime / DSL / Components / Services / Languages / Functions: only
  the data plane's exchange-construction seam changes. No new crate
  deps. Respects the canonical-builder/exchange ownership rules
  (CONTEXT-MAP.md: exchange lifecycle is a core concern; the claim is
  RAII, release paths are unchanged — split claims release exactly
  once on drop, the existing mechanism).
- Canon spec `observability` already mandates "Fanout SHALL split one
  claim per subscriber copy"; this change makes the fanout code comply.
  Delta only, `openspec/specs/**` untouched.

## Phases

Omitted — single-phase change: helper + site routing + tests form one
coherent slice; the per-task worker loop handles ordering.

## Blessing hash derivation

Spec/plan bless anchors are computed with the repo-standard script from
the main checkout root:

```
bash /home/kenny/dev/rust-camel/scripts/bless-hash.sh \
  <worktree>/openspec/changes/fix-fanout-claim-split
```

(executes the prebuilt xtask hash helper; this is the canonical
derivation, not ad-hoc `find|sha256sum`.)

## Alternatives considered

- **Make `InFlightClaim` `Clone` with a shared decrement flag** —
  explicitly forbidden by bd design field; hides double-counting
  hazards at every clone.
- **Split inside `Exchange::clone` itself** — makes every clone
  (including test-local and in-scope copies) increment the gauge and
  keeps the "clone drops the claim" doc invariant inconsistent;
  `clone()` is also used by non-fanout machinery where a silent gauge
  bump is surprising. Explicit `branch_clone()` keeps intent visible
  at each site.
- **Processor-side helper instead of `Exchange` method** — duplicates
  field access across two crates and cannot be used by camel-core
  adapters without re-export gymnastics; the field and `split` are
  both in `camel-api`.

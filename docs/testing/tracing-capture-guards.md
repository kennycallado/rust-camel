# Tracing capture guards: convention and inventory (rc-puo4f)

Convention for the `ensure_global_tracing_default` test guard and the
inventory of every guard copy, as of this change (base `cb34fec4`
plus the consolidation below; verified 2026-09-21 in the `guardconsol`
worktree). Source of the copies: commit `36ca7c73` (mission 167, bd
rc-img5), which audited 29 files repo-wide and healed 21 of them
with the `c3853198`-pattern OnceLock guard. This note consolidates
the copies that have a natural shared home and records the rules for
new ones.

## The poison mechanism

`tracing` caches each callsite's `Interest` process-wide from its
FIRST macro execution, evaluated against the executing thread's
dispatcher. A subscriber-less thread resolves `NoSubscriber` and the
cache stores `Interest::never` for that callsite. A later test that
captures through a thread-local `set_default`/`with_default`
subscriber then gets its events filtered before they reach the layer:
the capture asserts fail, or worse, negative asserts pass vacuously.
The failure is intermittent under parallel test load because it
depends on which sibling test hits the shared callsite first. First
diagnosed and fixed in `c3853198` (bd rc-zushg): 0 fails across 286+
worst-case runs, from 27/40 on the minimal pair.

## The guard

```rust
fn ensure_global_tracing_default() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INIT.set(()).is_ok() {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
    }
}
```

Every capture helper (`capture_warns`, `capture_sink`,
`capture_guardrail`, and similar) calls it before installing its
thread-local subscriber. The bare registry as global default heals
prior poison (a dispatch install rebuilds the interest cache) and
floors future rebuilds at `sometimes`, so captures consult their own
subscriber again.

## NO-UNWRAP rule

Never `unwrap`/`expect` the result of `set_global_default` in test
code. The call is first-wins per process: an `Err` means a sibling
guard, or an earlier test, already installed a global default — which
is the desired state. Discard the result with `let _ =`. Unwrapping
would turn benign first-wins into an intermittent panic under
parallel load (e_glm stage-4 note, mission 167).

## Shared home vs per-file copy

Consolidate into a natural shared home only when one already exists
within the same test-binary reachability:

- A crate-level `#[cfg(test)]` test module (`camel-dsl`:
  `src/lib.rs` `test_support`).
- A crate-local capture helper module (`camel-config`:
  `src/config.rs` `log_capture`).
- A `tests/common` module for integration binaries
  (`camel-config`: `tests/common/mod.rs`).

Where no natural home exists, keep the per-file copy. Do not invent
new crates, `pub` API, or cross-crate test-deps to deduplicate
(mission 167 convention decision). Multiple copies inside one test
binary are harmless: each copy has its own `OnceLock`, exactly one
`set_global_default` succeeds, and the losers ignore the expected
`Err`. The guard is per-binary state; per-binary duplication is
inherent and accepted.

## Inventory (22 guard sites)

### Natural homes (5)

| Location | Serves |
| --- | --- |
| `camel-dsl/src/lib.rs` (`test_support`) | `discovery.rs`, `env_int_probe.rs` call sites |
| `camel-config/src/config.rs` (`log_capture`) | `config_tests/parity_golden_tests.rs` call site |
| `camel-config/tests/common/mod.rs` | `tests/cache_repo_config.rs` call site (consolidated here by rc-puo4f) |
| `camel-component-wasm/tests/common/mod.rs` | `tests/source_bind_gate.rs` call site (consolidated here by rc-puo4f) |
| `camel-core/tests/route_interception/common.rs` | `tests/route_interception/support.rs` `capture_tracing` call site (floor added by rc-6jarb) |

### Per-file copies, no natural home in the binary (11)

`camel-core` lib binary (no crate-level test module):

- `src/cache/disk_offload_tests.rs` (shared with
  `disk_offload_reclaim_tests.rs` within the disk-offload tree only)
- `src/cache/redb.rs`
- `src/lifecycle/adapters/route_controller_trait_tests.rs`
- `src/lifecycle/adapters/consumer_management.rs`
- `src/step/function_step.rs` (inline in the span test)

`camel-processor` lib binary (no crate-level test module;
`data_format/test_util.rs` is data-format-scoped and keeps its own —
its header records a deliberate do-not-merge decision for capture
recorders):

- `src/wire_tap.rs`
- `src/cache_eip.rs` (inline in `EventRecorder::install`)
- `src/data_format/test_util.rs` (inline in `warn_capture`)
- `src/error_handler.rs` (inline in `capture_debugs`)
- `src/intercept_compose.rs`
- `src/multicast_segment_tests.rs`

### Per-file copies, sole copy in their binary (6)

- `camel-bundles/src/lib.rs` (inline in `logquiet_regression_tests`)
- `camel-component-cxf/src/pool_env_test.rs` (inline in `capture_sink`;
  floor added by rc-6jarb)
- `camel-integration-test/src/doc_parse_test.rs`
- `camel-ws/src/lib.rs`
- `camel-otel/src/service.rs`
- `camel-component-api/src/network_retry_tests.rs` (pattern origin,
  `c3853198`)

Count note: `36ca7c73` changed 22 files — 19 guard bodies plus 3
call-site-only edits (`discovery.rs`, `env_int_probe.rs`,
`parity_golden_tests.rs`). `network_retry_tests.rs` predates it
(`c3853198`, pattern origin), and rc-puo4f relocated the
`cache_repo_config` and `source_bind_gate` bodies into their crates'
`tests/common`. rc-6jarb then added the two floors for the
previously-uncovered `route_interception_test` and camel-cxf lib
binaries — 22 bodies total today. Any new capture test reuses
its binary's existing guard or follows this table's classification.

Not this pattern: `camel-component-mcp/tests/common/mod.rs` installs a
global *recording* subscriber via `set_global_default` (OnceLock plus
`expect`) because the rmcp transport polls on its own task and a
thread-local capture would miss those events. That is a deliberate,
self-documented different pattern, not a guard copy: its `expect` is
justified in its own module docs — no other global default exists in
those binaries, so an install failure is a hard test-infra error, not
a first-wins race.

# Design: httpflake

## Approach

Test-harness fix inside `crates/components/camel-http/src/lib.rs`
(`#[cfg(test)]` surfaces only). Four legs, per the pre-flight ruling and
the spec-bless fixes:

### Leg 1 — close the reset race (primary root cause)

`setup_consumer_on_free_port` acquires `REGISTRY_TEST_MUTEX` before
`stage_listener` and holds it through readiness-complete (the registry-poll
loop AND the 8-iteration tail-yield loop), releasing before returning the
`(port, rx, token)` triple. Scope rationale:

- Starting at `stage_listener` closes both exposed windows: (a) reset
  clearing `staged` mid-setup (dropped socket → fresh bind → rare
  `EADDRINUSE` → same panic signature) and (b) reset clearing `entries`
  after cell init but before the poll observes it (orphaned entry →
  `bound_addr` `None` forever).
- Ending after the tail yields keeps the helper's return contract
  unchanged; test bodies after `setup` never re-consult the registry, so a
  later reset is harmless.
- The mutex is the SAME `std::sync::Mutex` the ~20 reset-calling tests
  already hold; the inner registry `Mutex` is a leaf never held across
  await, so no lock-ordering cycle exists (no deadlock).
- Holding a sync guard across await points matches the existing pattern at
  the 29 guard sites in the same module; a targeted
  `#[allow(clippy::await_holding_lock)]` is added only if clippy demands
  it on the new site (verify against the existing sites, which pass CI
  clippy today).

### Leg 2 — robust readiness budget (secondary: starvation)

The readiness loop keeps the registry-poll probe (rc-w1u9 canon; TCP probes
are forbidden) but:

- Deadline 5 s → 10 s. Justification: a bounded policy value, not a
  measured constant — 2× the previous budget and far above the
  sub-second CFS scheduling latency of the 12-core box, while staying an
  order of magnitude under CI job timeouts. The deadline starts after
  mutex acquisition (std Mutex has no timed lock; acquisition is bounded
  in practice by the µs-scale critical sections of the other holders).
  Fail-loud is preserved — a genuinely dead server still panics.
- Backoff 1 ms doubling, capped at 64 ms (1, 2, 4, …, 64, 64, …). Cuts the
  poll count for a full 10 s wait from ~5000 to ~300, reducing wake and
  registry-lock pressure exactly when the machine is loaded.
- Panic text extended with a cause hint ("registry entry absent —
  concurrent reset or starvation") so the next human triages in seconds.

### Leg 3 — regression test (bounded, contention-proven evidence)

`readiness_survives_concurrent_registry_reset` (a `#[tokio::test]` in the
same test module):

- A hammer OS thread loops a LEGAL reset:
  `{ if try_lock fails { contended.fetch_add(1); lock(); } reset(); }`
  under a stop flag. It simulates the pattern all ~20 real reset callers
  follow; a bare reset would defeat the fix and prove nothing.
  `ServerRegistry::reset()` is `#[cfg(test)]`, so the test compiles only
  under test.
- The test drives `setup_consumer_on_free_port` setups sequentially on
  fresh ephemeral ports, cancelling each token per iteration. Termination
  rule (executable, consistent across artifacts): ALWAYS run at least 25
  setups; if `contended` is still 0 after 25, continue setup iterations
  until it reaches 1 or 50 total setups have run. After the loop, the test
  explicitly asserts `contended >= 1` — proving at least one reset attempt
  actually overlapped a protected setup window (post-fix the first
  setup's mutex hold makes contention immediate) — and that every setup
  that ran became ready within its deadline.
- Cleanup is unconditional: a Drop guard sets the stop flag AND joins the
  hammer thread, so a panic mid-test cannot leak a live reset loop that
  would corrupt unrelated tests. The join is part of the guard, not the
  happy path only.

### Leg 4 — loaded-soak evidence (mission mandate)

Harness (feature worktree only — repo law forbids cargo in the main
checkout): 12 shell spin burners (`while :; do :; done` background loops)
pinned across the cores, then a loop of full
`cargo test -p camel-component-http --lib -- --test-threads=12` runs
(N = 15), counting panics matching `did not become ready`. A targeted
multi-filter variant (libtest OR-filters selecting the 8 content-type
tests plus reset-heavy registry tests) raises collision density for the
BEFORE run. BEFORE (pre-fix commit): ≥ 1 readiness panic across the runs.
AFTER (post-fix commit): 0 panics across 15 loaded runs. Results land in
the park report as `evidence.repro_before` / `evidence.repro_after`.

## Affected crates

- `camel-component-http` (crates/components/camel-http): readiness helper,
  readiness loop constants/panic text, one new regression test. Nothing
  else. No `Cargo.toml` change.

## Architecture boundaries

All edits live in `#[cfg(test)]` code inside the component crate — the
production HTTP consumer/producer/registry surfaces are untouched. The
staged-listener law (ADR-0070: bind, keep, stage, one-shot consumption) is
preserved exactly; the readiness canon remains the registry poll with
mark-ready-after-bind (rc-w1u9). No DSL, CLI, core, or service crates are
involved; no cross-crate API changes.

Single-phase change — no `## Phases` section, no `## Phase N` headings in
tasks.md.

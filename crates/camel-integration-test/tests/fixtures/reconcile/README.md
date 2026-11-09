# Reconcile fixtures — reconstructed, not the customer's originals

These files reproduce the workload shape described in `bd rc-m01r9` by the
camel-cache team. **They are reconstructions.** The customer's files live in a
private repository and were not available in this workspace; the loop shape,
128-key chunking, ~10 phases, the 11 large properties read and written per
iteration, the oracle-as-text boundary and the synthetic `viirs:`/`graph:`/
`historic:` ids were transcribed from their written description and from the
behaviour probes recorded in `openspec/changes/archive/*/rhai-reconcile-bench/`.

File names and the document schema match what the customer described, so their
originals can replace these **drop-in**. If they do, re-run both measurements
(the 1200 control and the 5001 bench) and update the numbers below; do not mix
runs from different fixtures.

## Numbers measured on the reconstruction (2026-11-09, main @ 9bdfd099b)

- 1200 control (`reconcile_budget_1200_control`, debug): 6.6-7.4 s, reply
  `{"clusters":1200,"errors":0,"members":1200,"retiredIds":0}`.
- 5001 bench (`reconcile_budget_5001_bench`, release, `#[ignore]`):
  28.0 / 52.0 / 31.1 s over three runs (median ≈31 s) under concurrent fleet
  load; representative phase split: generator 376 ms, reconcile 27.5 s,
  oracle 61 ms. No timing assertion exists anywhere — these are observations.
- The customer's `direct:` timeout is 30 s; two of three local runs exceed it,
  which reproduces their failure. Their 0.56 baseline was ~8.3 s.

## What is NOT verified

A v0.56.0 comparison is **blocked**: the era engine cannot marshal 5001-entry
container properties across the script boundary at all (`to_json` is map-only
and size-capped), so the same fixture cannot execute on both sides. That
boundary change is the defect under investigation, not a measurement artefact.
See `bd rc-m01r9` and the mission-551 park for the probe transcripts.

## Recipe

```sh
# control (CI-shaped, debug)
cargo test -p camel-integration-test --features lang-rhai \
  --test reconcile_budget_test reconcile_budget_1200_control -- --nocapture

# bench (release, opt-in)
cargo test -p camel-integration-test --release --features lang-rhai \
  --test reconcile_budget_test -- --ignored --nocapture
```

The `lang-rhai` shadow feature must be passed: without it the test file compiles
out (`#![cfg(feature = "lang-rhai")]`) and the run is vacuously green.

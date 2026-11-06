# Compile probe — mission 510 (builderlayer)

Pre-design compile probe. Run BEFORE trusting the design (mission order: hazard clause).

## Manifest

- Probe crate: this directory (`probe/Cargo.toml`), standalone workspace.
- Deps: `camel-api` (path: `../../../../crates/camel-api` — fixed for standalone runs,
  see `Cargo.toml` path override below), `tower 0.5` with features
  `["util", "timeout", "limit", "buffer"]`, `tokio 1` full, `serde_json 1`.
- Toolchain: rustc 1.99.1 (58cf8f9c 2026-10-13), cargo 1.99.1, tower 0.5.3.
- Date: 2026-11-06.

## Commands

```sh
cargo check --tests
cargo test
```

The `camel-api` path dep is relative (`../../../../crates/camel-api`), so the probe
reproduces from any checkout of this change directory. The probe declares its own
`[workspace]` so it never joins the main workspace build.

## Findings (all reproducible from this source)

1. **P1 — Timeout wraps BoxProcessor.** `TimeoutLayer::layer(box_processor)` + error
   adapter fits `BoxProcessor::new` (bounds: service `Clone + Send + Sync + 'static`,
   error `Into<BoxError>`, future `Send`). Runtime: timeout fires at 52ms on a 50ms
   budget (ratio 1.04); error source chain reaches `tower::timeout::error::Elapsed`
   (Display: "request timed out").
2. **P2 — ConcurrencyLimit holds.** Cap 2 under 20 concurrent clones: max in-flight 2,
   ratio 1.000 (envelope ≤1.15).
3. **P3 — RateLimit is NOT Clone (HAZARD CONFIRMED).**
   `RateLimit<BoxCloneSyncService<Exchange, Exchange, CamelError>>: Clone` fails
   (E0277, see the commented-out `probe_rate_direct` in src/main.rs). Stateful layers
   REQUIRE `tower::buffer::Buffer`. Buffered RateLimit shares state: 5 clones at rate
   2/s took 2.01s (required 2.0s, ratio 1.006).
4. **P4 — ADAPTER DEADLOCK FINDING (design-critical).** An adapter that clones the
   layered service inside `call()` and re-`ready()`s the clone deadlocks permit-style
   layers under concurrency (instrumented probe: every concurrent handle holds an
   unconsumed `poll_ready` permit forever; tasks stuck at "ready ok, calling"). The
   adapter must pass `poll_ready` through and consume on the SAME handle in `call()`.
   `run_steps`' per-invocation `processor.clone()` is safe: cloning resets permit state
   (tower ConcurrencyLimit manual Clone) and each clone acquires+consumes on itself.
5. **P5 — OutcomeSegment adapter works.** `OutcomeSegment` → `Service<Exchange>`
   adapter with unconditional `CamelStop=true` re-marking on `Stopped` compiles and
   passes; `is_camel_stop` re-derivation in `run_steps`' Process arm propagates the
   stop.
6. **P6 — mdbook doctest constraint.** Plain ```rust fences in docs CANNOT resolve
   workspace crates (`mdbook test` compiles standalone rustdoc). Std-only fences run;
   camel-typed fences fail with E0432. Guide must use `{{#include}}` + `rust,ignore`;
   "doctests compile and run" is carried by rustdoc doctests (`cargo test --doc`).

## Probe evidence table

| Probe | Metric | Configured | Measured | Envelope | Result |
|---|---|---|---|---|---|
| P2 concurrency | max in-flight / cap | 2 | 2 | ≤1.15 | PASS (1.000) |
| P3c rate | elapsed / required delay | 2.0s | 2.014s | ≤1.25 | PASS (1.007) |
| P4 timeout | elapsed / deadline | 50ms | 52.06ms | ≤1.25 | PASS (1.041) |
| P4 no early fire | elapsed ≥ deadline | 50ms | 52.06ms | ≥ 1.0 | PASS |
| P4 error type | Elapsed in source chain | required | present | — | PASS |

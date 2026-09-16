# Proposal: clidiet

## Why

camel-cli carries two optional global-allocator features (`jemalloc`,
`mimalloc`). Under `--all-features` both allocator stacks resolve into the
dependency graph even though only jemalloc can ever back the binary — the
mimalloc `#[global_allocator]` block in `main.rs` is `cfg`-gated behind
`not(feature = "jemalloc")` and no build tooling enables mimalloc anywhere.
mimalloc is dead weight: an unused, structurally-unreachable alternative
(bd rc-rrz6a). In parallel, a minimal deployment (`http`-only routes)
still links grpc/wasm/llm/mcp/mqtt/surrealdb/exec bridges, the LSP
server, and five language runtimes because they are non-optional
camel-cli dependencies or unconditional feature forwards (bd rc-g0009).
The ride-along sweep bd rc-kyq15 (camel-cli jobhelp wave) has only closed
items and is verified here.

## What Changes

- **Allocator dedup (rc-rrz6a):** remove the `mimalloc` feature, the
  `mimalloc` optional dependency, and the mimalloc `#[global_allocator]`
  block from `crates/camel-cli`; prune `Cargo.lock` (camel-cli's `"mimalloc",` dependency edge is
  removed; the `[[package]]` entries stay because the benchmarks
  fixture keeps its own independent mimalloc dep — jemalloc, the
  surviving override, is unaffected). The jemalloc feature,
  `allocator_metrics.rs` gauges (ADR-0066), and `tikv-jemalloc-ctl` stay
  untouched. Default builds keep the system allocator; release/musl
  builds keep the documented jemalloc opt-in.
- **Slim feature profile (rc-g0009):** make the camel-cli-controllable
  heavy surface optional — the LSP server stack (`camel-lsp`,
  tower-lsp; ariadne stays non-optional — `camel lint` shares it) and
  the `camel-core` language forwards (`lang-js`, `lang-rhai`,
  `lang-jsonpath`, `lang-xpath`, `lang-minijinja`) become camel-cli
  features; `camel-core` is consumed without the language features and
  re-enabled through them. Of these, js/rhai/jsonpath/xpath crates are
  excludable in slim builds; `camel-language-minijinja` stays linked in
  every profile (camel-template hard-depends on it through
  non-optional, out-of-lease paths — deferred with the
  bundles-internal bridges). A `full` meta-feature re-aggregates today's
  exact default set (`default = ["full"]`); `slim-http` is a named empty
  feature marking the `--no-default-features` baseline (http + core
  plumbing non-optional). camel-bundles is already consumed with
  `default-features = false` via the workspace table; its unconditional
  bridge deps (jms, sql, redis, opensearch, ws, cxf, xslt, xj) are NOT
  excludable from camel-cli's side and stay linked in every profile —
  recorded as a deferral (camel-bundles-side optionalization is a
  separate zone).
- **Executable guards:** a camel-cli integration test asserts the default
  dependency closure against a committed golden `cargo tree` snapshot
  (exact command, normalization, and thresholds specified in design.md),
  asserts slim builds exclude the controllable bridge set, and asserts
  the mimalloc stack is absent under `--all-features` after dedup.
- **Measurements (local method, mission 99 replica):** binary size
  (exact bytes), cold startup (marker + help modes, n=30 per mode per
  binary, median and p90 reported), and max RSS (median). Acceptance
  bands: full-build startup median within ±5% of baseline, RSS median
  within ±2 MB, size delta explained; slim-http deltas reported as the
  win.
- **Sweep (rc-kyq15):** verify the jobhelp wave has no outstanding items;
  re-defer or file anything found.
- **Excluded:** benchmarks/ (mission 98-v2), camel-core/src/cache
  (mission 101), camel-api/redaction (mission 103), camel-bundles'
  Cargo.toml itself (consumed, never edited), CI workflow changes.

## Acceptance criteria

- Exactly one allocator override path exists (`jemalloc`); the mimalloc
  stack (mimalloc, libmimalloc-sys) is absent from camel-cli's resolved
  graph under every feature combination; the canonical policy is
  documented.
- Default-feature build semantics are unchanged: the default dependency
  closure (normalized `cargo tree` crate list) is identical to the
  pre-change golden snapshot, and the default binary's startup median,
  RSS median, and exact size show no regression within the stated bands.
- A slim-http build (`--no-default-features --features slim-http`)
  compiles, boots a fixture that binds an http consumer, and excludes
  the controllable bridge set — kafka, grpc, wasm, llm, mcp, mqtt,
  surrealdb, exec, lsp, and the js/rhai/jsonpath/xpath language runtimes
  (minijinja deferred via camel-template) — from its dependency graph
  (verified by the profile test).
- Cold RSS + startup deltas (full vs slim-http) are measured with the
  local mission-99-replica method and recorded in the change evidence.
- rc-kyq15 sweep items are done or re-deferred with reasons in the park
  report.

## Risk budget

Acceptable: feature-surface churn inside `crates/camel-cli` +
`Cargo.lock`; golden-snapshot test that must be regenerated when the
default dependency set intentionally changes. Out of bounds: any behavior
change of default builds, edits outside camel-cli/root `Cargo.toml`
(lease), editing camel-bundles or benchmarks, making jemalloc default,
touching the redaction surface or allocator metrics wiring.

Bd: rc-rrz6a, rc-g0009, rc-kyq15.

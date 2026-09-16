# Phase A — Allocator + feature audit (mission 102 / clidiet)

Base: `6f904f09` (main HEAD at worktree creation). All commands run in
`/home/shared/rust-camel-worktrees/clidiet`.

## 1. Allocator resolution (rc-rrz6a)

| Question | Evidence | Answer |
|---|---|---|
| Do any allocator crates resolve under DEFAULT features? | `cargo tree -p camel-cli -e no-dev` → 0 matches for `jemalloc\|mimalloc`; `cargo tree -i libmimalloc-sys` / `-i tikv-jemalloc-sys` → "did not match any packages" | **No.** Default build uses the system allocator; neither jemalloc nor mimalloc is linked. |
| Why did the bd see BOTH in the graph? | `cargo tree -p camel-cli --all-features -i mimalloc` → pulled by `camel-cli` only; same for `-i tikv-jemallocator` | `--all-features` artifact (plus Cargo.lock listing optional deps unconditionally). Each allocator has exactly ONE puller: camel-cli's own optional deps. |
| Feature unification across the workspace (another crate pulling allocator #2)? | inverted trees above show `camel-cli` as sole dependent; workspace grep: only `benchmarks/contenders/rust-camel-lib` has its own independent `mimalloc` (untouchable zone, does not depend on camel-cli's feature) | **Refuted.** No transitive culprit. |
| Which allocator actually backs the binary? | `main.rs`: `#[cfg(feature="jemalloc")] #[global_allocator] Jemalloc`; `#[cfg(all(feature="mimalloc", not(feature="jemalloc")))] MiMalloc` | jemalloc wins any combined build (musl production images ship jemalloc; gnu image = system glibc malloc per Dockerfile; nothing enables mimalloc anywhere in build tooling — CI, Dockerfile, `.cargo/config.toml` all clean). |
| What does each allocator power? | jemalloc: global_allocator + `allocator_metrics.rs` gauges (ADR-0066) + heap profiling (rc-i9f9 parked verify). mimalloc: one `#[global_allocator]` cfg block, structurally unreachable when jemalloc is on, unused by every build path | **mimalloc is the dead one.** |

Dedup fix (per pre-flight e_opus ruling, GO-WITH-CAVEATS): remove the
`mimalloc` feature + dep + `main.rs` cfg block from camel-cli entirely;
prune Cargo.lock — camel-cli's `"mimalloc",` dependency edge leaves the
lock, the package ENTRIES stay (the benchmarks fixture keeps its own);
the cargo-tree closure test is the AC evidence. jemalloc wiring, allocator_metrics.rs,
tikv-jemalloc-ctl stay untouched. Expected binary-size/RSS delta under
default features: zero (proof of "no behavior change", a pass not a fail).

## 2. Slim profile surface (rc-g0009)

Current camel-cli feature surface (`crates/camel-cli/Cargo.toml`):

- `default = ["otel","grpc","wasm","http-static","llm","surrealdb","exec","mqtt","mcp","integration-http","integration-sql","security","redis-tls"]`
- Already-optional components: kafka, mqtt, grpc, wasm, llm, mcp, surrealdb, exec (+ kafka-static/dynamic-linking/mqtt-tls variants).
- **Non-optional heavy deps** (the diet targets): camel-component-jms, -sql, -redis, -opensearch, -ws, -cxf, camel-xslt, camel-xj, camel-lsp (+ tower-lsp, ariadne), camel-master, camel-wit, camel-integration-test, camel-dataformat set, camel-core language features (lang-js/rhai/jsonpath/xpath/minijinja).
- Default dep closure: **1218 unique crates** (`cargo tree -e no-dev --prefix none | sort -u` — note: workspace-union scope, no `-p`; the like-for-like `-p camel-cli` closure used in matrix.txt/results is a different, smaller set), 25 component-crate nodes.

**Corrected bundles picture (bless round 1 finding):** the workspace
table already consumes camel-bundles with `default-features = false`
(root Cargo.toml), and camel-cli already forwards the seven feature-gated
bundles — no default re-pull exists. The REAL constraint is
camel-bundles' **unconditional** bridge deps (cxf, jms, opensearch,
redis, sql, ws, xj, xslt — plus core plumbing): those eight stay linked
in every camel-cli profile and cannot be excluded consumer-side. They
are deferred (camel-bundles-side optionalization, separate zone; deferral
ledger + bd by the master).

camel-cli-controllable diet surface (Phase A conclusion):
- Already-optional bridges excluded by `--no-default-features`:
  kafka, grpc (tonic/protobuf stack), wasm (wasmtime), llm, mcp, mqtt,
  surrealdb, exec, plus the default-on forwards otel, security,
  http-static, integration-http, integration-sql, redis-tls.
- Non-optional today, to be feature-gated by this change: the LSP server
  stack (camel-lsp, tower-lsp; ariadne stays non-optional — `camel lint`
  renders with it) and the five camel-core language
  features (lang-js, lang-rhai, lang-jsonpath, lang-xpath,
  lang-minijinja) hardcoded on the camel-core dep. Execution note
  (task 2.2): the minijinja CRATE is not excludable — camel-template
  hard-depends on it via non-optional out-of-lease paths; deferred.

Feature-unification coherence traps to gate in the compile matrix:
`otel` (forwards camel-component-http/otel + ws/otel), `security`
(camel-bundles/security), `redis-tls` (forwards camel-component-redis/tls
— redis itself arrives via camel-bundles, always present),
`integration-http`/`integration-sql` (camel-integration-test forwards).

## 3. Baseline (default build, release, local replica method)

Measured with `evidence/bench-cold-local.sh` (mission 99 M1-replica:
marker mode = spawn → `BENCH_ROUTE_READY` on a one-shot timer→log route
plus an http consumer that must bind; help mode = spawn → exit;
`/usr/bin/time -v` for max RSS; 3 warmups discarded; n=30 per mode).
Binary: `cargo build -p camel-cli --release` at base commit `6f904f09`
(copied to /tmp/clidiet-camel-base; the fixture boots both routes and the
process self-stops after the one-shot timer completes).

| Metric (n=30) | marker mode | help mode |
|---|---|---|
| median | 88 ms | 27 ms |
| p90 | 106 ms | 36 ms |
| min–max | 33–118 ms | 8–44 ms |
| max-RSS median | 48,942 kb | 19,152 kb |
| max-RSS range | 48,844–49,100 kb | 18,980–19,152 kb |

Binary size: **104,858,696 bytes** (100 MiB). Zero FAIL samples.

Raw data: `results/full-base.{marker,help}.txt` (ms) and
`.rss.txt` (kb); size in `results/full-base.size.txt`.
Post-change default build (task 3.1): `results/full-post.{marker,help}.txt`
(ms) and `.rss.txt` (kb); comparison in `results-full.txt`.

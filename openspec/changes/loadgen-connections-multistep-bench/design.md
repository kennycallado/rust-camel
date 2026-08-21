# Design: loadgen-connections-multistep-bench

## Context

`measure-throughput` today (benchmarks/harness/loadgen):

- `cli.rs::measure_throughput_main` parses `--url`, `--duration-secs`, `--warmup-secs`, `--workers`, `--out`. `workers` defaults to `available_parallelism()`.
- `cli_runtime.rs::run_measure_throughput` builds a multi-thread tokio runtime with `worker_threads(workers.max(2))`, one shared `reqwest::Client` with `pool_max_idle_per_host(workers)`, then spawns exactly `workers` request-loop tasks. Each task issues sequential POSTs; in-flight concurrency is therefore bounded by both the task count and the pool cap — all three derived from the single flag.
- The result JSON artifact is assembled at `cli_runtime.rs:~580` (`serde_json::json!`); its error field is `error_rate_pct` (percent units). `throughput.rs:~189` is a #[test]-only JSON literal mirroring that shape, not a second published artifact.

Scenario fixtures live under `benchmarks/scenarios/<name>/rust-camel-cli/` (`routes/*.yaml` + `Camel.toml`). The YAML DSL supports `set_body`, `set_header`, `script` (rhai), `log`, `to`, `choice`. `camel-surrealdb` is remote-only (no embedded engine path) — see Change 2 deviation.

## Change 1: --connections knob

- New flag `--connections N` in `measure_throughput_main` (default: `workers`, preserving current behavior bit-for-bit when absent).
- Threading: `run_measure_throughput(url, duration_secs, warmup_secs, workers, connections, output_path)`.
- Runtime sizing stays on `workers` (`worker_threads(workers.max(2))`) — worker threads are CPU lanes, not concurrency.
- Request tasks: spawn `connections` loops instead of `workers`.
- Pool: `pool_max_idle_per_host(connections)` so the pool never throttles below the requested in-flight count.
- The result JSON artifact gains `"workers": N` and `"connections": M`. Greppable flat fields, not nested.

Rationale for keeping workers as runtime sizing only: wrk-style semantics — connections model network concurrency; threads model client-side CPU. Conflating them (status quo) is what hid the convoy.

## Change 2: multi-step scenario fixture

Implementation amendment (task 2.1, discovered end-to-end): the route begins with
`stream_cache: true` + `convert_body_to: text` — the http consumer yields a
`Body::Stream` and rhai's `body` would read empty without materialization; the
preflight grep is case-insensitive because hyper emits lowercase header names.

New `benchmarks/scenarios/multi-step/rust-camel-cli/`:

- `routes/multi-step.yaml`: `from(http://0.0.0.0:8081/bench-multi)` with steps in this order:
  1. `script` (rhai): string-map/serialize work on the body (`body = body.to_upper() + "-M1"`)
     plus property seed (`properties["stage"] = "one"`) — CPU cost per exchange, mirrors
     demo rhai usage. Mutating scripts expose the `properties` map.
  2. second `script` (rhai): branch-seeded mutation — happy path sets `stage = "two"` and
     appends `"-M2"` to the body; any other state sets `stage = "branch-fail"`.
  3. `choice`: when-predicate (rhai) `property("stage") == "two"` → branch steps:
     `set_header` key `X-Bench-Stage`, `language: "rhai"`, value expression
     `property("stage")` (evaluates `"two"` inside the branch). The header lives INSIDE
     the happy branch deliberately: a skipped or short-circuited choice leaves the
     header absent, so the preflight proves branch execution, not just body shape.
     otherwise → `set_body` literal `BRANCH-FAIL` (observably wrong on every axis).
     Expressions expose scalars via the `property(name)` accessor (distinct from the
     scripts' `properties` map).
  No terminal `set_body` on the happy path — the response body IS the accumulated script
  output.
- Correctness is asserted by a PREFLIGHT request (single request before the load phase)
  whose deterministic response body (`PING-M1-M2`) and header (`X-Bench-Stage: two`) are
  asserted to flow through every intended step. The sustained load phase emits NO
  per-exchange logs — high-volume logging can itself become the serialization point and
  mask pipeline contention (the BENCH_MULTI_TICK log idea was dropped for exactly this
  reason).
- `Camel.toml`: minimal, mirroring http-server fixture.
- Port 8081 to allow side-by-side runs with http-server (8080).
- Wrapper script mirrors `http-server-cli-wrapper.sh` marker handshake (`BENCH_ROUTE_READY`).

Deviation from the bd description (noted for history): the demo workload's *cache* leg is NOT reproduced. `camel-surrealdb` has no embedded/`mem://` engine path (`supported_schemes()` = ws/wss/http/https; pool factory unconditionally signs in against a remote server) and the CLI registers an empty datasource catalog, so an embedded-cache step would require Docker or a new datasource wiring feature — out of scope here (candidate follow-up if a cache leg proves necessary). Per-exchange work is made non-trivial with in-DSL steps only: two rhai scripts of distinct shapes, header derivation, and branch evaluation.

Alternative rejected: extending the existing http-server route in place — would silently change the historical baseline scenario that published v1–v4 numbers reference.

## Risks / Trade-offs

- [low] rhai step cost dominates → route tuned so per-exchange work stays sub-millisecond; signal target is contention shape, not absolute numbers.
- [mitigated] back-compat: default `connections = workers` reproduces today's load exactly.
- [accepted] cache leg absent from the multi-step route (see Change 2 deviation) — convoy detection relies on CPU-side pipeline work; a storage-leg scenario is a possible follow-up.

## Migration Plan

Additive only. Existing invocations and published-artifact consumers see two new JSON fields; no schema breaks.

## Open Questions

None — resolved during grounding (pool/task/runtime decoupling chosen over reqwest pool-only approach because pool-only cannot exceed the task count).

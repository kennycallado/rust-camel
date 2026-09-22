# Design: mhandle

## Approach

Emission is facade-mediated, classification-site-owned:

1. **Facade threading.** `ComponentMetrics` (camel-api, re-exported) is the
   only metrics type that crosses crate boundaries. It is constructed at the
   single production construction site (`camel-config` `context_ext.rs`,
   inside `configure_context_with_beans`) as
   `ComponentMetrics::new(ctx.metrics(), config.observability.metrics.components_enabled())`
   — the shared late-bound `Arc<MetricsHandle>` (so collectors registered
   after repo construction are still observed, per the handle's
   composition-not-replacement contract) plus the lever snapshot taken from
   the same `CamelConfig` the context itself snapshots
   (`CamelContext::component_metrics_enabled` reads the same field later).
   The lever arrives as a bool by design: camel-api cannot depend on
   camel-core where `MetricsLeversConfig` lives; camel-config already
   imports it (`use camel_core::MetricsLeversConfig`).

2. **Classification-site emission.** The two places that call
   `is_transient_redis_error` in camel-redis-repo are the ONLY emission
   sites:
   - `executor::execute_retry_safe` transient arm: emit
     `observe("redis", <op>, true)` once, before refresh+retry. A recovered
     retry still counts one C1 event; a second (surfaced) failure does not
     double-emit — one classification, one observation.
     `scan_unlink_pattern` forwards the caller's repo-level label so SCAN
     and UNLINK batches share the invoking operation's label.
     Cardinality is per-classification, not per-call: a multi-page `clear`
     with N transiently-failing batches emits N `clear` observations (one
     per `execute_retry_safe` transient arm), each a genuine transport
     event.
   - `idempotent_repo::add` C1 arm (lost-outcome SET NX): emit alongside the
     existing `transient_refreshes` increment — the atomic counter stays the
     per-instance view, the facade the fleet view.
   Non-transient errors emit nothing: C1 means transient classification, and
   hard failures already surface through route-level error handling.

3. **Labels.** component = `"redis"` (scheme convention: seda/surrealdb/wasm
   precedents). operation = closed literal set at call sites:
   cache `set`/`get`/`remove`/`clear`/`invalidate_prefix`, idempotent
   `add`/`contains`/`remove`/`clear` (`remove` and `clear` are shared
   across both repos by design; the operation label denotes the logical
   op, not the repo type). outcome is the facade's two-literal
   set (`failure` here). All values are caller-bounded literals through the
   facade — covered by its existing `allow-open-label rc-gm6s` /
   `rc-otxh` annotations; no new annotation sites expected (verify with
   lint-metric-labels). camel_cache_* naming is untouched (grep confirmed:
   no live emitter in code; the post-rename family rides the prometheus
   dotted-name normalizer, out of scope here).

4. **Signatures.**
   - `RedisCacheRepository::connect/with_executor(.., metrics: ComponentMetrics)`
   - `RedisIdempotentRepository::connect/with_executor(.., metrics: ComponentMetrics)`
   - `execute_retry_safe(ex, cmd, metrics: &ComponentMetrics, operation: &'static str)`
   - `scan_unlink_pattern(ex, pattern, metrics: &ComponentMetrics, operation: &'static str)`
   - `build_redis_cache_repo(ccfg, metrics)` / `build_redis_idempotent_repo(icfg, metrics)`
   `ComponentMetrics` is `Send+Sync` (Arc<dyn MetricsCollector> + bool) —
   satisfies the repo trait objects' bounds. camel-api, camel-prometheus,
   and the facade itself are NOT modified.

## Affected crates

- `camel-redis-repo`: facade field on both repos; emission at the two
  classification sites; constructor/test-seam params; RecordingCollector
  test evidence (fake executor, mirrors rc-2or1's
  `transient_add_increments_observability_counter` and the metrics-batch
  wiring-harness pattern).
- `camel-config`: builders take + thread the facade; lever snapshot helper
  `redis_repo_component_metrics(config)` (unit-testable without network);
  call sites in `configure_context_with_beans`.
- `camel-test`: three TLS live-test call sites pass a lever-off facade
  (compile-only; suites stay ignored/live).

## Architecture boundaries

Services-layer change (camel-redis-repo is a service crate). No Runtime,
DSL, or Component boundary crossed: the redis component already gets the
facade via `RuntimeObservability` at endpoint creation and is untouched.
Metrics flow respects the dashboard-observability doctrine: lever gates only
the component-operations family; error-family forwarding is unconditional;
labels stay closed sets.

## Alternatives considered

- **Emit from the executor (`MultiplexedRepoExecutor`)** — rejected: wrong
  layer; op labels are repo-level semantics, transport would need a label
  injection anyway.
- **Emit only on `add`'s lost-outcome arm** — rejected: leaves cache-repo
  transient classifications (retry-recovered) unobservable, the larger half
  of the C1 class.
- **Also wire hits/misses into camel_cache_*** — rejected: out of AC; that
  family's wiring is tracked separately (rc-kah44) and its emitters are
  currently absent from code.
- **`increment_retry_attempt` instead of component-ops** — rejected: bd AC
  names the component-operations family explicitly.

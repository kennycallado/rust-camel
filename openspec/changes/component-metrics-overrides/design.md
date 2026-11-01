# Design: component-metrics-overrides

## Approach

### 1. Config model (camel-core domain, parsed by camel-config)

`MetricsLeversConfig` gains
`component_overrides: BTreeMap<String, bool>` (serde: absent map =
empty, `deny_unknown_fields` unchanged). One accessor owns the
precedence:

```text
component_enabled(C) = enabled && override.get(C).unwrap_or(components)
```

`components_enabled()` keeps its current meaning for the default. The
master `enabled` gate is applied LAST, so `enabled = false` wins over
any override; the error family has no lever and is forwarded before
this check, as today.

`CamelConfig::validate()` validates each override key as a component
identity: non-empty ASCII `[a-z0-9][a-z0-9_-]*`. An override for an
unregistered component is inert: registration is runtime state, and a
config that loads before registration must stay valid.

Profiles and includes use the existing recursive TOML merge
(`merge_toml_values`): tables merge key-by-key, scalars and arrays
replace. A profile section therefore overrides only the keys it names,
and nested `component_overrides` maps merge per component. The docs
state this explicitly.

### 2. Scoped handle (camel-api + camel-component-api)

`ComponentMetrics` carries the collector handle, the component identity
(`Arc<str>`), and the effective boolean snapshot:

```text
ComponentMetrics::new(collector, component, enabled)
ComponentMetrics::observe(operation, failed)
  failed  -> increment_errors(component, "e:{component}:{operation}")
  enabled -> record_component_operation(component, operation, outcome)
```

This preserves late binding (the collector `Arc` is the late-bound
`MetricsHandle`), NoOp, composite, and test collectors. The trait
`MetricsCollector` does not change. The handle is cheap to clone; one
handle is built per boundary and can be shared (e.g. a repository and
its payload store).

Because camel-component-api cannot depend on camel-core's
`MetricsLeversConfig`, camel-api also owns the small snapshot type:

```text
ComponentMetricsLevers { enabled: bool, components: bool,
                         overrides: Arc<BTreeMap<String,bool>> }
  enabled_for(C) = enabled && overrides.get(C).unwrap_or(components)
```

`enabled` is kept distinct from `components` precisely so an override
can never resurrect the family under the master switch. The overrides
map is shared behind `Arc`, so cloning the snapshot per
`component_metrics()` call is cheap.

`ComponentContext` replaces `component_metrics_enabled() -> bool` with
`component_metric_levers() -> ComponentMetricsLevers` (default: all
off). `RuntimeObservability::component_metrics(component: &str)`
builds the scoped handle from `metrics()` plus the snapshot.
`RegistryComponentContext` keeps its `components_enabled` argument for
source compatibility (mapping to a uniform snapshot) and gains
`with_component_metric_levers(...)` so the CLI path (wasm/beans) sees
overrides.

### 3. HTTP consumer

`HttpConsumer` keeps its `Arc<dyn RuntimeObservability>`. Each consumer
builds its own `http`-scoped `ComponentMetrics` from ITS runtime at
`start()` and registers it in the route entry alongside the envelope
sender; `dispatch_handler` selects the metrics together with the sender
from the same registry entry. The shared Axum server is process-global
and keyed by host/port, so a server-level handle would charge requests
from another context to the wrong collector and the wrong effective
switch; per-route registration keeps collector identity and override
resolution correct when several contexts share one listener.

The per-request task only classifies its reply (`HttpReply` gains
`failed: bool`, set for the cancel-503, auth-denial,
stream-already-consumed, and pipeline-error replies).
`dispatch_handler` owns exactly-once accounting through one state cell
(`Arc<AtomicU8>` with `Unrecorded | Succeeded | Failed | Delegated`);
the first transition wins and every later attempt is a no-op:

- A request guard is created when a route entry is selected. Its `Drop`
  records failure only while the state is `Unrecorded`, which covers
  handler cancellation while awaiting the route channel or the reply
  (for example the tower-http timeout layer dropping the future) and
  any rejection after the guard exists. The guard can therefore never
  fire on the normal path: returning the response first moves the state
  to `Delegated` (or leaves it `Failed` for a classified reply).
- A classified failed reply records failure at receipt and leaves the
  state terminal; its response body meter is inert.
- Early rejections before a route round-trip (request body over the
  limit, inflight-semaphore rejection, route-channel rejection, and the
  dispatcher's oversized-response replacement) record failure
  directly.
- A route-selected HTTP status (for example `CamelHttpResponseCode =
  500`) is NOT a failure: the route completed and produced a
  well-formed response.

The response body is wrapped in a completion meter (`http_body::Body`
over buffered and streaming bodies) that only acts while the state is
`Delegated`: exhaustion records success, a body error records failure,
and a drop before exhaustion records failure. After every frame the
meter consults the inner body's `is_end_stream()`; a final frame that
signals end-of-stream records success immediately, so normal teardown
never needs another poll. Two cases never reach the meter because no
body is sent: a body that is end-of-stream at construction (empty body)
and a response whose body is suppressed by protocol (`HEAD`, `204`,
`304`, `1xx`). Both record success at response construction. A reply
whose headers cannot build a valid response records failure.
Completion is therefore defined by the HTTP body contract, not by
handoff; a transport write error that occurs after the body is
exhausted is outside this metric.

- Static mounts are outside this requirement (`http-static` endpoint).
- No URL/host/header/body labels are added; retained `e:http:accept*`
  and `e:http:server-task-exited` labels are untouched, and the pinned
  client cache keeps its current unconditional behavior.

### 4. Migration

Every `.observe(component, operation, failed)` call site becomes a
scoped handle obtained from `component_metrics(component)` and a
two-argument `observe`. Redis repositories receive the scoped facade
from camel-config; their constructors and tests construct it with the
`redis` identity.

## Affected crates

- `camel-api`: scoped `ComponentMetrics`, new `ComponentMetricsLevers`.
- `camel-component-api`: `RuntimeObservability::component_metrics(&str)`,
  `ComponentContext::component_metric_levers`, test doubles.
- `camel-core`: `MetricsLeversConfig.component_overrides` + precedence,
  `CamelContext`, `ControllerComponentContext`, `RegistryComponentContext`.
- `camel-config`: validation, redis facade wiring, profile docs.
- `camel-http`: consumer `consume` metric, per-route registry entries
  carrying the scoped handle, reply classification, completion meter.
- Component call sites: kafka, redis, seda, wasm, mail, ws, cxf,
  opensearch, rabbitmq, surrealdb (mechanical signature migration).
  gRPC passes retry labels only and has no facade site, so it is untouched.
- `camel-test`: `COMPONENT_OP_SETS` http vocabulary gains `consume`.

## Architecture boundaries

The handle stays a contract type in camel-api; camel-component-api only
exposes it through the existing observability trait. Component crates do
not read config. camel-core owns the lever precedence; camel-config
owns parsing and validation. No data-plane dependency on the control
plane is added. ADR-0012 governs the error taxonomy; ADR-0066 Decision
5 (non-disableable errors) and Decision 13/14 (label vocabulary and the
forbidden double-wiring) are preserved and amended only for the new
override switch and the `http:consume` vocabulary entry.

## Alternatives considered

- Per-family override matrix (`component_overrides.http.duration = ...`):
  rejected by the design ruling; one switch per component.
- A `MetricsCollector` decorator per component instead of a facade:
  would multiply trait methods and break the composite/late-binding
  contract.
- Emitting `consume` in the per-request task at envelope handoff:
  simpler, but it measures handoff instead of completion/abort.
- Polling the reply channel without a guard: a dropped handler future
  (timeout layer) would leave the request unrecorded.
- Validating override keys against the registry at load: impossible
  before registration and hostile to config layering.

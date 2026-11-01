# Hot reload

Hot reload swaps route pipelines at runtime without stopping the context.
When a route file changes on disk, the runtime compiles the new pipeline
and swaps it atomically. In-flight exchanges complete against the
pipeline snapshot they entered.

## Architecture

The runtime stores the active pipeline behind `ArcSwap`. A swap
publishes a new `Arc` in one atomic step. New exchanges see the new
pipeline immediately. Exchanges already in flight hold their existing
`Arc` and finish against the old pipeline. Old and new pipelines coexist
until the last in-flight exchange drains.

**Reference**: [ADR-0004](../adr/0004-hot-reload-atomic-pipeline-swap.md)

## Configuration

Set `watch_debounce_ms` in `Camel.toml` to control the debounce delay.
The watcher waits this long after the last file event before it reloads.
Increase the value if one save triggers several rapid reloads.

```toml
{{#include ../../../examples/hot-reload-yaml/Camel.toml:hot-reload-config}}
```

## Usage

### Load the debounce from config

The `hot-reload-yaml` example reads `watch_debounce_ms` from `Camel.toml`
and passes it to `watch_and_reload`.

```rust,ignore
{{#include ../../../examples/hot-reload-yaml/src/main.rs:load-debounce}}
```

### Start the watcher

The `hot-reload` example resolves the directories to watch, then starts
`watch_and_reload` in a background task. A `CancellationToken` stops the
watcher on shutdown.

```rust,ignore
{{#include ../../../examples/hot-reload/src/main.rs:watch-setup}}
```

The watched route file uses a plain YAML route:

```yaml
{{#include ../../../examples/hot-reload/routes/route.yaml:hot-reload-route}}
```

## How it works

1. The file watcher monitors route directories for changes.
2. After the debounce window, it calls `discover_routes` to reload route
   definitions.
3. It computes reload actions (swap, add, remove) by comparing the old
   and new routes.
4. It applies each action on the runtime controller.

The watcher runs in a background task. Pass a `CancellationToken` to stop
it on shutdown.

## Dev watch for template closures

The `camel run --watch` watcher covers more than route files. For the
Template component it watches the dependency closure of each rendered
entry: the entry file plus every nested `include`, `import`, `extends`, and
`from` target. A closure edit re-derives the affected routes and rebuilds
the compiled set.

Targets are re-derived after every committed reload and every route reload
pass. Adding an include adds that file to the watch sources; removing one
drops it.

The swap is atomic and fail-closed. A failed or timed-out build keeps the
prior compiled set serving, logs a redacted error, and leaves the route
Running. The build is bounded by `reload-timeout-ms` under
`[components.template.limits]` (default 5000 ms). Failures cause no route
restart, no `RouteStatus` change, and no journal events.

Template and validator protobuf reloads commit together per route. A
failed target blocks the whole same-route commit, so a broken `.proto` also
blocks a template reload and the reverse. See
[Validator](../components/validator.md#descriptor-recompile-in-dev-watch)
for the proto side and the gRPC non-goal.

Dev watch is dev-only: compiled artifacts and non-watch runs never create a
watcher. See the [CLI hot-reload](../getting-started/cli.md#hot-reload)
reference and [Template](../components/template.md#dev-watch) for the
component view.

## When to use

Use hot reload for zero-downtime updates. Edit a route, save the file,
and the running context adopts the change within the debounce window.
This fits long-running integration services that cannot restart during
traffic. Do not use hot reload where route correctness needs a full
compile-time check. Prefer the Rust builder API and a redeploy for that
case.

**Reference**: `reload_watcher::watch_and_reload` in the [Runtime crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md)

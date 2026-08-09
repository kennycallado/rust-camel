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

```rust
{{#include ../../../examples/hot-reload-yaml/src/main.rs:load-debounce}}
```

### Start the watcher

The `hot-reload` example resolves the directories to watch, then starts
`watch_and_reload` in a background task. A `CancellationToken` stops the
watcher on shutdown.

```rust
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

## When to use

Use hot reload for zero-downtime updates. Edit a route, save the file,
and the running context adopts the change within the debounce window.
This fits long-running integration services that cannot restart during
traffic. Do not use hot reload where route correctness needs a full
compile-time check. Prefer the Rust builder API and a redeploy for that
case.

**Reference**: `reload_watcher::watch_and_reload` in the [Runtime crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md)

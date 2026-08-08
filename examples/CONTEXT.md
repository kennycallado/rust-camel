## Log-level policy

The three example apps (container-hot-reload, hot-reload, hot-reload-yaml) each have a single `error!` site at startup (bootstrap failure — the demo cannot proceed). All sites: (c) system-broken. Each keeps `error!` with `// log-policy: system-broken`. No metric call (the demo is exiting).

Sites:
- `examples/container-hot-reload/src/main.rs`
- `examples/hot-reload-yaml/src/main.rs`
- `examples/hot-reload/src/main.rs`
